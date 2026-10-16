//! The `pimax-eye` command line.
//!
//! ```text
//! pimax-eye pcap   <capture.pcap> [--device N] [--csv FILE] [--head N] [--plan]
//! pimax-eye plan   <capture.pcap> [--device N]
//! pimax-eye probe  [--usb-device PATH] [--claim]
//! pimax-eye stream [--capture FILE [--capture-device N] [--allow-incomplete] [--skip-op OPS] [--force-replay] [--dry-run]]
//!                  [--seconds S] [--print | --print-all] [--stop] [--usb-device PATH]
//! ```
//! Exit codes: 0 ok, 1 usage, 2 USB/device error, 3 replay refused (capture incomplete) or failed,
//! 4 no gaze sample received, 5 samples received but some failed to decode.

use crate::analysis::{
    analyze_file, collect_samples, format_report, head_lines, write_csv_row, CSV_HEADER,
};
use crate::gaze::{format_line, EyeState, GazeSample};
use crate::replay::{StepKind, EP_BULK_IN, EP_BULK_OUT};
use crate::session::{
    in_decoder, read_stream, run_plan, ReplayError, ReplayOptions, StreamSummary,
    CAPTURED_UNSUBSCRIBE_GAZE, IN_BUFFER,
};
use crate::transport::Transport;
use crate::ttp::{Kind, STREAM_GAZE};
use crate::usbfs::{find_eye_chips, SysDevice, UsbFs};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const USAGE: &str = "\
pimax-eye: Tobii EyeChip (2104:0561) client for the Pimax Dream Air

  pimax-eye pcap   <capture.pcap> [--device N] [--csv FILE] [--head N] [--plan]
        decode a Windows USBPcap capture: TTP messages, gaze stream statistics, replay plan completeness
  pimax-eye plan   <capture.pcap> [--device N]
        list the exact transfers of the Windows session start and which ones the capture cuts
  pimax-eye probe  [--usb-device PATH] [--claim]
        find the EyeChip in sysfs and check that the usbfs node can be opened (sends nothing)
  pimax-eye replay [--skip-auth] [--skip-cal] [--skip-vendor-write] [--dry-run] [--seconds S] [--force-replay]
                   [--raw-log FILE] [--usb-device PATH]
        send the 26-step cold-start list data/replay-frames.txt (bytes captured from Windows, nothing else) and
        listen for gaze. Calibration ops 0x3f2/0x407/0x42e/0x3fc/0x44c are refused in code.
  pimax-eye live   [--with-cal] [--skip-auth] [--seconds S] [--print-hz F] [--usb-device PATH]
        the Monado-side session (pimax_eye::live::Tracker): init, subscribe, print combined gaze in the OpenXR head
        frame (metres, -z forward; PIMAX_EYE_FLIP_X/Y/Z, PIMAX_EYE_OFFSET_MM, PIMAX_EYE_SWAP), unsubscribe on exit,
        reconnect if the chip re-enumerates. --with-cal also sends the captured calibration blob (0x456).
  pimax-eye watch  [--with-cal] [--skip-auth] [--seconds S] [--hz F] [--log FILE.csv] [--usb-device PATH]
        full-screen live view of both eye cameras' reports (same Tracker/init as `live`): pupil position in the
        sensor area with trail, pupil size, valid %, origin x/z with BAD flag (>2.5 mm from running median), BAD % of
        the last 5 s. --log writes every sample per eye as CSV. Needs the eye chip free (stop desk/VR mode first).
  pimax-eye ipd --read | --set MM
        no eye tracker: --read prints the live lens IPD (headset info report); --set sends the captured set-IPD
        command once (clamped to 58-72 mm), refused while monado-service runs, waits for the lens to arrive (about
        1.8 mm/s) and writes ~/.config/omarchyvr/ipd.env. This is what the app's Optics page Apply uses.
  pimax-eye ipd [--seconds S] [--apply [--mm X] | --watch] [--usb-device PATH]
        measure the IPD for S s (default 5) from chip field 34 and the distance between the two gaze origins
        (samples >2.5 mm from the per-eye median origin dropped) and print both medians, the sample counts and the
        current lens IPD (headset info report). Prints only, unless --apply: then send the captured set-IPD
        command `58 06 5c 05 01 <lo> <hi>` (u16 LE = mm x 100) ONCE for X (or the measurement), clamped to 58-72 mm,
        and poll the lens IPD for 5 s (success within 0.5 mm). Refuses if the two measurements differ by >3 mm or
        <100 good samples (unless --mm). Exits only if monado-service has the eye device open (desk mode without --eye is fine) or
        the hidraw/eye node cannot be opened. Monado hook: OXR_DEBUG_IPD_MM=<mm>.
        --watch: live full-screen view (10 Hz) of eyes IPD (field 34 + gated origin distance, last 3 s) vs the live
        lens IPD on a 58-72 mm scale with the difference (green <1, yellow 1-3, red >3 mm) and per-eye good %.
        Keys: a apply measured (same checks as --apply), + / - target +-0.5 mm sent after a 1 s pause, q quit; at
        most one motor command per second; ~/.config/omarchyvr/ipd.env is written on quit if a value was applied.
  pimax-eye power-on [--wait S]
        if the headset (34a4:0044) is enumerated and the EyeChip is not, send the captured eye-module enable
        `58 05 3c 05 00 01` once on the headset command interface (what Windows sends; the module powers up and
        enumerates ~1.6 s later) and wait up to S s (default 15) for 2104:0561. Sends nothing if the chip is there.
  pimax-eye stream [--capture FILE] [--capture-device N] [--allow-incomplete] [--skip-op OPS] [--force-replay]
                   [--dry-run] [--seconds S] [--print | --print-all] [--stop] [--usb-device PATH]
        replay the session start from a capture (only bytes found in the capture are sent), then read EP 0x83
        and decode the gaze stream for S seconds (default 10).
        --allow-incomplete  skip the steps the capture cuts (instead of refusing the whole replay)
        --skip-op 0x76c,..  leave the listed TTP opcodes out of the replay (experiments, e.g. without authentication)
        --force-replay      replay even when the chip is already streaming
        --stop              send the captured unsubscribe at the end (only after a replay by this run)
        --dry-run           print the plan and exit without opening the device

