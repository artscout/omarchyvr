use omarchyvr_hud::{FrameResult, Hud, Opts, Placement};
use pimax_ipd::devices::{Real, Sim};
use pimax_ipd::png_sink::PngSink;
use pimax_ipd::{run, Plan};
use pimax_ipd::manual::{self, Key, KeyParser, Pacer, Timing};
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const USAGE: &str = "\
pimax-ipd: guided IPD test with an in-headset screen (Dream Air, Monado)

  pimax-ipd [--seconds S] [--apply [--mm X]] [--overlay] [--placed] [--distance M] [--usb-device PATH] [--no-hud]
  pimax-ipd manual [--overlay] [--placed] [--distance M] [--no-hud] [--simulate [--png-dir DIR] [--sim-lens MM]]
  pimax-ipd --simulate [--png-dir DIR] [--sim-field34 MM] [--sim-origin MM] [--sim-lens MM] [--apply [--mm X]]

Shows 'Look straight ahead', a 3-2-1 countdown, the measurement (default 5 s) with a progress bar, then measured
IPD, lens IPD and the difference; with --apply the lenses move (the one approved IPD command, 58-72 mm) and the
result is shown. --apply REFUSES (nothing sent, 'Measurement looks wrong') when the measurement is outside 58-72 mm
or more than 4 mm from the current lens IPD, unless --mm gives the value. The screen stays 5 s (Enter ends it).

  --overlay      XR_EXTX_overlay session on top of a running main app (WayVR / desk mode); default is a main app
  --placed       fix the panel in the room where you looked at start (default: head-locked)
  --no-hud       terminal only
  --simulate     no headset: fake measurement, screens written as PNG files (default DIR /tmp/pimax-ipd-sim)

manual: pick the sharpest IPD by hand. The panel shows a sharpness test pattern, the target IPD, the live lens IPD
and a status. A/D -/+0.5 mm, W/S +/-1 mm (58-72; arrows work too), Enter saves the lens IPD to ~/.config/omarchyvr/ipd.env and
exits, q exits (Esc does nothing). Keys only change the target on screen; the motor command is sent 700 ms after the last key,
never during a move (waits until the lens is within 0.3 mm or 6 s), at least 1.5 s after the previous one.
No eye tracking is used: the eye chip is not opened.

The eye chip can be opened by one process only: start Monado without eye tracking (omarchyvr vr start --force with
PIMAX_MONADO_BUILD=~/src/monado-eye/build, or omarchyvr desk start --force without --eye) and run with
XR_RUNTIME_JSON=~/src/monado-eye/build/openxr_monado-dev.json.
";

struct Args(Vec<String>);

impl Args {
    fn flag(&mut self, n: &str) -> bool {
        match self.0.iter().position(|a| a == n) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }

    fn value(&mut self, n: &str) -> Result<Option<String>, String> {
        match self.0.iter().position(|a| a == n) {
            Some(i) if i + 1 < self.0.len() => {
                let v = self.0.remove(i + 1);
                self.0.remove(i);
                Ok(Some(v))
            }
            Some(_) => Err(format!("{n} needs a value")),
            None => Ok(None),
        }
    }

    fn num(&mut self, n: &str) -> Result<Option<f32>, String> {
        self.value(n)?.map(|v| v.parse::<f32>().map_err(|_| format!("{n}: not a number: {v}"))).transpose()
    }
}

