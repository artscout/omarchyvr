//! `pimax-lenstest preview`: every pattern and step as a PNG, without a headset (what the eye image looks like
//! before the lens model distorts it, with the on-screen text).

use crate::capture::file_stem;
use crate::capture::Task;
use crate::geometry::Fov;
use crate::image::default_threads;
use crate::patterns::{self, Dir, Params, Pattern};
use crate::report;
use crate::state::{EyeSelect, State};
use std::path::{Path, PathBuf};

/// Write one PNG per pattern step (left eye) and the first step of every pattern for the right eye.
pub fn write_all(
    dir: &Path,
    w: u32,
    h: u32,
    fov_deg: [f32; 4],
    square_deg: f32,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let fov = Fov::from_half_angles_deg(fov_deg[0], fov_deg[1], fov_deg[2], fov_deg[3]);
    let mut written = Vec::new();
    let mut index = 0usize;
    for (pi, pattern) in Pattern::ALL.into_iter().enumerate() {
        let mut p = Params::new(w, h, fov, 0);
        p.square_deg = square_deg;
        p.square_dir = Dir::Right;
        for step in 0..pattern.steps(&p) {
            for eye in 0..2usize {
                if eye == 1 && step != 0 {
                    continue;
                }
                p.eye = eye;
                p.step = step;
                let mut state = State::new(Pattern::ALL.to_vec());
                state.idx = pi;
                state.step = step;
                let hud = report::hud_lines(&state, &p);
                let img = patterns::render(pattern, &p, Some(&hud), default_threads());
                let mut stem = file_stem(
                    index,
                    &Task {
                        pattern,
                        step,
                        eye: EyeSelect::Both,
                    },
                    &pattern.step_name(&p, step),
                );
                if eye == 1 {
                    stem.push_str("-right-eye");
                }
                let path = dir.join(format!("{stem}.png"));
                pimax_lens::png::write_rgb8(&path, img.w, img.h, &img.rgb)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                written.push(path);
                index += 1;
            }
        }
    }
    Ok(written)
}

/// What the panel shows for every pattern step through the lens model (the compositor modelled on the CPU): both
/// eyes side by side in the orientation the user sees, `div` times smaller than the 3840 x 3552 display.
pub fn simulate_all(
    dir: &Path,
    lens: &pimax_lens::Lens,
    div: u32,
    client_div: u32,
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (pw, ph) = (3840 / div, 3552 / div);
    let (cw, ch) = (3840 / client_div, 3552 / client_div);
    let threads = default_threads();
    let mut written = Vec::new();
    let mut index = 0usize;
    for pattern in Pattern::ALL {
        let steps = {
            let d = lens.fov_deg(0);
            let mut p = Params::new(cw, ch, Fov::from_half_angles_deg(d[0], d[1], d[2], d[3]), 0);
            p.square_deg = crate::patterns::square::DEFAULT_SIZE_DEG;
            pattern.steps(&p)
        };
        for step in 0..steps {
            let mut eyes = Vec::new();
            let mut name = String::new();
            for eye in 0..2usize {
                let d = lens.fov_deg(eye);
                let fov = Fov::from_half_angles_deg(d[0], d[1], d[2], d[3]);
                let mut p = Params::new(cw, ch, fov, eye);
                p.step = step;
                name = pattern.step_name(&p, step);
                let client = patterns::render(pattern, &p, None, threads);
                eyes.push(crate::sim::distort(
                    lens, eye, &client, pw, ph, div, threads,
                ));
            }
            let img = crate::sim::side_by_side(&eyes[0], &eyes[1], (8 / div).max(2));
            let stem = file_stem(
                index,
                &Task {
                    pattern,
                    step,
                    eye: EyeSelect::Both,
                },
                &name,
            );
            let path = dir.join(format!("{stem}.png"));
            pimax_lens::png::write_rgb8(&path, img.w, img.h, &img.rgb)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            written.push(path);
            index += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulate_writes_one_picture_per_step() {
        let dir = std::env::temp_dir().join(format!("lenstest-sim-{}", std::process::id()));
        let lens = pimax_lens::Lens::new(pimax_lens::LensConfig::defaults(3840.0 / 3552.0));
        let files = simulate_all(&dir, &lens, 16, 8).unwrap();
        assert_eq!(files.len(), 1 + 1 + 4 + 3 + 6 + 6 + 1 + 1 + 1 + 7 + 1);
        for f in &files {
            let d = pimax_lens::png::decode(&std::fs::read(f).unwrap()).unwrap();
            assert_eq!((d.width, d.height), (240 + 2 + 240, 222), "{}", f.display());
        }
        assert!(simulate_all(Path::new("/proc/nope/x"), &lens, 16, 8).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_every_step_and_the_right_eye() {
        let dir = std::env::temp_dir().join(format!("lenstest-preview-{}", std::process::id()));
        let files = write_all(&dir, 200, 180, [50.0, 50.0, 44.5, 44.5], 5.0).unwrap();
        // grid 1+1, rings 1+1, channels 4+1, fov-edge 3+1, square100 6+1, square25 6+1, corners 1+1, align 1+1, swim 1+1, flat 7+1, gaze 1+1
        assert_eq!(files.len(), 2 + 2 + 5 + 4 + 7 + 7 + 2 + 2 + 2 + 8 + 2);
        for f in &files {
            let d = pimax_lens::png::decode(&std::fs::read(f).unwrap()).unwrap();
            assert_eq!((d.width, d.height), (200, 180), "{}", f.display());
        }
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n.contains("channels-red-only")));
        assert!(names.iter().any(|n| n.contains("right-eye")));
        // all different
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        assert!(write_all(Path::new("/proc/nope/x"), 200, 180, [50.0; 4], 5.0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