Run device commands under the headset lock:  flock -w 3600 /tmp/omarchyvr-headset.lock pimax-eye stream ...
";

struct Args {
    items: Vec<String>,
}

impl Args {
    fn flag(&mut self, name: &str) -> bool {
        match self.items.iter().position(|a| a == name) {
            Some(i) => {
                self.items.remove(i);
                true
            }
            None => false,
        }
    }

    fn value(&mut self, name: &str) -> Result<Option<String>, String> {
        if let Some(i) = self.items.iter().position(|a| a == name) {
            if i + 1 >= self.items.len() {
                return Err(format!("{name} needs a value"));
            }
            let v = self.items.remove(i + 1);
            self.items.remove(i);
            return Ok(Some(v));
        }
        let prefix = format!("{name}=");
        if let Some(i) = self.items.iter().position(|a| a.starts_with(&prefix)) {
            let v = self.items.remove(i)[prefix.len()..].to_string();
            return Ok(Some(v));
        }
        Ok(None)
    }

    fn number<T: std::str::FromStr>(&mut self, name: &str) -> Result<Option<T>, String> {
        self.value(name)?
            .map(|v| {
                v.parse::<T>()
                    .map_err(|_| format!("{name}: bad number '{v}'"))
            })
            .transpose()
    }

    fn positional(&mut self) -> Option<String> {
        let i = self.items.iter().position(|a| !a.starts_with("--"))?;
        Some(self.items.remove(i))
    }

    fn finish(&self) -> Result<(), String> {
        match self.items.first() {
            None => Ok(()),
            Some(a) => Err(format!("unexpected argument '{a}'")),
        }
    }
}

/// Run the tool; returns the process exit code.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(cmd) = args.first() else {
        let _ = write!(err, "{USAGE}");
        return 1;
    };
    let mut a = Args {
        items: args[1..].to_vec(),
    };
    let r = match cmd.as_str() {
        "pcap" => cmd_pcap(&mut a, out),
        "plan" => cmd_plan(&mut a, out),
        "probe" => cmd_probe(&mut a, out),
        "stream" => cmd_stream(&mut a, out, err),
        "replay" => cmd_replay(&mut a, out),
        "live" => cmd_live(&mut a, out),
        "watch" => cmd_watch(&mut a, out),
        "power-on" => cmd_power_on(&mut a, out),
        "ipd" => cmd_ipd(&mut a, out),
        "help" | "--help" | "-h" => {
            let _ = write!(out, "{USAGE}");
            return 0;
        }
        other => Err(Fail::usage(format!("unknown command '{other}'"))),
    };
    match r {
        Ok(code) => code,
        Err(f) => {
            let _ = writeln!(err, "pimax-eye: {}", f.msg);
            if f.code == 1 {
                let _ = write!(err, "\n{USAGE}");
            }
            f.code
        }
    }
}

struct Fail {
    code: i32,
    msg: String,
}

impl Fail {
    fn usage(msg: String) -> Fail {
        Fail { code: 1, msg }
    }

    fn with(code: i32, msg: impl Into<String>) -> Fail {
        Fail {
            code,
            msg: msg.into(),
        }
    }
}

impl From<String> for Fail {
    fn from(msg: String) -> Fail {
        Fail::usage(msg)
    }
}

