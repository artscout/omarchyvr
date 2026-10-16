//! `pimax-lens`: inspect a lens.conf without Monado or a headset.
//!
//! * `show [--conf PATH]`   effective configuration, hash of the file, model numbers per eye (ring radii, corners)
//! * `grid [--conf PATH] [--out DIR] [--div N]`   write the UV grid pictures and the summary (what
//!   `PIMAX_LENS_DEBUG_GRID=DIR` does inside the driver)
//! * `fit-pvr <pimax-distortion-*.json> [--out FILE] [--conf FILE] [--dry-run] ...`   fit our model to Pimax's own distortion
//!   (dumped on Windows by `windows/pimax-dump-distortion.ps1`), compare with lens.conf, write `lens-ipd/pvr.conf`
//! * `make-lut <dump.json> --out FILE`   convert the dump into the exact-distortion LUT file (lens.conf `lut = FILE`)
//! * `fit [--terms N] [--rmax R] [--window N] [--table FILE]`   fit the polynomial to an angle -> radius table
//!   (built-in: "Dream Air like Pimax"), optionally continued out to the panel corners, print a lens.conf fragment

use pimax_lens::{config, fit, grid, load, load_file, png, write_debug_grid, Level};
use std::path::PathBuf;
use std::process::ExitCode;

const ASPECT: f32 = 3840.0 / 3552.0;

fn usage() -> ExitCode {
    eprintln!(
        "usage: pimax-lens show [--conf PATH]\n       pimax-lens grid [--conf PATH] [--out DIR] [--div N]\n       pimax-lens fit [--terms N] [--rmax R] [--window N] [--table FILE]\n       {}\n       {}",
        pimax_lens::pvrfit::USAGE,
        pimax_lens::lut::USAGE
    );
    ExitCode::from(2)
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn loaded(args: &[String]) -> pimax_lens::Loaded {
    match opt(args, "--conf") {
        Some(p) => load_file(ASPECT, &config::process_env, PathBuf::from(p)),
        None => load(ASPECT, &config::process_env),
    }
}

fn cmd_show(args: &[String]) -> ExitCode {
    let l = loaded(args);
    println!(
        "config file: {} ({})",
        l.path.display(),
        if l.file_found {
            "found"
        } else {
            "not found, built-in defaults"
        }
    );
    if l.file_found {
        if let Ok(bytes) = std::fs::read(&l.path) {
            println!(
                "config crc32: {:08x}  ({} bytes)",
                png::crc32(&bytes),
                bytes.len()
            );
        }
    }
    for m in &l.messages {
        println!(
            "{}{}",
            if m.level == Level::Warn {
                "WARNING "
            } else {
                ""
            },
            m.text
        );
    }
    println!();
    for view in 0..2 {
        print!("{}", grid::summary(&l.lens, view));
    }
    ExitCode::SUCCESS
}

fn cmd_grid(args: &[String]) -> ExitCode {
    let l = loaded(args);
    let out = PathBuf::from(opt(args, "--out").unwrap_or_else(|| "lens-grid".to_string()));
    let div = opt(args, "--div").and_then(|d| d.parse().ok()).unwrap_or(4);
    let mut bad = false;
    for m in write_debug_grid(&l.lens, &out, div) {
        println!("{}", m.text);
        bad |= m.level == Level::Warn;
    }
    if bad {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn read_table(path: &str) -> Result<Vec<(f64, f64)>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut v = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split([',', ' ', '\t']).filter(|s| !s.is_empty());
        let a = it.next().and_then(|s| s.parse::<f64>().ok());
        let r = it.next().and_then(|s| s.parse::<f64>().ok());
        match (a, r) {
            (Some(a), Some(r)) => v.push((a, r)),
            _ => return Err(format!("{path}:{}: expected 'angle_deg radius'", i + 1)),
        }
    }
    Ok(v)
}

fn cmd_fit(args: &[String]) -> ExitCode {
    let terms = opt(args, "--terms")
        .and_then(|d| d.parse().ok())
        .unwrap_or(5usize);
    let rmax = opt(args, "--rmax")
        .and_then(|d| d.parse().ok())
        .unwrap_or(1.5f64);
    let window = opt(args, "--window")
        .and_then(|d| d.parse().ok())
        .unwrap_or(3usize);
    let table: Vec<(f64, f64)> = match opt(args, "--table") {
        Some(p) => match read_table(&p) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
        None => fit::DREAM_AIR_LIKE_PIMAX_GREEN.to_vec(),
    };
    let last_r = table.last().map_or(0.0, |s| s.1);
    let cont_weight = opt(args, "--cont-weight")
        .and_then(|d| d.parse().ok())
        .unwrap_or(0.1f64);
    let mut samples: Vec<(f64, f64, f64)> = table.iter().map(|&(a, r)| (a, r, 1.0)).collect();
    let extra = if rmax > last_r {
        fit::continuation(&table, rmax, 0.04, window)
    } else {
        Vec::new()
    };
    samples.extend(extra.iter().map(|&(a, r)| (a, r, cont_weight)));
    let Some(f) = fit::fit_weighted(&samples, terms) else {
        eprintln!(
            "fit failed: need at least {} samples and 1..=5 terms",
            terms + 1
        );
        return ExitCode::FAILURE;
    };
    let table_err = table
        .iter()
        .map(|&(a, r)| (f.angle_deg(r) - a).abs())
        .fold(0.0, f64::max);
    println!("# {} table samples (radius up to {last_r:.3}), {} continuation samples (tan(angle) linear in r, mean slope of the last {window} segments; an assumption, not a measurement)", table.len(), extra.len());
    println!("# {terms} polynomial terms; max angle error on the table {table_err:.3} deg, over all samples {:.3} deg (rms {:.3})", f.max_err_deg, f.rms_err_deg);
    if let Some(&(a, r)) = extra.last() {
        println!("# at the last continuation radius {r:.3} the model looks {a:.1} deg off axis");
    }
    print!(
        "{}",
        fit::conf_fragment(&f, if extra.is_empty() { 0.0 } else { rmax })
    );
    ExitCode::SUCCESS
}

fn cmd_fit_pvr(args: &[String]) -> ExitCode {
    match pimax_lens::pvrfit::cli(args, &config::process_env) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    // `pimax-lens show | head` must end quietly, not with a panic about the closed pipe
    // SAFETY: restoring the default SIGPIPE disposition before anything else runs.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("show") => cmd_show(&args[1..]),
        Some("grid") => cmd_grid(&args[1..]),
        Some("fit") => cmd_fit(&args[1..]),
        Some("fit-pvr") => cmd_fit_pvr(&args[1..]),
        Some("make-lut") => match pimax_lens::lut::cli(&args[1..]) {
            Ok(t) => {
                print!("{t}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("pimax-lens make-lut: {e}");
                ExitCode::FAILURE
            }
        },
        _ => usage(),
    }
}
