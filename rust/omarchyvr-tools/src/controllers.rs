//! `omarchyvr controllers [seconds] [--raw]`: port of `linux/pimax_controllers.py`.
//! `omarchyvr controllers use when-held|always|off` / `status`: when the running session shows the controllers
//! (PIMAX_CTRL_INHAND, docs/CONTROLLERS-INHAND.md).
//! Sends only the per-slot attach burst (0x16, 0x50, 0x0a, 0x03), re-sends it when a 0x37 heartbeat shows a controller awake
//! but silent, and prints decoded EP 0x82 reports about 10x/s per slot. No wake, no keepalive: runs next to `omarchyvr keepalived`.
use crate::args::Args;
use crate::{hex, CliError, CliResult};
use pimax::{discovery, Config, Event, Session};
use omarchyvr_config::ctrl_inhand::{self, CtrlMode};
use pimax_proto::{parse_reply, ControllerReport, SLOT_LEFT};
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

const STALE_AFTER: f64 = 1.5;
/// Bytes printed explicitly as decoded fields; the diff covers everything else.
fn decoded(i: usize) -> bool {
    i < 4
        || [5, 6, 7].contains(&i)
        || (8..12).contains(&i)
        || (16..28).contains(&i)
        || (30..42).contains(&i)
        || [44, 46, 47, 48, 49].contains(&i)
}
/// 45 blink phase, 28/29 aux ADC, 51 checksum: all change constantly.
fn diff_ignored(i: usize) -> bool {
    [28, 29, 45, 51].contains(&i)
}

fn name(slot: u8) -> &'static str {
    if slot == SLOT_LEFT {
        "L"
    } else {
        "R"
    }
}