impl From<&str> for Fail {
    fn from(msg: &str) -> Fail {
        Fail::usage(msg.to_string())
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Fail {
        Fail::with(2, e.to_string())
    }
}

impl From<crate::pcap::PcapError> for Fail {
    fn from(e: crate::pcap::PcapError) -> Fail {
        Fail::with(2, e.to_string())
    }
}

fn parse_opcodes(list: &str) -> Result<Vec<u32>, String> {
    list.split(',')
        .map(|t| {
            let t = t.trim();
            match t.strip_prefix("0x") {
                Some(h) => u32::from_str_radix(h, 16),
                None => t.parse::<u32>(),
            }
            .map_err(|_| format!("--skip-op: bad opcode '{t}'"))
        })
        .collect()
}

fn cmd_pcap(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    let device = a.number::<u16>("--device")?;
    let csv = a.value("--csv")?;
    let head = a.number::<usize>("--head")?;
    let verbose = a.flag("--plan");
    let file = a.positional().ok_or("pcap needs a capture file")?;
    a.finish()?;
    let report = analyze_file(Path::new(&file), device)?;
    write!(out, "{}", format_report(&report, verbose))?;
    for d in &report.devices {
        if csv.is_none() && head.is_none() {
            break;
        }
        let samples = collect_samples(Path::new(&file), d.device)?;
        if let Some(n) = head {
            writeln!(out, "\nfirst {n} samples of device {}:", d.device)?;
            write!(out, "{}", head_lines(&samples, n))?;
        }
        if let Some(path) = &csv {
            let path = if report.devices.len() > 1 {
                format!("{path}.dev{}", d.device)
            } else {
                path.clone()
            };
            let mut w = BufWriter::new(File::create(&path)?);
            writeln!(w, "{CSV_HEADER}")?;
            for s in &samples {
                write_csv_row(&mut w, s)?;
            }
            w.flush()?;
            writeln!(out, "wrote {} rows to {path}", samples.len())?;
        }
    }
    Ok(0)
}

fn cmd_plan(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    let device = a.number::<u16>("--device")?;
    let file = a.positional().ok_or("plan needs a capture file")?;
    a.finish()?;
    let report = analyze_file(Path::new(&file), device)?;
    if report.devices.is_empty() {
        return Err(Fail::with(2, "no device with TTP traffic in the capture"));
    }
    for d in &report.devices {
        write!(out, "{}", d.plan.describe())?;
        writeln!(
            out,
            "=> {} ({} bytes missing)",
            if d.plan.is_complete() {
                "complete: every byte is in the capture"
            } else {
                "NOT replayable verbatim"
            },
            d.plan.missing_bytes()
        )?;
    }
    Ok(0)
}

fn pick_chip(usb_device: Option<String>) -> Result<(PathBuf, Option<SysDevice>), Fail> {
    if let Some(p) = usb_device {
        return Ok((PathBuf::from(p), None));
    }
    let mut chips = find_eye_chips();
    match chips.len() {
        0 => Err(Fail::with(2, "no EyeChip (2104:0561) on the USB bus")),
        1 => {
            let c = chips.remove(0);
            Ok((c.devnode.clone(), Some(c)))
        }
        n => Err(Fail::with(
            2,
            format!("{n} EyeChips found, choose one with --usb-device /dev/bus/usb/BBB/DDD"),
        )),
    }
}

fn describe_chip(c: &SysDevice) -> String {
    format!(
        "{:04x}:{:04x} {} serial {} bus {} dev {} {} Mb/s, node {}, interface 0 driver: {}",
        c.vid,
        c.pid,
        c.product.as_deref().unwrap_or("?"),
        c.serial.as_deref().unwrap_or("?"),
        c.bus,
        c.devnum,
        c.speed.as_deref().unwrap_or("?"),
        c.devnode.display(),
        c.interface_driver().unwrap_or_else(|| "none".into())
    )
}

fn cmd_probe(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    let usb_device = a.value("--usb-device")?;
    let claim = a.flag("--claim");
    a.finish()?;
    let (node, chip) = pick_chip(usb_device)?;
    if let Some(c) = &chip {
        writeln!(out, "{}", describe_chip(c))?;
    }
    match UsbFs::open(&node) {
        Ok(mut dev) => {
            writeln!(out, "open {}: ok (read/write)", node.display())?;
            if claim {
                dev.claim_interface(0)?;
                writeln!(
                    out,
                    "claim interface 0: ok (released on exit; nothing was sent)"
                )?;
            }
            Ok(0)
        }
        Err(e) => {
            writeln!(out, "open {}: FAILED: {e}", node.display())?;
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                writeln!(
                    out,
                    "install the udev rule (needs sudo): sudo cp linux/70-pimax.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger"
                )?;
            }
            Ok(2)
        }
    }
}

/// Per-eye min/max of the gaze angles, to see at a glance that the values move.
#[derive(Default, Clone)]
struct Ranges {
    n: u64,
    yaw: (f64, f64),
    pitch: (f64, f64),
    pupil: (f64, f64),
}

impl Ranges {
    fn add(&mut self, yaw: f64, pitch: f64, pupil: f64) {
        if self.n == 0 {
            self.yaw = (yaw, yaw);
            self.pitch = (pitch, pitch);
            self.pupil = (pupil, pupil);
        } else {
            self.yaw = (self.yaw.0.min(yaw), self.yaw.1.max(yaw));
            self.pitch = (self.pitch.0.min(pitch), self.pitch.1.max(pitch));
            self.pupil = (self.pupil.0.min(pupil), self.pupil.1.max(pupil));
        }
        self.n += 1;
    }
}

