//! Input frames: sets of the four 640x480 tiles of one tracking frame ("scenes"), from tile PNGs (as written by
//! `pimax-cam tiles` / `rectify`) or from `.pxcam` recordings (tracking frames only).

use pimax_cam::frame::{TILE_COUNT, TILE_H, TILE_W};
use std::path::{Path, PathBuf};

/// The four tiles of one frame (raw sensor orientation: tiles 0 and 2 upside down, the rig handles that).
#[derive(Clone, Debug)]
pub struct Scene {
    /// Short name (frame counter or file stem).
    pub name: String,
    /// Tiles 0..3, `TILE_W` x `TILE_H` grey each.
    pub tiles: Vec<Vec<u8>>,
}

fn read_png(p: &Path) -> Result<Vec<u8>, String> {
    let g = omarchyvr_passthrough::pngdec::read(p)?;
    if (g.width, g.height) != (TILE_W, TILE_H) {
        return Err(format!(
            "{}: {}x{}, expected {TILE_W}x{TILE_H}",
            p.display(),
            g.width,
            g.height
        ));
    }
    Ok(g.pixels)
}

fn name_from(s: &str) -> String {
    // prefer the frame counter ("ctr49470") when the path has one
    if let Some(i) = s.rfind("ctr") {
        let digits: String = s[i + 3..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return format!("ctr{digits}");
        }
    }
    Path::new(s)
        .file_stem()
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.to_string())
}

/// Load `PREFIX0.png` .. `PREFIX3.png`.
pub fn from_prefix(prefix: &str) -> Result<Scene, String> {
    let tiles = (0..TILE_COUNT)
        .map(|i| read_png(Path::new(&format!("{prefix}{i}.png"))))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Scene {
        name: name_from(prefix),
        tiles,
    })
}

/// All tile sets in a directory: files named `<prefix>tile0.png` (contrast-stretched copies named
/// `..._stretched.png` are ignored when the plain one exists).
pub fn from_dir(dir: &Path) -> Result<Vec<Scene>, String> {
    let mut prefixes: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_suffix("tile0.png")
                .map(|p| dir.join(format!("{p}tile")).to_string_lossy().into_owned())
        })
        .collect();
    prefixes.sort();
    let mut out = Vec::new();
    for p in prefixes {
        out.push(from_prefix(&p)?);
    }
    if out.is_empty() {
        return Err(format!("{}: no *tile0.png", dir.display()));
    }
    Ok(out)
}

/// Tracking frames of a `.pxcam` recording: `first`, then every `step`-th tracking frame, `count` in total.
pub fn from_pxcam(
    path: &Path,
    first: usize,
    count: usize,
    step: usize,
) -> Result<Vec<Scene>, String> {
    let mut cam = omarchyvr_passthrough::frames::open_source(&path.to_string_lossy(), false, false)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut ts = omarchyvr_passthrough::frames::TileSet::black();
    let mut out = Vec::new();
    let mut idx = 0usize;
    while out.len() < count {
        if !omarchyvr_passthrough::frames::next_tracking(&mut cam, 200, &mut ts)
            .map_err(|e| e.to_string())?
        {
            break;
        }
        if idx >= first && (idx - first).is_multiple_of(step.max(1)) {
            out.push(Scene {
                name: format!("ctr{}", ts.counter),
                tiles: ts.tiles.to_vec(),
            });
        }
        idx += 1;
    }
    if out.is_empty() {
        return Err(format!("{}: no tracking frames", path.display()));
    }
    Ok(out)
}

/// Load from a spec: a directory of tile PNGs, a tile prefix (`.../cam01_ctr49470_tile`), or
/// `FILE.pxcam[@FIRST[:COUNT[:STEP]]]`.
pub fn load(spec: &str) -> Result<Vec<Scene>, String> {
    let (file, rest) = match spec.rsplit_once('@') {
        Some((f, r)) if f.ends_with(".pxcam") => (f, Some(r)),
        _ => (spec, None),
    };
    if file.ends_with(".pxcam") {
        let nums: Vec<usize> = match rest {
            Some(r) => r
                .split(':')
                .map(|x| x.parse().map_err(|_| format!("bad frame spec in {spec}")))
                .collect::<Result<_, _>>()?,
            None => vec![],
        };
        let first = nums.first().copied().unwrap_or(0);
        let count = nums.get(1).copied().unwrap_or(1);
        let step = nums.get(2).copied().unwrap_or(1);
        return from_pxcam(Path::new(file), first, count, step);
    }
    let p = PathBuf::from(spec);
    if p.is_dir() {
        return from_dir(&p);
    }
    from_prefix(spec).map(|s| vec![s])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name_from("/x/cam01_ctr49470_tile"), "ctr49470");
        assert_eq!(name_from("/x/foo_tile"), "foo_tile");
        assert!(load("/nonexistent/prefix_tile").is_err());
        assert!(load("/nonexistent/x.pxcam@0:2").is_err());
        assert!(load("x.pxcam@a").is_err());
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn loads_the_fixture_recording() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../pimax-cam/tests/fixtures/evening2.pxcam");
        let s = load(&format!("{}@0:5", p.display())).unwrap();
        assert!(!s.is_empty());
        assert!(s
            .iter()
            .all(|x| x.tiles.len() == 4 && x.tiles[0].len() == TILE_W * TILE_H));
    }
}