fn main() -> ExitCode {
    match real_main() {
        Ok(c) => ExitCode::from(c as u8),
        Err(e) => {
            eprintln!("pimax-ipd: {e}");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> Result<i32, String> {
    let mut a = Args(std::env::args().skip(1).collect());
    if a.flag("--help") || a.flag("-h") {
        print!("{USAGE}");
        return Ok(0);
    }
    if a.0.first().map(String::as_str) == Some("manual") {
        a.0.remove(0);
        return manual_main(a);
    }
    let simulate = a.flag("--simulate");
    let apply = a.flag("--apply");
    let overlay = a.flag("--overlay");
    let placed = a.flag("--placed");
    let no_hud = a.flag("--no-hud");
    let seconds = a.num("--seconds")?.unwrap_or(5.0).clamp(1.0, 120.0) as f64;
    let mm = a.num("--mm")?;
    let distance = a.num("--distance")?.unwrap_or(1.5).clamp(0.5, 5.0);
    let usb = a.value("--usb-device")?.map(PathBuf::from);
    let png_dir = a.value("--png-dir")?.map(PathBuf::from).unwrap_or_else(|| "/tmp/pimax-ipd-sim".into());
    let f34 = a.num("--sim-field34")?.unwrap_or(64.2);
    let org = a.num("--sim-origin")?.unwrap_or(f34 - 0.2);
    let sim_lens = a.num("--sim-lens")?.unwrap_or(63.0);
    if let Some(x) = a.0.first() {
        return Err(format!("unexpected argument {x}\n\n{USAGE}"));
    }
    if mm.is_some() && !apply {
        return Err("--mm only makes sense with --apply".into());
    }
    let mut plan = Plan { seconds, apply, mm, ..Plan::default() };
    pimax_eye::sigint::install();
    let skip = Arc::new(AtomicBool::new(false));
    let mut say = |s: &str| println!("{s}");
    let stop = || pimax_eye::sigint::stopped();

    if simulate {
        plan.step = std::time::Duration::from_millis(50);
        plan.hold = std::time::Duration::from_millis(50);
        plan.write_env = false;
        plan.seconds = 1.0;
        let sink = PngSink::new(png_dir.clone()).map_err(|e| format!("{}: {e}", png_dir.display()))?;
        let mut dev = Sim::new(Some(sim_lens), f34, org);
        let code = run(&mut dev, &sink, &plan, &mut say, &skip, &stop);
        println!("simulated: {} screens in {} (exit code {code}, sent {:?})", sink.count(), png_dir.display(), dev.sent);
        return Ok(code);
    }
    spawn_keys(skip.clone());
    let mut dev = Real::new(usb, apply);
    if no_hud {
        struct Null;
        impl omarchyvr_hud::Sink for Null {
            fn show(&self, _: omarchyvr_hud::Screen) {}
        }
        return Ok(run(&mut dev, &Null, &plan, &mut say, &skip, &stop));
    }
    let mut hud = Hud::new(Opts {
        placement: if placed { Placement::PlacedAtStart } else { Placement::HeadLocked },
        distance,
        overlay,
        app_name: "pimax-ipd".into(),
        input: false,
    })?;
    println!("OpenXR runtime: {}", hud.runtime_name());
    let handle = hud.handle();
    let worker = {
        let (handle, skip) = (handle.clone(), skip.clone());
        std::thread::spawn(move || {
            let mut say = |s: &str| println!("{s}");
            let code = run(&mut dev, &handle, &plan, &mut say, &skip, &|| pimax_eye::sigint::stopped());
            handle.request_exit();
            code
        })
    };
    hud_loop(&mut hud, &handle, worker, &skip)
}

/// Pump the HUD frames until the worker is done (or the runtime ends the session); returns the worker's code.
fn hud_loop(hud: &mut Hud, handle: &omarchyvr_hud::HudHandle, worker: std::thread::JoinHandle<i32>, skip: &AtomicBool) -> Result<i32, String> {
    let mut err = None;
    let mut done_at: Option<std::time::Instant> = None;
    loop {
        if pimax_eye::sigint::stopped() {
            handle.request_exit();
        }
        match hud.frame() {
            Ok(FrameResult::Exit) => break,
            Ok(_) => {}
            Err(e) => {
                err = Some(e);
                break;
            }
        }
        if worker.is_finished() {
            done_at.get_or_insert_with(std::time::Instant::now);
        }
        if done_at.is_some_and(|t| t.elapsed().as_secs() >= 3) {
            break; // the runtime did not end the session in time
        }
    }
    if let Some(e) = err {
        // The headset screen is gone: stop the procedure, keep the terminal output.
        skip.store(true, Ordering::SeqCst);
        let code = worker.join().unwrap_or(2);
        eprintln!("pimax-ipd: OpenXR error: {e}");
        return Ok(if code == 0 { 2 } else { code });
    }
    Ok(worker.join().unwrap_or(2))
}

fn spawn_keys(skip: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut b = [0u8; 1];
        let mut stdin = std::io::stdin().lock();
        while let Ok(1) = stdin.read(&mut b) {
            skip.store(true, Ordering::SeqCst);
        }
    });
}

/// Terminal in non-canonical, no-echo mode (Ctrl-C keeps working); restored on drop.
struct RawTerm(Option<libc::termios>);

impl RawTerm {
    fn enable() -> RawTerm {
        // SAFETY: plain termios calls on fd 0 with a zeroed, then filled, struct.
        #[allow(unsafe_code)]
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return RawTerm(None);
            }
            let orig = t;
            t.c_lflag &= !(libc::ICANON | libc::ECHO);
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(0, libc::TCSANOW, &t) != 0 {
                return RawTerm(None);
            }
            RawTerm(Some(orig))
        }
    }
}

impl Drop for RawTerm {
    fn drop(&mut self) {
        if let Some(t) = &self.0 {
            // SAFETY: restoring the attributes read in `enable`.
            #[allow(unsafe_code)]
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, t);
            }
        }
    }
}

/// Read stdin bytes (polling, so a lone Esc is told from an arrow) and send the keys.
fn spawn_key_reader(tx: std::sync::mpsc::Sender<Key>) {
    std::thread::spawn(move || {
        let mut parser = KeyParser::default();
        // Raw read(2) on fd 0: std's Stdin is buffered, so bytes of an arrow sequence that arrived together would
        // sit in its buffer where poll(2) cannot see them and the ESC would time out as a lone Esc.
        loop {
            let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd, count 1.
            #[allow(unsafe_code)]
            let r = unsafe { libc::poll(&mut p, 1, 50) };
            let key = if r > 0 {
                let mut b = [0u8; 64];
                // SAFETY: reading into a local buffer of the given length.
                #[allow(unsafe_code)]
                let n = unsafe { libc::read(0, b.as_mut_ptr().cast(), b.len()) };
                if n <= 0 {
                    eprintln!("pimax-ipd: stdin closed (EOF or error): no keyboard input");
                    return;
                }
                for &x in &b[..n as usize] {
                    if let Some(k) = parser.feed(x) {
                        if tx.send(k).is_err() {
                            return;
                        }
                    }
                }
                None
            } else if r == 0 {
                parser.timeout()
            } else {
                None
            };
            if let Some(k) = key {
                if tx.send(k).is_err() {
                    return;
                }
            }
        }
    });
}