fn cmd_stream(a: &mut Args, out: &mut dyn Write, err: &mut dyn Write) -> Result<i32, Fail> {
    let seconds = a.number::<f64>("--seconds")?.unwrap_or(10.0);
    let capture = a.value("--capture")?;
    let cap_dev = a.number::<u16>("--capture-device")?;
    let usb_device = a.value("--usb-device")?;
    let print_all = a.flag("--print-all");
    let print = a.flag("--print") || print_all;
    let allow_incomplete = a.flag("--allow-incomplete");
    let skip_ops = a
        .value("--skip-op")?
        .map(|v| parse_opcodes(&v))
        .transpose()?
        .unwrap_or_default();
    let force_replay = a.flag("--force-replay");
    let dry_run = a.flag("--dry-run");
    let stop = a.flag("--stop");
    a.finish()?;
    if !(0.1..=3600.0).contains(&seconds) {
        return Err("--seconds must be between 0.1 and 3600".to_string().into());
    }
    let opts = ReplayOptions {
        allow_incomplete,
        ..ReplayOptions::default()
    };

    // 1. the plan, before the device is touched
    let plan = match &capture {
        None => None,
        Some(file) => {
            let report = analyze_file(Path::new(file), cap_dev)?;
            let mut devs: Vec<_> = report.devices.into_iter().collect();
            if devs.len() > 1 {
                devs.sort_by_key(|d| {
                    std::cmp::Reverse((d.plan.stream_start.is_some(), d.plan.steps.len()))
                });
                writeln!(
                    err,
                    "capture has {} TTP devices, using address {} (override with --capture-device)",
                    devs.len(),
                    devs[0].device
                )?;
            }
            let d = devs
                .into_iter()
                .next()
                .ok_or_else(|| Fail::with(3, "no device with TTP traffic in the capture"))?;
            let mut plan = d.plan;
            if !skip_ops.is_empty() {
                let before = plan.steps.len();
                plan.steps.retain(|st| {
                    !matches!(&st.kind, StepKind::Request { opcode, .. } if skip_ops.contains(opcode))
                });
                writeln!(
                    out,
                    "--skip-op: leaving {} step(s) out of the replay",
                    before - plan.steps.len()
                )?;
            }
            write!(out, "{}", plan.describe())?;
            let d = crate::analysis::DeviceReport { plan, ..d };
            if d.plan.auth_steps().count() > 0 {
                writeln!(
                    out,
                    "note: the plan contains the authentication exchange; the device may refuse the recorded answers"
                )?;
            }
            Some(d.plan)
        }
    };
    if let Some(p) = &plan {
        if p.steps.is_empty() {
            return Err(Fail::with(3, ReplayError::EmptyPlan.to_string()));
        }
        if !p.is_complete() && !allow_incomplete {
            let e = ReplayError::Incomplete(
                p.steps
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| !s.is_complete())
                    .map(|(i, s)| crate::session::IncompleteStep {
                        step: i,
                        what: s.label(),
                        missing: s.missing_bytes,
                        total: s.total_bytes,
                    })
                    .collect(),
            );
            return Err(Fail::with(
                3,
                format!(
                    "{e}\nnothing was sent; the USB device was not opened. Record the Windows session start again with a full snap length (capture.ps1 -Snaplen 65535) or pass --allow-incomplete to skip those steps"
                ),
            ));
        }
    }
    if dry_run {
        writeln!(out, "dry run: device not opened")?;
        return Ok(0);
    }

    // 2. the device
    let (node, chip) = pick_chip(usb_device)?;
    if let Some(c) = &chip {
        writeln!(out, "device: {}", describe_chip(c))?;
    }
    let mut dev = UsbFs::open(&node).map_err(|e| {
        Fail::with(
            2,
            format!(
                "cannot open {}: {e}{}",
                node.display(),
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    " (install linux/70-pimax.rules with sudo, see `pimax-eye probe`)"
                } else {
                    ""
                }
            ),
        )
    })?;
    dev.claim_interface(0).map_err(|e| {
        Fail::with(
            2,
            format!("cannot claim interface 0: {e} (a kernel driver is bound or another program has it; nothing is detached)"),
        )
    })?;
    let mut dec = in_decoder();

    // 3. is a session already streaming?
    let already = probe_stream(&mut dev, &mut dec, Duration::from_millis(300))?;
    let mut replayed = false;
    if already > 0 && !force_replay {
        writeln!(out, "the chip is already streaming ({already} gaze notification(s) in 300 ms): attaching, nothing is sent")?;
    } else if let Some(p) = &plan {
        writeln!(
            out,
            "replaying {} steps from the capture ...",
            p.steps.len()
        )?;
        let rep = run_plan(&mut dev, p, &opts, &mut dec, |l| {
            let _ = writeln!(err, "  {l}");
        });
        match rep {
            Ok(r) => {
                replayed = true;
                writeln!(
                    out,
                    "replay done: {} steps sent, {} skipped, {} TTP exchanges, {} notifications already arrived",
                    r.sent,
                    r.skipped.len(),
                    r.exchanges.len(),
                    r.notifications
                )?;
            }
            Err(e) => return Err(Fail::with(3, format!("replay stopped: {e}"))),
        }
    } else {
        writeln!(
            out,
            "no --capture: passive mode, nothing is sent. The chip only streams after a TTP session was started."
        )?;
    }

    // 4. read
    let mut ranges = [Ranges::default(), Ranges::default()];
    let mut last_print = Duration::ZERO;
    let mut printed_any = false;
    let summary = {
        let ranges = &mut ranges;
        let out_ref = &mut *out;
        read_stream(
            &mut dev,
            &mut dec,
            Duration::from_secs_f64(seconds),
            |s: &GazeSample, at| {
                for (i, e) in s.eyes.iter().enumerate() {
                    if let (EyeState::Tracked, Some((yaw, pitch))) = (e.state(), e.yaw_pitch_deg())
                    {
                        ranges[i].add(yaw, pitch, e.pupil_diameter_mm);
                    }
                }
                if print
                    && (print_all
                        || !printed_any
                        || at.saturating_sub(last_print) >= Duration::from_millis(100))
                {
                    let _ = writeln!(out_ref, "{:>7.3}s {}", at.as_secs_f64(), format_line(s));
                    last_print = at;
                    printed_any = true;
                }
            },
        )?
    };
    print_summary(out, &summary, &ranges)?;

    if stop && replayed {
        writeln!(
            out,
            "sending the captured unsubscribe (0x4ce, stream 0x0505) ..."
        )?;
        dev.bulk_out(
            EP_BULK_OUT,
            &CAPTURED_UNSUBSCRIBE_GAZE,
            Duration::from_secs(2),
        )?;
        let mut stop_dec = in_decoder();
        let mut got = false;
        for _ in 0..40 {
            if let Some(b) = dev.bulk_in(EP_BULK_IN, IN_BUFFER, Duration::from_millis(50))? {
                got |= stop_dec
                    .push(&b)
                    .into_iter()
                    .flatten()
                    .any(|m| m.header.kind == Kind::Response && m.header.id == 0x4ce);
                if got {
                    break;
                }
            }
        }
        writeln!(out, "unsubscribe answered: {got}")?;
    } else if replayed {
        writeln!(out, "the stream is left running on the chip (use --stop to unsubscribe); the next run attaches to it")?;
    }

    Ok(if summary.samples == 0 {
        4
    } else if summary.parse_errors > 0 {
        5
    } else {
        0
    })
}

/// Read for `window` and count the gaze notifications already flowing.
fn probe_stream<T: Transport>(
    t: &mut T,
    dec: &mut crate::ttp::StreamDecoder,
    window: Duration,
) -> Result<u64, Fail> {
    let start = std::time::Instant::now();
    let mut n = 0;
    while start.elapsed() < window {
        let wait = window
            .saturating_sub(start.elapsed())
            .max(Duration::from_millis(1));
        if let Some(b) = t.bulk_in(EP_BULK_IN, IN_BUFFER, wait)? {
            n += dec
                .push(&b)
                .into_iter()
                .flatten()
                .filter(|m| m.header.kind == Kind::Notification && m.header.id == STREAM_GAZE)
                .count() as u64;
        }
    }
    Ok(n)
}

