//! Soak test of the window in demo mode with a running desk session (live status every second, like the real app):
//! `PIMAX_SOAK_SECS` (default 60) of
//! * `PIMAX_SOAK_MODE=idle`: a frame every 100 ms, nothing touched (a window left open), or
//! * `PIMAX_SOAK_MODE=resize`: a new window size every frame, another page every 5 s, the Logs tabs, the output
//!   panel and the log filter in between.
//!
//! Checks: the UI thread never touches the machine, no frame over the budget, memory and threads stay flat, the
//! output and log views stay bounded, no panic. A summary goes to stderr and to `PIMAX_SOAK_OUT` if set.
//!
//! `PIMAX_SOAK_SECS=1800 PIMAX_SOAK_MODE=idle cargo test --release -p pimax-setup --test soak -- --ignored --nocapture`
mod common;

use common::{rss_threads, Ui};
use eframe::egui::accesskit::Role;
use pimax_gui_core::app::Page;
use pimax_setup::app::LogsTab;
use std::time::{Duration, Instant};

#[test]
#[ignore]
fn soak() {
    let secs: u64 = std::env::var("PIMAX_SOAK_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let mode = std::env::var("PIMAX_SOAK_MODE").unwrap_or_else(|_| "idle".into());
    let real = std::env::var("PIMAX_SOAK_REAL").is_ok_and(|v| v == "1");
    // the window when idle draws about one frame per second (its 1 s timer and the live status)
    let step_ms: u64 = std::env::var("PIMAX_SOAK_STEP_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    let mut ui = if real { Ui::new_real(&format!("soak-real-{mode}")) } else { Ui::new(&format!("soak-{mode}"), true, Duration::from_millis(1)) };
    ui.wait_facts();
    // memory and threads are sampled on a thread of their own: the UI thread must not read files, the test's own
    // measurements included
    let samples: std::sync::Arc<std::sync::Mutex<Vec<(f64, u64, u64)>>> = Default::default();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = {
        let (samples, stop) = (samples.clone(), stop.clone());
        let t = Instant::now();
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let (r, th) = rss_threads();
                samples.lock().unwrap().push((t.elapsed().as_secs_f64(), r, th));
                std::thread::sleep(Duration::from_secs(1));
            }
        })
    };
    std::thread::sleep(Duration::from_millis(1100));
    let (rss0, thr0) = samples.lock().unwrap().last().map(|x| (x.1, x.2)).unwrap_or((0, 0));
    let t0 = Instant::now();
    let cpu0 = common::cpu_secs();
    let warm = Duration::from_secs(secs.min(120) / 2);
    let mut warm_rss: Option<(u64, u64)> = None;
    let mut max_rss = rss0;
    let mut max_threads = thr0;
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut rnd = |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n.max(1)
    };
    let mut next_page = Instant::now();
    let mut page_i = 0usize;
    let mut last_report = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        match mode.as_str() {
            "resize" => {
                let w = 760.0 + rnd(1160) as f32;
                let h = 520.0 + rnd(680) as f32;
                ui.resize(w, h);
                if next_page.elapsed() > Duration::from_secs(5) {
                    next_page = Instant::now();
                    page_i += 1;
                    let p = Page::ALL[page_i % Page::ALL.len()];
                    ui.h.state_mut().core.page = p;
                    if p == Page::Logs {
                        let tabs = [LogsTab::Logs, LogsTab::History, LogsTab::Issues, LogsTab::Lessons, LogsTab::Safety];
                        ui.h.state_mut().ui.logs_tab = tabs[(page_i / Page::ALL.len()) % tabs.len()];
                    }
                    ui.step("page change");
                    // a few clicks that change nothing outside the window
                    let out = ui.widgets().into_iter().find(|(r, l)| *r == Role::Button && (l.starts_with("Output") || l.starts_with("Hide output")));
                    if let Some((r, l)) = out {
                        ui.click(r, &l, 0);
                    }
                    if p == Page::Logs && rnd(2) == 0 {
                        if let Some((r, l)) = ui.widgets().into_iter().find(|(r, _)| *r == Role::TextInput) {
                            ui.click(r, &l, 0);
                        }
                    }
                    ui.close_dialogs();
                }
                std::thread::sleep(Duration::from_millis(16));
            }
            _ => {
                ui.step("idle");
                std::thread::sleep(Duration::from_millis(step_ms));
            }
        }
        let (rss, thr) = samples.lock().unwrap().last().map(|x| (x.1, x.2)).unwrap_or((rss0, thr0));
        max_rss = max_rss.max(rss);
        max_threads = max_threads.max(thr);
        if warm_rss.is_none() && t0.elapsed() > warm {
            warm_rss = Some((rss, thr));
        }
        if last_report.elapsed() > Duration::from_secs(300) {
            last_report = Instant::now();
            let (n, max, p99, _) = ui.stats();
            eprintln!("soak {mode}: {:.0} s, {n} frames, max {max:?}, p99 {p99:?}, rss {rss} kB, threads {thr}", t0.elapsed().as_secs_f64());
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    sampler.join().unwrap();
    let (rss1, thr1) = samples.lock().unwrap().last().map(|x| (x.1, x.2)).unwrap_or((rss0, thr0));
    let (wrss, wthr) = warm_rss.unwrap_or((rss0, thr0));
    let (n, max, p99, mean) = ui.stats();
    let cpu_pct = 100.0 * (common::cpu_secs() - cpu0) / t0.elapsed().as_secs_f64();
    let a = ui.h.state();
    let summary = format!(
        "soak mode={mode} real={real} step_ms={step_ms} cpu_pct_of_one_core={cpu_pct:.2} secs={secs} frames={n} clicks={} max_frame_ms={:.2} p99_ms={:.2} mean_ms={:.3} slowest=\"{}\" rss_kb_start={rss0} rss_kb_warm={wrss} rss_kb_end={rss1} rss_kb_max={max_rss} threads_start={thr0} threads_warm={wthr} threads_end={thr1} threads_max={max_threads} job_log_lines={} tail_lines={} violations={} app_panics={}",
        ui.clicked,
        max.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0,
        mean.as_secs_f64() * 1000.0,
        ui.slowest.1,
        a.core.log.len(),
        a.core.logs.tail.as_ref().map(|(_, l)| l.len()).unwrap_or(0),
        ui.probe.violations().len(),
        common::app_panics().len(),
    );
    eprintln!("{summary}");
    if let Ok(p) = std::env::var("PIMAX_SOAK_OUT") {
        let _ = std::fs::write(p, format!("{summary}\n"));
    }
    assert!(a.core.log.len() <= 3000 && a.core.logs.tail.as_ref().is_none_or(|(_, l)| l.len() <= 600), "views stay bounded");
    assert!(common::app_panics().is_empty(), "panics: {:?}", common::app_panics());
    assert!(rss1 < wrss + 64 * 1024, "memory grew from {wrss} kB (warm) to {rss1} kB");
    assert!(thr1 <= wthr + 8, "threads grew from {wthr} to {thr1}");
    ui.assert_responsive();
    let _ = std::fs::remove_dir_all(&ui.dir);
}
