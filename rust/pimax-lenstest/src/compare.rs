//! `pimax-lenstest compare CAPTURE_DIR SIM_DIR`: the compositor's real output (capture PNGs) against the CPU model
//! of it (simulate PNGs), picture by picture. A real run that follows the model gives a root-mean-square difference
//! well below 1 %; a wrong rotation, a different lens.conf in the service or a compositor bug does not.

use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Root mean square difference over all channels, 0..1 of full scale.
    pub rms: f64,
    pub max_diff: u8,
    /// Fraction of pixels with any channel differing by more than 8 codes.
    pub bad_fraction: f64,
}

/// RMS difference, largest difference and the share of clearly different pixels of two RGB8 buffers.
pub fn diff(a: &[u8], b: &[u8]) -> (f64, u8, f64) {
    let n = a.len().min(b.len());
    if n == 0 {
        return (0.0, 0, 0.0);
    }
    let mut sq = 0.0f64;
    let mut max = 0u8;
    let mut bad = 0usize;
    for (pa, pb) in a[..n].chunks_exact(3).zip(b[..n].chunks_exact(3)) {
        let mut pixel_bad = false;
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            sq += f64::from(d) * f64::from(d);
            max = max.max(d);
            pixel_bad |= d > 8;
        }
        bad += usize::from(pixel_bad);
    }
    (
        (sq / n as f64).sqrt() / 255.0,
        max,
        bad as f64 / (n / 3) as f64,
    )
}

/// Compare every PNG of `capture` with the PNG of the same name in `sim`.
pub fn compare_dirs(capture: &Path, sim: &Path) -> Result<Vec<Row>, String> {
    let mut names: Vec<String> = std::fs::read_dir(capture)
        .map_err(|e| format!("{}: {e}", capture.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".png"))
        .collect();
    names.sort();
    let mut rows = Vec::new();
    for name in names {
        let other = sim.join(&name);
        if !other.exists() {
            continue;
        }
        let load = |p: &Path| -> Result<pimax_lens::png::Decoded, String> {
            pimax_lens::png::decode(&std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
                .map_err(|e| format!("{}: {e}", p.display()))
        };
        let (a, b) = (load(&capture.join(&name))?, load(&other)?);
        if (a.width, a.height, a.channels) != (b.width, b.height, b.channels) {
            return Err(format!(
                "{name}: sizes differ ({}x{} vs {}x{}); use the same --down / --div",
                a.width, a.height, b.width, b.height
            ));
        }
        let (rms, max_diff, bad_fraction) = diff(&a.data, &b.data);
        rows.push(Row {
            name,
            width: a.width,
            height: a.height,
            rms,
            max_diff,
            bad_fraction,
        });
    }
    Ok(rows)
}

/// Text table; `limit` is the RMS above which a row is flagged.
pub fn table(rows: &[Row], limit: f64) -> String {
    let mut s =
        String::from("picture                                  rms     max  >8 codes   verdict\n");
    for r in rows {
        s.push_str(&format!(
            "{:<40} {:>5.2}%  {:>3}  {:>7.3}%   {}\n",
            r.name,
            r.rms * 100.0,
            r.max_diff,
            r.bad_fraction * 100.0,
            if r.rms <= limit {
                "matches the model"
            } else {
                "DIFFERS from the model"
            }
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_of_equal_and_different_buffers() {
        let a = vec![10u8; 300];
        assert_eq!(diff(&a, &a), (0.0, 0, 0.0));
        let mut b = a.clone();
        b[0] = 20; // one pixel, one channel, 10 codes
        let (rms, max, bad) = diff(&a, &b);
        assert_eq!(max, 10);
        assert!((bad - 0.01).abs() < 1e-9);
        assert!((rms - (100.0f64 / 300.0).sqrt() / 255.0).abs() < 1e-9);
        // a difference of 8 or less is not "bad"
        b[0] = 18;
        assert_eq!(diff(&a, &b).2, 0.0);
        assert_eq!(diff(&[], &[]), (0.0, 0, 0.0));
    }

    #[test]
    fn dirs_are_compared_by_name_and_sizes_must_agree() {
        let root = std::env::temp_dir().join(format!("lenstest-cmp-{}", std::process::id()));
        let (cap, sim) = (root.join("cap"), root.join("sim"));
        std::fs::create_dir_all(&cap).unwrap();
        std::fs::create_dir_all(&sim).unwrap();
        let write = |dir: &Path, name: &str, w: u32, h: u32, v: u8| {
            pimax_lens::png::write_rgb8(&dir.join(name), w, h, &vec![v; (w * h * 3) as usize])
                .unwrap();
        };
        write(&cap, "01-a.png", 8, 4, 100);
        write(&sim, "01-a.png", 8, 4, 100);
        write(&cap, "02-b.png", 8, 4, 100);
        write(&sim, "02-b.png", 8, 4, 160);
        write(&cap, "03-only-in-capture.png", 8, 4, 1);
        let rows = compare_dirs(&cap, &sim).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].rms, 0.0);
        assert!(
            (rows[1].rms - 60.0 / 255.0).abs() < 1e-9
                && rows[1].max_diff == 60
                && rows[1].bad_fraction == 1.0
        );
        let t = table(&rows, 0.01);
        assert!(
            t.contains("01-a.png")
                && t.contains("matches the model")
                && t.contains("DIFFERS from the model"),
            "{t}"
        );
        write(&sim, "03-only-in-capture.png", 4, 4, 1);
        assert!(compare_dirs(&cap, &sim)
            .unwrap_err()
            .contains("sizes differ"));
        assert!(compare_dirs(&root.join("missing"), &sim).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