fn print_summary(
    out: &mut dyn Write,
    s: &StreamSummary,
    ranges: &[Ranges; 2],
) -> std::io::Result<()> {
    writeln!(
        out,
        "\n--- stream summary ({:.2} s) ---",
        s.elapsed.as_secs_f64()
    )?;
    writeln!(
        out,
        "USB: {} bytes in {} transfers, {} read timeouts, {} framing errors, {} other TTP messages",
        s.bytes, s.transfers, s.timeouts, s.framing_errors, s.other_messages
    )?;
    writeln!(
        out,
        "samples: {} decoded, {} undecodable{}",
        s.samples,
        s.parse_errors,
        s.first_parse_error
            .as_ref()
            .map_or(String::new(), |e| format!(" (first: {e})"))
    )?;
    if s.samples > 0 {
        writeln!(
            out,
            "first sample after {:.0} ms | rate {} by host clock, {} by device timestamps | counter gaps {}, timestamp gaps {}, regressions {}",
            s.first_sample_after.map_or(0.0, |d| d.as_secs_f64() * 1000.0),
            s.host_rate_hz().map_or("-".into(), |x| format!("{x:.2} Hz")),
            s.stats.device_rate_hz().map_or("-".into(), |x| format!("{x:.2} Hz")),
            s.stats.counter_gaps,
            s.stats.timestamp_gaps,
            s.stats.timestamp_regressions
        )?;
        for (i, name) in ["A", "B"].iter().enumerate() {
            let st = s.stats.states[i];
            let r = &ranges[i];
            write!(
                out,
                "eye {name}: lost {} | origin only {} | tracked {}",
                st[0], st[1], st[2]
            )?;
            if r.n > 0 {
                write!(
                    out,
                    " | yaw {:+.1}..{:+.1} deg, pitch {:+.1}..{:+.1} deg, pupil {:.2}..{:.2} mm",
                    r.yaw.0, r.yaw.1, r.pitch.0, r.pitch.1, r.pupil.0, r.pupil.1
                )?;
            }
            writeln!(out)?;
        }
    } else {
        writeln!(out, "no gaze sample arrived: the stream did not start")?;
    }
    Ok(())
}

fn cmd_live(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    use crate::live::{Config, State, Tracker};
    let mut cfg = Config::from_env();
    cfg.with_cal |= a.flag("--with-cal");
    cfg.skip_auth |= a.flag("--skip-auth");
    let seconds = a.number::<f64>("--seconds")?.unwrap_or(10.0);
    let hz = a.number::<f64>("--print-hz")?.unwrap_or(10.0).clamp(0.1, 120.0);
    if let Some(p) = a.value("--usb-device")? {
        cfg.usb_device = Some(PathBuf::from(p));
    }
    a.finish()?;
    writeln!(
        out,
        "conversion: {:?}; calibration blob: {}",
        cfg.conv,
        if cfg.with_cal { "sent" } else { "not sent" }
    )?;
    crate::sigint::install();
    let tr = Tracker::start(cfg);
    let t0 = std::time::Instant::now();
    let mut last_seq = None;
    let mut printed = 0u64;
    while t0.elapsed().as_secs_f64() < seconds && !crate::sigint::stopped() {
        std::thread::sleep(Duration::from_secs_f64(1.0 / hz));
        let st = tr.state();
        match tr.latest() {
            Some(s) if st == State::Streaming && last_seq != Some(s.seq) => {
                last_seq = Some(s.seq);
                printed += 1;
                let f = |v: [f32; 3]| format!("({:+.4},{:+.4},{:+.4})", v[0], v[1], v[2]);
                match s.gaze {
                    Some(g) => writeln!(
                        out,
                        "[{:6.2}] #{} dev={}us gaze origin={} dir={} eyes={:02b} L-pupil={:.2} R-pupil={:.2}",
                        t0.elapsed().as_secs_f64(), s.seq, s.device_ts_us, f(g.origin), f(g.direction), g.eyes,
                        s.eyes[0].pupil_mm, s.eyes[1].pupil_mm
                    )?,
                    None => writeln!(out, "[{:6.2}] #{} no valid eye", t0.elapsed().as_secs_f64(), s.seq)?,
                }
            }
            _ => writeln!(out, "[{:6.2}] state {:?}", t0.elapsed().as_secs_f64(), st)?,
        }
    }
    let stats = tr.stats();
    tr.stop();
    writeln!(out, "stopped: {} samples, {} reconnects, {printed} lines", stats.samples, stats.reconnects)?;
    Ok(if stats.samples > 0 { 0 } else { 4 })
}