/// One-line rendering of a report (the python `print` line without the diff part).
pub fn render(c: &ControllerReport, n: u64, b37: Option<u8>) -> String {
    let t = c.touch_bits();
    let mut btn: Vec<&str> = c.pressed().collect();
    for (on, nm) in [
        (t.a, "touch-A"),
        (t.b, "touch-B"),
        (t.stick, "touch-stick"),
        (t.hand_on, "hand-on"),
    ] {
        if on {
            btn.push(nm);
        }
    }
    let bt = format!(
        "{}%{}{}",
        c.battery,
        if c.charging { "+chg" } else { "" },
        b37.map(|b| format!("/0x37:{b}%")).unwrap_or_default()
    );
    let s = (c.raw_accel, c.raw_gyro);
    format!(
        "{}{} bat={bt} btn=[{}] trig={:3} grip={:3} stick=({:+4},{:+4}) acc=({:+.2},{:+.2},{:+.2})g gyro=({},{},{}) n={n}",
        name(c.slot), c.slot, if btn.is_empty() { "-".to_string() } else { btn.join(",") }, c.trigger, c.grip, c.stick_x, c.stick_y,
        f64::from(s.0[0]) / 1024.0, f64::from(s.0[1]) / 1024.0, f64::from(s.0[2]) / 1024.0, s.1[0], s.1[1], s.1[2]
    )
}

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    if a.pos::<String>(0)?.as_deref() == Some("unpair") {
        let side = a.pos::<String>(1)?.unwrap_or_default();
        let msg = omarchyvr_autod::unpair::unpair_side(&side).map_err(CliError::new)?;
        println!("{msg}");
        return Ok(());
    }
    if a.pos::<String>(0)?.as_deref() == Some("use") {
        // `omarchyvr controllers use when-held|always|off`: change the running session (PIMAX_CTRL_INHAND)
        let v = a.pos::<String>(1)?.unwrap_or_default();
        a.no_unknown_flags()?;
        let m = CtrlMode::parse(&v).ok_or_else(|| CliError::usage("usage: omarchyvr controllers use when-held|always|off"))?;
        ctrl_inhand::request(&ctrl_inhand::request_path(), m, Duration::from_millis(1000)).map_err(CliError::new)?;
        println!("controllers: {} (until the session ends; the app's switch sets the default)", m.label().to_lowercase());
        return Ok(());
    }
    if a.pos::<String>(0)?.as_deref() == Some("angle") {
        // `omarchyvr controllers angle [DEG]`: the controller angle (PIMAX_CTRL_ANGLE): saved, and live in a running session
        let v = a.pos::<String>(1)?;
        a.no_unknown_flags()?;
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let store = omarchyvr_config::Store::from_env(&env);
        let Some(v) = v else {
            let deg = omarchyvr_config::ctrl_angle::saved(&store).unwrap_or(omarchyvr_config::ctrl_angle::DEFAULT_DEG);
            println!("controller angle: {deg} deg (saved setting; -20..70, default 30)");
            return Ok(());
        };
        let deg = omarchyvr_config::ctrl_angle::parse(&v).map_err(|e| CliError::usage(format!("usage: omarchyvr controllers angle DEG: {e}")))?;
        let saved = omarchyvr_config::ctrl_angle::save(&store, deg).map_err(CliError::new)?;
        let rt = std::path::PathBuf::from(env("XDG_RUNTIME_DIR").ok_or_else(|| CliError::new("XDG_RUNTIME_DIR is not set"))?);
        let session = std::path::PathBuf::from(env("PIMAX_VR_SESSION_DIR").unwrap_or_else(|| crate::vr::config::SESSION_DIR.to_string()));
        crate::vr::sixdof::apply_ctrl_angle_live(&rt, &session, deg).map_err(CliError::new)?;
        println!("controller angle: {deg} deg (now in a running session{})", if saved { " and saved" } else { "; no profile to save it in" });
        return Ok(());
    }
    if a.pos::<String>(0)?.as_deref() == Some("status") {
        a.no_unknown_flags()?;
        match ctrl_inhand::read_status(&ctrl_inhand::status_path(), ctrl_inhand::pid_alive) {
            Some(s) => println!(
                "controllers {}: left {} ({}), right {} ({})",
                s.mode.label().to_lowercase(),
                s.left.words(),
                s.left.why,
                s.right.words(),
                s.right.why
            ),
            None => println!("controllers: no VR session (or its Monado build has no PIMAX_CTRL_INHAND)"),
        }
        return Ok(());
    }
    if a.pos::<String>(0)?.as_deref() == Some("buzz") {
        let side = a.pos::<String>(1)?.unwrap_or_default();
        let amp: u32 = a.opt_parse("--amp")?.unwrap_or(crate::buzz::DEFAULT_AMP as u32);
        let ms: u32 = a.opt_parse("--ms")?.unwrap_or(crate::buzz::DEFAULT_MS as u32);
        a.no_unknown_flags()?;
        println!("{}", crate::buzz::buzz_side(&side, amp, ms).map_err(CliError::new)?);
        return Ok(());
    }
    let raw = a.flag("--raw");
    a.no_unknown_flags()?;
    let duration: Option<f64> = a.pos(0)?;
    let ifs = discovery::find_interfaces(&discovery::Roots::from_env());
    if ifs[0].is_none() {
        return Err(CliError::new("headset IF0 not found (34a4:0044 absent?)"));
    }
    let map: Vec<String> = ifs
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|p| format!("{i}: '{}'", p.display())))
        .collect();
    println!("interfaces: {{{}}}", map.join(", "));
    let stop = pimax::sys::install_term_flag();
    let cfg = Config {
        wake: false,
        keepalive: false,
        auto_attach: true,
        ..Config::default()
    };
    let (s, rx) = Session::open_channel(cfg, 262_144, true)?;
    s.wait_connected(Duration::from_secs(5))
        .map_err(|_| CliError::new("headset gone"))?;

    let t0 = Instant::now();
    let el = || t0.elapsed().as_secs_f64();
    let mut last: HashMap<u8, ControllerReport> = HashMap::new();
    let mut last_rx: HashMap<u8, Instant> = HashMap::new();
    let mut prev: HashMap<u8, ControllerReport> = HashMap::new();
    let mut count = [0u64; 4];
    let mut batt37: HashMap<u8, u8> = HashMap::new();
    let mut replied: [BTreeSet<u8>; 4] = Default::default();
    let mut bursts = [0u32; 4];
    bursts[2] = 1;
    bursts[3] = 1;
    let mut n37 = 0u64;
    let (mut next_print, mut next_status) = (0.1, 3.0);
    while duration.is_none_or(|d| el() < d) && !stop.load(std::sync::atomic::Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Event::Controller(c)) => {
                if !last.contains_key(&c.slot) {
                    println!(
                        "[{:5.1}s] {} controller attached: EP 0x82 reports flowing",
                        el(),
                        name(c.slot)
                    );
                }
                if raw {
                    println!("RAW {} {}", name(c.slot), hex(&c.raw));
                }
                count[c.slot as usize] += 1;
                last_rx.insert(c.slot, Instant::now());
                last.insert(c.slot, c);
            }
            Ok(Event::Status(st)) => {
                n37 += 1;
                batt37.insert(st.slot, st.battery);
                if raw || n37 <= 2 {
                    println!(
                        "0x37 [({}, {})]: {}",
                        st.slot,
                        st.battery,
                        hex(&st.raw[..16])
                    );
                }
            }
            Ok(Event::Reply(d)) => {
                if pimax_proto::parse_status(&d).iter().any(Option::is_some) {
                    continue;
                }
                if let Some(r) = parse_reply(&d) {
                    if [0x16, 0x50, 0x0a, 0x03].contains(&r.cmd) && (r.opt == 2 || r.opt == 3) {
                        replied[r.opt as usize].insert(r.cmd);
                        if r.cmd == 0x16 {
                            let txt: String = d[5..24]
                                .iter()
                                .map(|&b| if b.is_ascii() { b as char } else { '\u{fffd}' })
                                .collect();
                            println!("[{:5.1}s] {} serial: {txt}", el(), name(r.opt));
                        } else if raw {
                            println!("reply 0x{:02x} slot {}: {}", r.cmd, r.opt, hex(&d[..16]));
                        }
                    } else if ![0x39, 0x19, 0x51].contains(&r.cmd) {
                        println!("reply [if0] {}", hex(&d[..20]));
                    }
                }
            }
            Ok(Event::Attach(slot, retry)) => {
                if retry {
                    println!(
                        "[{:5.1}s] {} controller awake (0x37), re-sending attach burst",
                        el(),
                        name(slot)
                    );
                    bursts[slot as usize] += 1;
                }
            }
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(CliError::new("session ended")),
        }
        let now = Instant::now();
        if el() >= next_print {
            next_print = el() + 0.1;
            let mut slots: Vec<u8> = last.keys().copied().collect();
            slots.sort();
            for slot in slots {
                if now.duration_since(last_rx[&slot]).as_secs_f64() > STALE_AFTER {
                    continue;
                }
                let c = last[&slot];
                let mut diff = String::new();
                if let Some(p) = prev.get(&slot) {
                    let ch: Vec<String> = (4..64)
                        .filter(|&i| {
                            p.raw[i] != c.raw[i]
                                && !decoded(i)
                                && !diff_ignored(i)
                                && !(8..16).contains(&i)
                        })
                        .map(|i| format!("{i}:{:02x}>{:02x}", p.raw[i], c.raw[i]))
                        .collect();
                    if !ch.is_empty() {
                        diff = format!(" | chg {}", ch.join(" "));
                    }
                }
                prev.insert(slot, c);
                println!(
                    "{}{diff}",
                    render(&c, count[slot as usize], batt37.get(&slot).copied())
                );
            }
        }
        if el() >= next_status {
            next_status = el() + 3.0;
            let live = [2u8, 3].iter().any(|s| {
                last_rx
                    .get(s)
                    .is_some_and(|t| now.duration_since(*t).as_secs_f64() <= STALE_AFTER)
            });
            if !live {
                let why = if n37 == 0 {
                    "asleep: no 0x37 announce, 0x16/0x0a/0x03 unanswered"
                } else {
                    "0x37 seen but not attached yet"
                };
                println!("[{:5.1}s] no controller reports ({why}). PRESS A BUTTON on each controller (or power it on) to wake it; the burst is re-sent automatically when it announces.", el());
            }
        }
    }
    let fmt = |s: &BTreeSet<u8>| {
        format!(
            "[{}]",
            s.iter()
                .map(|c| format!("'0x{c:02x}'"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    println!(
        "done: reports L={} R={}, 0x37 msgs={n37}, bursts L={} R={}, cmd replies {{L:{}, R:{}}}",
        count[2],
        count[3],
        bursts[2],
        bursts[3],
        fmt(&replied[2]),
        fmt(&replied[3])
    );
    Ok(())
}
