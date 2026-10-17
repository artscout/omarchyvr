//! `omarchyvr-backgrounds`: the engine behind the app's Backgrounds page and "Max, set background ...".
//!
//! ```text
//! omarchyvr-backgrounds list [--json]            the library (downloaded pictures)
//! omarchyvr-backgrounds download [--theme T] [--only ID] [--limit N] [--deep]
//!                                            fetch / resume / verify; prints `PROGRESS i/n id status` lines
//! omarchyvr-backgrounds verify [--deep]          check every file of the library
//! omarchyvr-backgrounds set NAME|next|previous|off   choose (saved in ~/.config/omarchyvr/background.toml)
//! omarchyvr-backgrounds res auto|8k|4k|2k        choose the resolution
//! omarchyvr-backgrounds ocean [low|medium|high]  the animated ocean (Work only), optionally its quality
//! omarchyvr-backgrounds apply [--wayvrctl PATH]  show the saved choice in the running session
//! omarchyvr-backgrounds status                   the saved choice
//! omarchyvr-backgrounds sources                  the curated list
//! ```
use omarchyvr_backgrounds::fetch::{self, Options};
use omarchyvr_backgrounds::{Dirs, OceanQuality, Res, Setting, OCEAN_ID};
use std::path::PathBuf;
use std::process::ExitCode;

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let d = Dirs::from_env();
    let sub = args.first().map(String::as_str).unwrap_or("status");
    match run(sub, &args[args.len().min(1)..], &d) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("omarchyvr-backgrounds: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(sub: &str, args: &[String], d: &Dirs) -> Result<ExitCode, String> {
    match sub {
        "list" => {
            let lib = omarchyvr_backgrounds::library(d);
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&lib).map_err(|e| e.to_string())?);
            } else {
                for e in &lib {
                    let r: Vec<&str> = e.files.iter().map(|f| f.res.as_str()).collect();
                    println!("{:<44} {:<10} {}", e.voice_name(), r.join("/"), e.title);
                }
                println!("{} pictures, {} MB", lib.len(), fetch::library_bytes(&d.library) / 1_000_000);
            }
        }
        "sources" => {
            for s in fetch::sources()? {
                println!("{:<10} {:<10} {}", s.provider, s.theme, if s.r#ref.is_empty() { s.url.clone() } else { s.r#ref.clone() });
            }
        }
        "download" => {
            let o = Options {
                theme: arg(args, "--theme"),
                only: arg(args, "--only"),
                limit: arg(args, "--limit").and_then(|v| v.parse().ok()),
                deep: args.iter().any(|a| a == "--deep"),
            };
            let failed = fetch::run(d, &o, &mut |p| println!("{}", p.line()))?;
            println!("DONE failed={failed} library={} MB", fetch::library_bytes(&d.library) / 1_000_000);
            if failed > 0 {
                return Ok(ExitCode::from(2));
            }
        }
        "verify" => {
            let bad = fetch::verify(d, args.iter().any(|a| a == "--deep"));
            for b in &bad {
                println!("BAD {b}");
            }
            println!("{} pictures checked, {} problems", omarchyvr_backgrounds::library(d).len(), bad.len());
            if !bad.is_empty() {
                return Ok(ExitCode::from(2));
            }
        }
        "status" => {
            let s = Setting::load(d);
            println!("{}", if s.enabled { format!("background {} ({})", s.id, s.res.label()) } else { format!("background off (last: {}, {})", if s.id.is_empty() { "none" } else { &s.id }, s.res.label()) });
        }
        "res" => {
            let r = args.first().and_then(|v| Res::parse(v)).ok_or("res: auto, 8k, 4k or 2k")?;
            let mut s = Setting::load(d);
            s.res = r;
            s.save(d).map_err(|e| e.to_string())?;
            println!("resolution {}", r.label());
        }
        "set" => {
            let what = args.first().map(String::as_str).ok_or("set NAME|next|previous|off")?;
            let lib = omarchyvr_backgrounds::library(d);
            let mut s = Setting::load(d);
            match what {
                "off" | "none" | "passthrough" => s.enabled = false,
                "next" | "previous" => {
                    let cur = s.enabled.then_some(s.id.as_str());
                    let e = omarchyvr_backgrounds::step(&lib, cur, what == "next").ok_or("the library is empty (download first)")?;
                    s.enabled = true;
                    s.id = e.id;
                }
                name if omarchyvr_backgrounds::parse_ocean(name).is_some() => {
                    omarchyvr_backgrounds::remember(d, name).map_err(|e| e.to_string())?;
                    s = Setting::load(d);
                }
                name => {
                    let after = s.enabled.then_some(s.id.as_str());
                    let e = omarchyvr_backgrounds::find(&lib, name, after).ok_or_else(|| format!("no background \"{name}\""))?;
                    s.enabled = true;
                    s.id = e.id;
                }
            }
            s.save(d).map_err(|e| e.to_string())?;
            println!("{}", if s.enabled { format!("background {}", s.id) } else { "background off".into() });
            if args.iter().any(|a| a == "--apply") {
                let ctl = arg(args, "--wayvrctl").map(PathBuf::from).unwrap_or_else(omarchyvr_backgrounds::default_wayvrctl);
                println!("{}", omarchyvr_backgrounds::apply(d, &ctl)?);
            }
        }
        "ocean" => {
            let mut s = Setting::load(d);
            if let Some(q) = args.first().filter(|a| !a.starts_with("--")) {
                s.ocean_quality = OceanQuality::parse(q).ok_or("ocean: low, medium or high")?;
            }
            s.enabled = true;
            s.id = OCEAN_ID.to_string();
            s.save(d).map_err(|e| e.to_string())?;
            println!("background {} ({})", s.id, s.ocean_quality.label());
            if args.iter().any(|a| a == "--apply") {
                let ctl = arg(args, "--wayvrctl").map(PathBuf::from).unwrap_or_else(omarchyvr_backgrounds::default_wayvrctl);
                println!("{}", omarchyvr_backgrounds::apply(d, &ctl)?);
            }
        }
        "apply" => {
            let ctl = arg(args, "--wayvrctl").map(PathBuf::from).unwrap_or_else(omarchyvr_backgrounds::default_wayvrctl);
            println!("{}", omarchyvr_backgrounds::apply(d, &ctl)?);
        }
        _ => return Err("list | download | verify | set | res | ocean | apply | status | sources".into()),
    }
    Ok(ExitCode::SUCCESS)
}