fn cmd_watch(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    use crate::live::{Config, State, Tracker};
    use crate::watch::{Watch, LOG_HEADER};
    let mut cfg = Config::from_env();
    cfg.with_cal |= a.flag("--with-cal");
    cfg.skip_auth |= a.flag("--skip-auth");
    let seconds = a.number::<f64>("--seconds")?.unwrap_or(f64::INFINITY);
    let hz = a.number::<f64>("--hz")?.unwrap_or(10.0).clamp(0.5, 60.0);
    let log_path = a.value("--log")?;
    if let Some(p) = a.value("--usb-device")? {
        cfg.usb_device = Some(PathBuf::from(p));
    }
    a.finish()?;
    let mut log = match &log_path {
        Some(p) => {
            let mut f = BufWriter::new(File::create(p)?);
            writeln!(f, "{LOG_HEADER}")?;
            Some(f)
        }
        None => None,
    };
    crate::sigint::install();
    let tr = Tracker::start(cfg);
    let mut w = Watch::default();
    let (mut last_seq, mut t0_us) = (None, None);
    let t0 = std::time::Instant::now();
    write!(out, "\x1b[?1049h\x1b[?25l\x1b[2J")?;
    while t0.elapsed().as_secs_f64() < seconds && !crate::sigint::stopped() {
        std::thread::sleep(Duration::from_secs_f64(1.0 / hz));
        let mut rows = String::new();
        for s in tr.samples_since(last_seq) {
            last_seq = Some(s.seq);
            let base = *t0_us.get_or_insert(s.device_ts_us);
            w.push(&s, base, log.as_ref().map(|_| &mut rows));
        }
        if let Some(f) = log.as_mut() {
            f.write_all(rows.as_bytes())?;
        }
        let st = tr.state();
        let label = if st == State::Streaming { "streaming".to_string() } else { format!("{st:?}") };
        write!(out, "\x1b[H{}\x1b[J", w.render(&label))?;
        out.flush()?;
    }
    write!(out, "\x1b[?25h\x1b[?1049l")?;
    if let Some(mut f) = log {
        f.flush()?;
    }
    let stats = tr.stats();
    tr.stop();
    writeln!(out, "stopped: {} samples, {} reconnects", stats.samples, stats.reconnects)?;
    Ok(if stats.samples > 0 { 0 } else { 4 })
}

/// `pimax-eye ipd --read` / `--set MM`: the lens IPD of the headset without the eye tracker (what the app's Optics
/// page uses). `--read` prints the live lens IPD from the info report. `--set MM` sends the captured set-IPD command
/// once (clamped to 58-72 mm), refuses while a VR session runs (monado-service), polls the lens until it arrives (the
/// motor moves about 1.8 mm/s) and writes ~/.config/omarchyvr/ipd.env. Exit 0 ok, 2 refused/failed, 3 lens did not arrive.
fn cmd_ipd_headless(set: Option<f32>, out: &mut dyn Write) -> Result<i32, Fail> {
    use crate::ipd;
    let (hc, dev) = (Path::new("/sys/class/hidraw"), Path::new("/dev"));
    let info = ipd::headset_node(hc, dev, "02");
    let Some(target) = set else {
        let Some(info) = info else {
            return Err(Fail::with(2, "headset not found (no hidraw node for interface 2 of 34a4:0044)"));
        };
        return match ipd::read_lens_ipd(&info, Duration::from_millis(1500)) {
            Some(v) => {
                writeln!(out, "lens IPD: {v:.2} mm")?;
                Ok(0)
            }
            None => Err(Fail::with(3, "no info report received from the headset")),
        };
    };
    if !target.is_finite() {
        return Err(Fail::with(2, "--set is not a finite number; nothing sent"));
    }
    if let Some(pid) = ipd::session_running(Path::new("/proc")) {
        return Err(Fail::with(2, format!("monado-service (pid {pid}) is running: stop VR first, the lenses are set only while no session owns the headset; nothing sent")));
    }
    let Some(node) = ipd::headset_node(hc, dev, "00") else {
        return Err(Fail::with(2, "headset not found (no hidraw node for interface 0 of 34a4:0044): nothing sent"));
    };
    let mm = ipd::clamp_mm(target);
    if (mm - target).abs() > 1e-6 {
        writeln!(out, "note: {target:.2} mm clamped to {mm:.2} mm (allowed {}-{} mm)", ipd::MIN_MM, ipd::MAX_MM)?;
    }
    let lens0 = info.as_ref().and_then(|n| ipd::read_lens_ipd(n, Duration::from_millis(500)));
    if let Some(v) = lens0 {
        writeln!(out, "lens IPD now: {v:.2} mm")?;
    }
    let rep = ipd::motor_report(mm);
    ipd::send_motor(&node, mm).map_err(|e| Fail::with(2, format!("{}: {e}", node.display())))?;
    writeln!(out, "sent once: {} -> lens IPD {mm:.2} mm", rep[..7].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "))?;
    let Some(info) = info else {
        writeln!(out, "no info interface (hidraw IF2): cannot verify the lens")?;
        return Ok(3);
    };
    // the motor runs about 1.8 mm/s: the whole range takes under 8 s
    let t1 = std::time::Instant::now();
    let mut fin = None;
    while t1.elapsed().as_secs_f64() < 10.0 {
        if let Some(v) = ipd::read_lens_ipd(&info, Duration::from_millis(500)) {
            fin = Some(v);
            if ipd::arrived(v, mm) {
                break;
            }
        }
    }
    let ok = fin.is_some_and(|v| ipd::arrived(v, mm));
    match fin {
        Some(v) => writeln!(out, "final lens IPD: {v:.2} mm ({})", if ok { "OK" } else { "NOT within 0.5 mm of target" })?,
        None => writeln!(out, "final lens IPD: no info report received")?,
    }
    if ok {
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home).join(".config/omarchyvr/ipd.env");
            if std::fs::create_dir_all(p.parent().unwrap_or(Path::new("."))).is_ok() && std::fs::write(&p, format!("{}\n", ipd::monado_line(mm))).is_ok() {
                writeln!(out, "wrote {} (the next start renders with it)", p.display())?;
            }
        }
    }
    Ok(if ok { 0 } else { 3 })
}

