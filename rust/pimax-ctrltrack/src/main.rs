#![allow(
    clippy::needless_range_loop,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::manual_is_multiple_of
)]
use pimax_cam::frame::{self, FrameKind, Meta, TILE_COUNT, TILE_H, TILE_W};
use pimax_cam::record::Reader;
use pimax_ctrltrack::blob::{self, BlobParams};
use std::path::Path;
mod live;
mod lockrate;
mod track;
mod tracker;
use track::track;

fn usage() -> ! {
    eprintln!("usage: pimax-ctrltrack scan FILE.pxcam [--every N]\n       pimax-ctrltrack dump FILE.pxcam FRAME OUT.png   (LED frame mosaic, blobs marked red)\n       pimax-ctrltrack track FILE.pxcam [--start N] [--count N] [--every N] [--calib XML] [--left JSON] [--right JSON] [--csv OUT] [--no-prior] [--overlay DIR [--overlay-every N]] [--fit-extrinsics [--save-extrinsics P]] [--extrinsics P | --factory]\n       pimax-ctrltrack live [--source broker|FILE] [--out POSEFILE] [--seconds N] [--aim-pitch-deg D] [--extrinsics P | --factory] [--baseline] [--no-body] [--thr T] [--peak P]\n       pimax-ctrltrack lockrate FILE.pxcam [--baseline] [--oracle] [--start N] [--count N] [--csv OUT] [--few N] [--wide PX] [--no-predict] [--rot-w W] [--thr T] [--peak P]   (offline lock-rate replay through the live tracker)\n       pimax-ctrltrack show [--out POSEFILE] [--seconds N]\n       pimax-ctrltrack health [--out POSEFILE] [--max-age S]   (heartbeat as JSON; exit 0 fresh, 1 no file / no heartbeat, 2 stale or source down)\n       pimax-ctrltrack imufit LOG.csv... [--pool] [--save [PATH]]   (controller gyro calibration from a PIMAX_CTRL_IMU_LOG log; --save writes ~/.config/omarchyvr/ctrl-imu.json)\n       pimax-ctrltrack fusereplay IN.csv OUT.csv [--no-filter] [--no-still-lock]   (replay a log through the fuser, A/B)\n       pimax-ctrltrack occlusion LOG.csv... [--gaps 0.5,1,2] [--fit-forearm]   (masked replay: hide visible fixes, score the out-of-view pose)");
    std::process::exit(2)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        usage();
    }
    match a[1].as_str() {
        "scan" => scan(
            &a[2],
            a.iter()
                .position(|s| s == "--every")
                .map(|i| a[i + 1].parse().unwrap())
                .unwrap_or(1),
        ),
        "track" => track(&a[2..]),
        "live" => live::live(&a[2..]),
        "show" => live::show(&a[2..]),
        "health" => live::health(&a[2..]),
        "lockrate" => lockrate::lockrate(&a[2..]),
        "fusereplay" => {
            // fusereplay IN.csv OUT.csv [--no-filter] [--no-still-lock]
            let a = &a[2..];
            if a.len() < 2 {
                usage();
            }
            let nf = a.iter().any(|s| s == "--no-filter");
            let ns = a.iter().any(|s| s == "--no-still-lock");
            match pimax_ctrltrack::fusereplay::replay(
                std::path::Path::new(&a[0]),
                std::path::Path::new(&a[1]),
                |c| {
                    c.filter = !nf;
                    c.still_lock = !ns;
                },
            ) {
                Ok((fixes, shown)) => println!(
                    "{}: {fixes} fixes accepted, {shown} shown poses -> {}",
                    a[0], a[1]
                ),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        "imufit" => {
            let a = &a[2..];
            let save = a.iter().position(|s| s == "--save").map(|i| {
                a.get(i + 1)
                    .filter(|p| !p.starts_with("--"))
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(pimax_hands::fuse::calib_path)
            });
            let files: Vec<&String> = a
                .iter()
                .filter(|s| {
                    !s.starts_with("--")
                        && Some(s.as_str()) != save.as_ref().and_then(|p| p.to_str())
                })
                .collect();
            if files.is_empty() {
                usage();
            }
            if a.iter().any(|s| s == "--pool") {
                let paths: Vec<&std::path::Path> =
                    files.iter().map(|f| std::path::Path::new(f.as_str())).collect();
                if let Err(e) = pimax_ctrltrack::imufit::run_pooled(&paths, save.as_deref()) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            let mut rc = 0;
            for f in files {
                if let Err(e) =
                    pimax_ctrltrack::imufit::run(std::path::Path::new(f), save.as_deref())
                {
                    eprintln!("{f}: {e}");
                    rc = 1;
                }
            }
            std::process::exit(rc);
        }
        "occlusion" => {
            if let Err(e) = pimax_ctrltrack::occlusion::run(&a[2..]) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        "dump" => dump(&a[2], a[3].parse().unwrap(), &a[4]),
        _ => usage(),
    }
}

fn scan(path: &str, every: usize) {
    let mut r = Reader::open(Path::new(path)).expect("open");
    let prm = BlobParams::default();
    let mut tile = vec![0u8; TILE_W * TILE_H];
    let mut n_led = 0usize;
    let mut hist = [[0usize; 8]; TILE_COUNT];
    let mut i = 0usize;
    while let Some(rec) = r.next_record().expect("read") {
        i += 1;
        let Some(m) = Meta::parse(&rec.data[..frame::META_LEN]) else {
            continue;
        };
        if m.kind() != FrameKind::Led {
            continue;
        }
        n_led += 1;
        if n_led % every != 0 {
            continue;
        }
        let rows = frame::tile_rows(Some(&m), &rec.data, rec.stride);
        let mut line = format!(
            "frame {i:5} ts {:.3} exp {:?} gain {:?}",
            m.ts_us as f64 * 1e-6,
            m.exposure,
            m.gain
        );
        for t in 0..TILE_COUNT {
            frame::copy_tile(&rec.data, rec.stride, t, rows.y[t], &mut tile, TILE_W);
            let b = blob::detect(&tile, TILE_W, TILE_H, TILE_W, &prm);
            hist[t][b.len().min(7)] += 1;
            let mx = tile.iter().copied().max().unwrap_or(0);
            line += &format!(" | c{t} n={:2} max={mx:3}", b.len());
        }
        println!("{line}");
    }
    eprintln!("LED frames {n_led}; blob-count histogram per camera (0..6, 7+):");
    for (t, h) in hist.iter().enumerate() {
        eprintln!("  cam{t}: {h:?}");
    }
}

/// Write the 4-tile mosaic of record number `want` (1-based, as printed by `scan`) with blobs marked.
fn dump(path: &str, want: usize, out: &str) {
    let mut r = Reader::open(Path::new(path)).expect("open");
    let mut i = 0usize;
    while let Some(rec) = r.next_record().expect("read") {
        i += 1;
        if i != want {
            continue;
        }
        let m = Meta::parse(&rec.data[..frame::META_LEN]).expect("meta");
        let rows = frame::tile_rows(Some(&m), &rec.data, rec.stride);
        let w = TILE_W * TILE_COUNT;
        let mut rgb = vec![0u8; w * TILE_H * 3];
        let mut tile = vec![0u8; TILE_W * TILE_H];
        for t in 0..TILE_COUNT {
            frame::copy_tile(&rec.data, rec.stride, t, rows.y[t], &mut tile, TILE_W);
            for y in 0..TILE_H {
                for x in 0..TILE_W {
                    let v = tile[y * TILE_W + x];
                    let o = (y * w + t * TILE_W + x) * 3;
                    rgb[o..o + 3].copy_from_slice(&[v, v, v]);
                }
            }
            for b in blob::detect(&tile, TILE_W, TILE_H, TILE_W, &BlobParams::default()) {
                let (cx, cy) = (b.x.round() as i64, b.y.round() as i64);
                for d in -8i64..=8 {
                    for (x, y) in [
                        (cx + d, cy - 8),
                        (cx + d, cy + 8),
                        (cx - 8, cy + d),
                        (cx + 8, cy + d),
                    ] {
                        if x >= 0 && y >= 0 && (x as usize) < TILE_W && (y as usize) < TILE_H {
                            let o = (y as usize * w + t * TILE_W + x as usize) * 3;
                            rgb[o..o + 3].copy_from_slice(&[255, 0, 0]);
                        }
                    }
                }
            }
        }
        pimax_cam::png::write_rgb(Path::new(out), w, TILE_H, &rgb).expect("png");
        eprintln!("wrote {out} ({:?})", m.kind());
        return;
    }
    eprintln!("frame {want} not found");
}
