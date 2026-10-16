//! `omarchyvr-desktest`: offline analysis for the VR-desktop tests. See `--help`.

use omarchyvr_desktest::dump::{Channel, Dump, Rect};
use omarchyvr_desktest::{metrics, png, sharp};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "omarchyvr-desktest <command> ...

  metrics FILE [--trim SECONDS] [--window T0,T1]
                                     Monado XRT_METRICS_FILE: compositor GPU time and frame cadence by set of client sessions;
                                     with --window (monotonic ns, e.g. from omarchyvr-voicebench) inside and outside are reported separately
  info RAW                           size, format and mean colour per eye of a compositor target dump
  png RAW OUT.png [--crop x,y,w,h] [--eye left|right] [--down N] [--up N]
                                     crop / box-downscale / pixel-repeat a dump into an 8-bit PNG
  black RAW --crop x,y,w,h           fraction of (near) black pixels in a crop
  edge RAW --crop x,y,w,h [--chan g]   10-90 % rise and MTF50 of the straight edge in a crop (final-image pixels)
  ladder RAW --crop x,y,w,h --period P [--kmin 0.8] [--kmax 2.0] [--chan g]
                                     line-pair ladder of P source px: local magnification and transfer (1.0 = lossless)
  (--chan r|g|b|y selects the channel analysed; default g, which avoids the chromatic-aberration shift of R and B)
";

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn chan(args: &[String]) -> Result<Channel, String> {
    opt(args, "--chan").map_or(Ok(Channel::G), |c| Channel::parse(c).ok_or_else(|| "bad --chan (r|g|b|y)".to_string()))
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).ok_or(HELP)?;
    let path = || -> Result<PathBuf, String> { args.get(1).map(PathBuf::from).ok_or_else(|| format!("{cmd}: missing file\n{HELP}")) };
    let crop = || -> Result<Rect, String> {
        Rect::parse(opt(args, "--crop").ok_or("--crop x,y,w,h is required")?).ok_or_else(|| "bad --crop (want x,y,w,h)".to_string())
    };
    match cmd {
        "metrics" => {
            let data = std::fs::read(path()?).map_err(|e| e.to_string())?;
            let trim: f64 = opt(args, "--trim").map_or(Ok(1.0), str::parse).map_err(|_| "bad --trim")?;
            let parsed = metrics::parse_stream(&data);
            println!("{} records, {} trailing bytes ignored", parsed.records.len(), parsed.trailing_bytes);
            if let Some(w) = opt(args, "--window") {
                let (a, b) = w.split_once(',').ok_or("--window wants T0,T1")?;
                let (t0, t1): (u64, u64) = (a.trim().parse().map_err(|_| "bad T0")?, b.trim().parse().map_err(|_| "bad T1")?);
                println!("--- INSIDE the window ({:.1} s)", (t1.saturating_sub(t0)) as f64 / 1e9);
                print!("{}", metrics::analyze_with(&parsed.records, 0.0, &|t| t >= t0 && t < t1));
                println!("--- OUTSIDE the window (trim {trim} s per class)");
                print!("{}", metrics::analyze_with(&parsed.records, trim, &|t| !(t >= t0 && t < t1)));
            } else {
                print!("{}", metrics::analyze(&parsed.records, trim));
            }
        }
        "info" => {
            let d = Dump::load(&path()?).map_err(|e| e.to_string())?;
            println!("{}x{} VkFormat {}", d.width, d.height, d.format);
            for (name, right) in [("left", false), ("right", true)] {
                let r = d.eye(right);
                let g = d.gray_crop(Rect { x: r.x, y: r.y + r.h / 4, w: r.w, h: r.h / 2 });
                let mean = g.data.iter().map(|v| f64::from(*v)).sum::<f64>() / g.data.len() as f64;
                println!("{name} eye: mean luma of the middle half {mean:.4}, black fraction {:.3}", d.black_fraction(r, 2));
            }
        }
        "png" => {
            let d = Dump::load(&path()?).map_err(|e| e.to_string())?;
            let out = args.get(2).ok_or("png: missing OUT.png")?;
            let mut r = match opt(args, "--crop") {
                Some(c) => Rect::parse(c).ok_or("bad --crop")?,
                None => Rect { x: 0, y: 0, w: d.width, h: d.height },
            };
            if let Some(e) = opt(args, "--eye") {
                let eye = d.eye(e == "right");
                r.x += eye.x;
                if opt(args, "--crop").is_none() {
                    r = eye;
                }
            }
            let down: u32 = opt(args, "--down").map_or(Ok(1), str::parse).map_err(|_| "bad --down")?;
            let up: u32 = opt(args, "--up").map_or(Ok(1), str::parse).map_err(|_| "bad --up")?;
            let img = d.crop_rgb8(r, down, up);
            png::write_rgb8(std::path::Path::new(out), img.w, img.h, &img.data).map_err(|e| e.to_string())?;
            println!("wrote {out} ({}x{})", img.w, img.h);
        }
        "black" => {
            let d = Dump::load(&path()?).map_err(|e| e.to_string())?;
            println!("black fraction {:.4}", d.black_fraction(crop()?, 2));
        }
        "edge" => {
            let d = Dump::load(&path()?).map_err(|e| e.to_string())?;
            let g = d.gray_crop_channel(crop()?, chan(args)?);
            let e = sharp::edge_profile(&g).ok_or("no clean straight edge found in the crop")?;
            println!(
                "edge normal {:.1} deg, 10-90 rise {:.2} px, MTF50 {} c/px, levels {:.3} -> {:.3}",
                e.normal_deg,
                e.rise_10_90,
                e.mtf50.map_or("n/a".to_string(), |m| format!("{m:.3}")),
                e.levels.0,
                e.levels.1
            );
        }
        "ladder" => {
            let d = Dump::load(&path()?).map_err(|e| e.to_string())?;
            let g = d.gray_crop_channel(crop()?, chan(args)?);
            let period: usize = opt(args, "--period").ok_or("--period P required")?.parse().map_err(|_| "bad --period")?;
            let kmin: f64 = opt(args, "--kmin").map_or(Ok(0.8), str::parse).map_err(|_| "bad --kmin")?;
            let kmax: f64 = opt(args, "--kmax").map_or(Ok(2.0), str::parse).map_err(|_| "bad --kmax")?;
            let l = sharp::ladder_transfer(&g, period, kmin, kmax).ok_or("no ladder found")?;
            println!(
                "period {period} src px: magnification k={:.3} final px per source px, depth final {:.3} vs source {:.3}, transfer {:.3} (orientation {:.1} deg)",
                l.k,
                l.depth_final,
                l.depth_source,
                l.transfer,
                l.peak.fy.atan2(l.peak.fx).to_degrees()
            );
        }
        _ => return Err(HELP.to_string()),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