fn cmd_ipd(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    use crate::ipd::{self, Measure};
    use crate::live::{Config, State, Tracker};
    let seconds = a.number::<f64>("--seconds")?.unwrap_or(5.0).clamp(1.0, 120.0);
    let watch = a.flag("--watch");
    let apply = a.flag("--apply");
    let mm_arg = a.number::<f32>("--mm")?;
    let set_arg = a.number::<f32>("--set")?;
    let read = a.flag("--read");
    let mut cfg = Config::from_env();
    if let Some(p) = a.value("--usb-device")? {
        cfg.usb_device = Some(PathBuf::from(p));
    }
    a.finish()?;
    if read || set_arg.is_some() {
        if apply || watch || mm_arg.is_some() || (read && set_arg.is_some()) {
            return Err("--read and --set MM stand alone (no eye tracker is used); do not combine them with the other options".into());
        }
        return cmd_ipd_headless(set_arg, out);
    }
    if mm_arg.is_some() && !apply {
        return Err("--mm only makes sense with --apply".into());
    }
    if watch && (apply || mm_arg.is_some()) {
        return Err("--watch has its own keys (a applies); do not combine with --apply/--mm".into());
    }
    let eye_node = cfg.usb_device.clone().or_else(|| crate::usbfs::find_eye_chips().first().map(|d| d.devnode.clone()));
    match ipd::busy_check(Path::new("/proc"), eye_node.as_deref()) {
        ipd::Busy::Refuse(m) => return Err(Fail::with(2, m)),
        ipd::Busy::Warn(m) => writeln!(out, "warning: {m}")?,
        ipd::Busy::Free => {}
    }
    let (hc, dev) = (Path::new("/sys/class/hidraw"), Path::new("/dev"));
    let info = ipd::headset_node(hc, dev, "02");
    let cmd = ipd::headset_node(hc, dev, "00");
    if apply && cmd.is_none() {
        return Err(Fail::with(2, "no hidraw node for interface 0 of 34a4:0044: nothing sent"));
    }
    if watch {
        let Some(node) = cmd else {
            return Err(Fail::with(2, "no hidraw node for interface 0 of 34a4:0044: cannot drive the lenses"));
        };
        std::fs::OpenOptions::new().write(true).open(&node).map_err(|e| Fail::with(2, format!("cannot open {} for writing: {e}", node.display())))?;
        if let Some(n) = &info {
            std::fs::File::open(n).map_err(|e| Fail::with(2, format!("cannot open {}: {e}", n.display())))?;
        }
        return crate::ipd_watch::run(cfg, info, node, out).map_err(|e| Fail::with(2, e));
    }
    let lens0 = info.as_ref().and_then(|n| ipd::read_lens_ipd(n, Duration::from_millis(500)));
    crate::sigint::install();
    let tr = Tracker::start(cfg);
    let wait0 = std::time::Instant::now();
    while tr.state() != State::Streaming {
        if wait0.elapsed().as_secs() >= 15 || crate::sigint::stopped() {
            tr.stop();
            return Err(Fail::with(
                2,
                "no gaze stream after 15 s (eye chip absent or busy: stop monado-service / other pimax-eye, or run `pimax-eye power-on`); nothing sent",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut me = Measure::default();
    let (mut last_seq, t0) = (None, std::time::Instant::now());
    while t0.elapsed().as_secs_f64() < seconds && !crate::sigint::stopped() {
        std::thread::sleep(Duration::from_millis(50));
        for s in tr.samples_since(last_seq) {
            last_seq = Some(s.seq);
            me.push(&s);
        }
    }
    tr.stop();
    let m = me.result();
    writeln!(
        out,
        "samples: {} seen, {} valid, {} good (origin within {} mm of the per-eye median)",
        m.seen,
        m.valid,
        m.good,
        ipd::REJECT_MM
    )?;
    writeln!(out, "IPD field 34:        {:.2} mm", m.field34_mm)?;
    writeln!(out, "IPD origin distance: {:.2} mm", m.origin_mm)?;
    match lens0 {
        Some(v) => writeln!(out, "lens IPD (info report): {v:.2} mm")?,
        None => writeln!(out, "lens IPD (info report): unavailable")?,
    }
    if !apply {
        writeln!(out, "nothing sent (no --apply)")?;
        return Ok(0);
    }
    let (target, note) = ipd::decide(&m, mm_arg, lens0).map_err(|e| Fail::with(2, format!("{e}; nothing sent")))?;
    if let Some(n) = note {
        writeln!(out, "note: {n}")?;
    }
    let node = cmd.unwrap_or_default();
    let rep = ipd::motor_report(target);
    ipd::send_motor(&node, target).map_err(|e| Fail::with(2, format!("{}: {e}", node.display())))?;
    writeln!(
        out,
        "sent once to {}: {} -> lens IPD {target:.2} mm",
        node.display(),
        rep[..7].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
    )?;
    let Some(info) = info else {
        writeln!(out, "no info interface (hidraw IF2): cannot verify the lens")?;
        return Ok(3);
    };
    let t1 = std::time::Instant::now();
    let mut fin = None;
    while t1.elapsed().as_secs_f64() < 5.0 {
        if let Some(v) = ipd::read_lens_ipd(&info, Duration::from_millis(500)) {
            fin = Some(v);
            if ipd::arrived(v, target) {
                break;
            }
        }
    }
    let ok = fin.is_some_and(|v| ipd::arrived(v, target));
    match fin {
        Some(v) => writeln!(out, "final lens IPD: {v:.2} mm ({})", if ok { "OK" } else { "NOT within 0.5 mm of target" })?,
        None => writeln!(out, "final lens IPD: no info report received")?,
    }
    let line = ipd::monado_line(fin.filter(|_| ok).unwrap_or(target));
    writeln!(out, "Monado render IPD (read once at session start): {line}")?;
    if ok {
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home).join(".config/omarchyvr/ipd.env");
            if std::fs::create_dir_all(p.parent().unwrap_or(Path::new("."))).is_ok() && std::fs::write(&p, format!("{line}\n")).is_ok() {
                writeln!(out, "wrote {} (source it before monado-service)", p.display())?;
            }
        }
    }
    Ok(if ok { 0 } else { 3 })
}

fn cmd_power_on(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    use crate::power::{eye_chip_present, headset, headset_command_node, send_enable};
    let wait = a.number::<f64>("--wait")?.unwrap_or(15.0).clamp(0.0, 120.0);
    a.finish()?;
    let usb = Path::new("/sys/bus/usb/devices");
    if eye_chip_present(usb) {
        writeln!(out, "EyeChip 2104:0561 is already on the bus: nothing sent")?;
        return Ok(0);
    }
    let Some(h) = headset(usb) else {
        return Err(Fail::with(2, "headset 34a4:0044 is not enumerated: nothing sent"));
    };
    let node = headset_command_node(Path::new("/sys/class/hidraw"), Path::new("/dev"))
        .ok_or_else(|| Fail::with(2, "no hidraw node for interface 0 of 34a4:0044"))?;
    send_enable(&node).map_err(|e| Fail::with(2, format!("{}: {e}", node.display())))?;
    writeln!(
        out,
        "sent the captured eye-module enable (58 05 3c 05 00 01) to {} (headset {} devnum {}); waiting up to {wait:.0} s",
        node.display(),
        h.name,
        h.devnum
    )?;
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f64() < wait {
        if eye_chip_present(usb) {
            writeln!(out, "EyeChip enumerated after {:.1} s", t0.elapsed().as_secs_f64())?;
            return Ok(0);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    writeln!(out, "EyeChip did not enumerate within {wait:.0} s")?;
    Ok(4)
}

fn cmd_replay(a: &mut Args, out: &mut dyn Write) -> Result<i32, Fail> {
    use crate::frames::{parse, select, Action, Skips, DATA};
    use crate::replaycmd::{run_session, Opts};
    let skips = Skips {
        auth: a.flag("--skip-auth"),
        cal: a.flag("--skip-cal"),
        vendor_write: a.flag("--skip-vendor-write"),
    };
    let dry = a.flag("--dry-run");
    let force = a.flag("--force-replay");
    let seconds = a.number::<f64>("--seconds")?.unwrap_or(10.0);
    let raw_log = a.value("--raw-log")?;
    let usb_device = a.value("--usb-device")?;
    a.finish()?;
    let steps = parse(DATA).map_err(|e| Fail::with(3, e))?;
    let sel = select(&steps, skips);
    writeln!(out, "replay plan ({} of {} steps; step 1 enumeration is never sent):", sel.len(), steps.len())?;
    for s in &sel {
        let what = match &s.action {
            Action::Control { setup, data } => format!("CTRL {} data {}", crate::frames::hex(setup), crate::frames::hex(data)),
            Action::Ttp { seq, op, frames } => format!(
                "TTP seq {seq} op {op:#05x} {} ({} URB, {} B)",
                crate::ttp::opcode_name(*op),
                frames.len(),
                frames.iter().map(Vec::len).sum::<usize>()
            ),
            Action::Enumeration => String::new(),
        };
        writeln!(out, "  step {:2} [{:4}] {what}", s.n, s.class)?;
    }
    if dry {
        return Ok(0);
    }
    let (node, chip) = pick_chip(usb_device)?;
    if let Some(c) = &chip {
        writeln!(out, "{}", describe_chip(c))?;
        if let Some(d) = c.interface_driver() {
            return Err(Fail::with(2, format!("kernel driver '{d}' is bound to interface 0; not detaching it, stopping")));
        }
    }
    let mut dev = UsbFs::open(&node)?;
    dev.claim_interface(0)?;
    crate::sigint::install();
    let mut raw = match raw_log {
        Some(p) => Some(BufWriter::new(File::create(p)?)),
        None => None,
    };
    let opts = Opts { skips, seconds, force, timeout: Duration::from_secs(2) };
    let r = run_session(
        &mut dev,
        &steps,
        &opts,
        out,
        raw.as_mut().map(|w| w as &mut dyn Write),
    );
    if let Some(w) = raw.as_mut() {
        let _ = w.flush();
    }
    drop(dev); // releases the interface
    match r {
        Ok(o) => Ok(o.code),
        Err(e) => Err(Fail::with(3, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_args(v: &[&str]) -> (i32, String, String) {
        let args: Vec<String> = v.iter().map(|s| s.to_string()).collect();
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let c = run(&args, &mut o, &mut e);
        (
            c,
            String::from_utf8(o).unwrap(),
            String::from_utf8(e).unwrap(),
        )
    }

    #[test]
    fn usage_and_errors() {
        assert_eq!(run_args(&[]).0, 1);
        assert_eq!(run_args(&["help"]).0, 0);
        let (c, _, e) = run_args(&["bogus"]);
        assert_eq!(c, 1);
        assert!(e.contains("unknown command"));
        let (c, _, e) = run_args(&["stream", "--seconds", "x"]);
        assert_eq!(c, 1);
        assert!(e.contains("bad number"));
        let (c, _, e) = run_args(&["stream", "--seconds"]);
        assert_eq!(c, 1);
        assert!(e.contains("needs a value"));
        let (c, _, e) = run_args(&["stream", "--nope"]);
        assert_eq!(c, 1);
        assert!(e.contains("unexpected argument"));
        assert_eq!(run_args(&["pcap"]).0, 1);
        assert_eq!(run_args(&["pcap", "/nonexistent/file.pcap"]).0, 2);
    }

    #[test]
    fn args_accept_equals_form() {
        let mut a = Args {
            items: vec!["--seconds=3".into(), "file".into(), "--print".into()],
        };
        assert_eq!(a.number::<f64>("--seconds").unwrap(), Some(3.0));
        assert!(a.flag("--print"));
        assert_eq!(a.positional().as_deref(), Some("file"));
        assert!(a.finish().is_ok());
    }
}