fn manual_main(mut a: Args) -> Result<i32, String> {
    let simulate = a.flag("--simulate");
    let overlay = a.flag("--overlay");
    let placed = a.flag("--placed");
    let no_hud = a.flag("--no-hud");
    let distance = a.num("--distance")?.unwrap_or(1.5).clamp(0.5, 5.0);
    let png_dir = a.value("--png-dir")?.map(PathBuf::from).unwrap_or_else(|| "/tmp/pimax-ipd-manual-sim".into());
    let sim_lens = a.num("--sim-lens")?.unwrap_or(63.2);
    if let Some(x) = a.0.first() {
        return Err(format!("unexpected argument {x}\n\n{USAGE}"));
    }
    pimax_eye::sigint::install();
    let t0 = std::time::Instant::now();
    let clock = move || t0.elapsed();
    let mut say = |s: &str| println!("{s}");
    let (tx, rx) = std::sync::mpsc::channel();

    if simulate {
        let timing = Timing::scaled(20);
        let sink = PngSink::new(png_dir.clone()).map_err(|e| format!("{}: {e}", png_dir.display()))?;
        let mut motor = manual::SimMotor::new(sim_lens, timing.min_gap / 2);
        let mut pacer = Pacer::new(sim_lens, timing);
        // A scripted user: a burst, a pause, a big step, a correction, Enter.
        std::thread::spawn(move || {
            let pause = |ms: u64| std::thread::sleep(std::time::Duration::from_millis(ms));
            pause(100);
            for k in [Key::Right, Key::Right, Key::Right] {
                let _ = tx.send(k);
                pause(8);
            }
            pause(300);
            let _ = tx.send(Key::Up);
            pause(300);
            let _ = tx.send(Key::Left);
            pause(500);
            let _ = tx.send(Key::Enter);
        });
        let out = manual::run(&mut pacer, &mut motor, &sink, &rx, &clock, &mut say, &mut |mm| Ok(format!("(simulated) would save {}", pimax_eye::ipd::monado_line(mm))), &|| pimax_eye::sigint::stopped());
        println!("simulated: {} screens in {} ({out:?}, sent {:?})", sink.count(), png_dir.display(), motor.sent);
        return Ok(0);
    }

    let mut motor = manual::RealMotor::new()?;
    let mut lens0 = None;
    for _ in 0..6 {
        use pimax_ipd::manual::Motor;
        lens0 = motor.lens();
        if lens0.is_some() {
            break;
        }
    }
    let lens0 = lens0.ok_or("no lens IPD in the headset info reports (is the headset on and connected?)")?;
    println!("lens IPD {lens0:.2} mm; target starts there, nothing is sent until you press a key");
    println!("A/D -/+0.5 mm, W/S +/-1 mm (arrows also work), Enter save + exit, q exit");
    let mut pacer = Pacer::new(lens0, Timing::REAL);
    let _raw = RawTerm::enable();
    spawn_key_reader(tx);
    let stop = || pimax_eye::sigint::stopped();
    let mut save = |mm: f32| manual::save_env(mm);
    if no_hud {
        struct Null;
        impl omarchyvr_hud::Sink for Null {
            fn show(&self, _: omarchyvr_hud::Screen) {}
        }
        let out = manual::run(&mut pacer, &mut motor, &Null, &rx, &clock, &mut say, &mut save, &stop);
        return Ok(if out == manual::Outcome::NoLens { 2 } else { 0 });
    }
    let mut hud = Hud::new(Opts {
        placement: if placed { Placement::PlacedAtStart } else { Placement::HeadLocked },
        distance,
        overlay,
        app_name: "pimax-ipd".into(),
        input: false,
    })?;
    println!("OpenXR runtime: {}", hud.runtime_name());
    let handle = hud.handle();
    let gone = Arc::new(AtomicBool::new(false)); // set by hud_loop when the headset screen is gone
    let worker = {
        let (handle, gone) = (handle.clone(), gone.clone());
        std::thread::spawn(move || {
            let mut say = |s: &str| println!("{s}");
            let out = manual::run(&mut pacer, &mut motor, &handle, &rx, &clock, &mut say, &mut save, &|| {
                pimax_eye::sigint::stopped() || gone.load(Ordering::SeqCst)
            });
            handle.request_exit();
            if out == manual::Outcome::NoLens { 2 } else { 0 }
        })
    };
    hud_loop(&mut hud, &handle, worker, &gone)
}
