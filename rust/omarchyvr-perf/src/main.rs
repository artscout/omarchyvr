//! `omarchyvr-perf run [--file F] [--wayvrctl P] [--overlay perf]`: stats file of Monado -> WayVR wrist panel, 2x a second,
//! plus a frame-health line every 5 s on stderr (the session's perf.log; see `timeline`).

use omarchyvr_perf::{parse, Tracker, View};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

const USAGE: &str = "usage: omarchyvr-perf run [--file F] [--wayvrctl P] [--overlay NAME] [--interval-ms N]
  F defaults to $PIMAX_PERF_FILE, else $XDG_RUNTIME_DIR/omarchyvr-perf.stats. Monado must run with PIMAX_PERF_FILE=F.";

/// A stats file older than this is treated as 'no data' (Monado stopped or no frames).
const STALE: Duration = Duration::from_millis(2500);

fn default_file() -> PathBuf {
    if let Some(f) = std::env::var_os("PIMAX_PERF_FILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(f);
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dir.join("omarchyvr-perf.stats")
}

fn read(path: &PathBuf) -> Option<omarchyvr_perf::Stats> {
    let age = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .unwrap_or_default();
    if age > STALE {
        return None;
    }
    parse(&std::fs::read_to_string(path).ok()?)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("run") {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    args.remove(0);
    let mut file = default_file();
    let mut wayvrctl = PathBuf::from(
        std::env::var_os("HOME").unwrap_or_default(),
    )
    .join(".local/lib/omarchyvr/wayvr/wayvrctl");
    let mut overlay = "perf".to_string();
    let mut interval = 500u64;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| {
            eprintln!("{a} needs a value\n{USAGE}");
            std::process::exit(2)
        });
        match a.as_str() {
            "--file" => file = PathBuf::from(val()),
            "--wayvrctl" => wayvrctl = PathBuf::from(val()),
            "--overlay" => overlay = val(),
            "--interval-ms" => interval = val().parse().unwrap_or(500).clamp(100, 5000),
            _ => {
                eprintln!("unknown option {a}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let t0 = Instant::now();
    let mut tracker = Tracker::new();
    let mut timeline = omarchyvr_perf::Timeline::new();
    let mut link = omarchyvr_perf::wayvr::Wayvr::new(wayvrctl, &overlay);
    let mut shown: Option<(View, Instant)> = None;
    eprintln!("omarchyvr-perf: reading {}", file.display());
    loop {
        let view = match read(&file) {
            Some(s) => {
                let now = t0.elapsed().as_secs_f64();
                let unix = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or_default();
                if let Some(line) = timeline.push(&s, now, unix) {
                    eprintln!("{line}");
                }
                tracker.view(&s, now)
            }
            None => View::none(),
        };
        // send on change, and every 2 s anyway (a restarted WayVR or a dead wayvrctl link must not leave a stale panel)
        let due = match &shown {
            Some((v, at)) => *v != view || at.elapsed() >= Duration::from_secs(2),
            None => true,
        };
        if due {
            let mut fresh = false;
            shown = link.show(&view, &mut fresh).then(|| (view, Instant::now()));
        }
        std::thread::sleep(Duration::from_millis(interval));
    }
}
