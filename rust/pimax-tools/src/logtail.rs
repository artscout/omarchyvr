//! `pimax logtail [seconds] [regex]`: passive tail of the headset firmware log (IF3) and the display-bridge MCU log
//! (34a4:0043). Sends nothing. Port of `linux/pimax_logtail.py`.
use crate::args::Args;
use crate::CliResult;
use pimax::{discovery, hid, sys};
use pimax_proto::LineSplitter;
use std::fs::File;
use std::time::{Duration, Instant};

pub(crate) struct Tail {
    pub name: &'static str,
    pub f: File,
    pub split: LineSplitter,
}

pub(crate) fn open_tails() -> Vec<Tail> {
    let roots = discovery::Roots::from_env();
    let mut v = vec![];
    if let Some(p) = &discovery::find_interfaces(&roots)[3] {
        if let Ok(f) = hid::open(p, false) {
            v.push(Tail {
                name: "hmd",
                f,
                split: LineSplitter::new(),
            });
        }
    }
    if let Some(p) = discovery::find_bridge_log(&roots) {
        if let Ok(f) = hid::open(&p, false) {
            v.push(Tail {
                name: "bridge",
                f,
                split: LineSplitter::new(),
            });
        }
    }
    v
}

/// Poll the tails once; calls `line(name, text)` for each complete line. Returns false if a device vanished.
pub(crate) fn pump(
    tails: &mut [Tail],
    timeout: Duration,
    mut line: impl FnMut(&str, &str),
) -> bool {
    if tails.is_empty() {
        std::thread::sleep(timeout);
        return true;
    }
    let files: Vec<&File> = tails.iter().map(|t| &t.f).collect();
    let Ok(ready) = sys::wait_readable(&files, Some(timeout)) else {
        return false;
    };
    for (t, r) in tails.iter_mut().zip(ready) {
        if !r {
            continue;
        }
        let mut d = [0u8; 64];
        loop {
            match hid::read_report(&t.f, &mut d) {
                Ok(Some(n)) => {
                    let name = t.name;
                    t.split.push(&d[..n], |l| {
                        let s = String::from_utf8_lossy(l);
                        let s = s.trim();
                        if !s.is_empty() {
                            line(name, s);
                        }
                    });
                }
                Ok(None) => break,
                Err(_) => return false,
            }
        }
    }
    true
}

/// Entry point.
pub fn run(a: Args) -> CliResult {
    a.no_unknown_flags()?;
    let secs: f64 = a.pos(0)?.unwrap_or(30.0);
    let pat: Option<String> = a.pos(1)?;
    let re = pat
        .map(|p| regex_lite::Regex::new(&p))
        .transpose()
        .map_err(|e| crate::CliError::usage(format!("bad regex: {e}")))?;
    let stop = sys::install_term_flag();
    let mut tails = open_tails();
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs && !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let t = t0.elapsed().as_secs_f64();
        let ok = pump(&mut tails, Duration::from_millis(200), |name, text| {
            if re.as_ref().is_none_or(|r| r.is_match(text)) {
                println!("{t:6.2}  {name:6} {text}");
            }
        });
        if !ok {
            break;
        }
    }
    Ok(())
}
