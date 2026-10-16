//! Capture mode: show every pattern and step, ask the compositor for a dump of its final output when the pattern has
//! been on screen long enough (trigger file, see Monado `XRT_COMPOSITOR_DUMP_TRIGGER`), convert the dump to a small
//! PNG in the background and go on.

use crate::dump;
use crate::patterns::Pattern;
use crate::state::EyeSelect;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    pub pattern: Pattern,
    pub step: usize,
    pub eye: EyeSelect,
}

/// Every pattern with every step on both eyes, plus the rings once with only the left and once with only the right
/// eye lit (checks the eye select and shows the unlit eye is black).
pub fn plan(patterns: &[Pattern], steps_of: &dyn Fn(Pattern) -> usize) -> Vec<Task> {
    let mut v = Vec::new();
    for &pattern in patterns {
        for step in 0..steps_of(pattern).max(1) {
            v.push(Task {
                pattern,
                step,
                eye: EyeSelect::Both,
            });
        }
        if pattern == Pattern::Rings {
            v.push(Task {
                pattern,
                step: 0,
                eye: EyeSelect::Left,
            });
            v.push(Task {
                pattern,
                step: 0,
                eye: EyeSelect::Right,
            });
        }
    }
    v
}

/// `07-channels-red-only`, `12-rings-left-eye`: index, pattern, step name, eye.
pub fn file_stem(index: usize, task: &Task, step_name: &str) -> String {
    let mut s = format!("{:02}-{}", index + 1, task.pattern.name());
    let step: String = step_name
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if !step.is_empty() {
        s.push('-');
        s.push_str(&step);
    }
    if task.eye != EyeSelect::Both {
        s.push_str(&format!("-{}-eye", task.eye.name()));
    }
    s
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Waiting for the pattern to settle or for the dump to appear.
    Idle,
    /// The trigger file was just created.
    Triggered,
    /// A dump was found and handed to a worker; continue with the next task (or finish).
    Captured(String),
    /// The compositor did not answer in time; the task is skipped.
    TimedOut(String),
}

enum Phase {
    Settling,
    Waiting(Instant),
}

pub struct Capture {
    pub tasks: Vec<Task>,
    idx: usize,
    names: Vec<String>,
    trigger: PathBuf,
    dump_dir: PathBuf,
    out_dir: PathBuf,
    settle: u32,
    factor: u32,
    phase: Phase,
    known: HashSet<String>,
    workers: Vec<(String, JoinHandle<Result<u64, String>>)>,
    keep_raw: bool,
    pub timeout: Duration,
}

fn list_raw(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| dump::parse_dump_name(n).is_some())
                .collect()
        })
        .unwrap_or_default()
}

impl Capture {
    /// `names[i]` is the file stem of `tasks[i]`. Files already in `dump_dir` are ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tasks: Vec<Task>,
        names: Vec<String>,
        trigger: PathBuf,
        dump_dir: PathBuf,
        out_dir: PathBuf,
        settle: u32,
        factor: u32,
        keep_raw: bool,
    ) -> std::io::Result<Capture> {
        assert_eq!(tasks.len(), names.len());
        std::fs::create_dir_all(&out_dir)?;
        let known = list_raw(&dump_dir).into_iter().collect();
        let _ = std::fs::remove_file(&trigger);
        Ok(Capture {
            tasks,
            idx: 0,
            names,
            trigger,
            dump_dir,
            out_dir,
            settle,
            factor,
            phase: Phase::Settling,
            known,
            workers: Vec::new(),
            keep_raw,
            timeout: Duration::from_secs(10),
        })
    }

    pub fn current(&self) -> Option<&Task> {
        self.tasks.get(self.idx)
    }

    pub fn index(&self) -> usize {
        self.idx
    }

    pub fn current_name(&self) -> Option<&str> {
        self.names.get(self.idx).map(String::as_str)
    }

    pub fn done(&self) -> bool {
        self.idx >= self.tasks.len()
    }

    /// Call once per displayed frame. `frames_shown` = frames since the current task's images were completely
    /// uploaded (all swapchain images of both eyes).
    pub fn frame(&mut self, frames_shown: u32, now: Instant) -> Step {
        if self.done() {
            return Step::Idle;
        }
        match self.phase {
            Phase::Settling => {
                if frames_shown >= self.settle {
                    if std::fs::write(&self.trigger, b"").is_err() {
                        return self.skip(format!(
                            "{}: cannot create the trigger file {}",
                            self.names[self.idx],
                            self.trigger.display()
                        ));
                    }
                    self.phase = Phase::Waiting(now);
                    return Step::Triggered;
                }
                Step::Idle
            }
            Phase::Waiting(since) => {
                if !self.trigger.exists() {
                    // the compositor consumed the trigger: its dump is in the directory
                    let new: Vec<String> = list_raw(&self.dump_dir)
                        .into_iter()
                        .filter(|n| !self.known.contains(n))
                        .collect();
                    let newest = new
                        .iter()
                        .max_by_key(|n| dump::parse_dump_name(n).map_or(0, |i| i.frame))
                        .cloned();
                    return match newest {
                        Some(file) => {
                            self.known.extend(new);
                            let name = self.names[self.idx].clone();
                            let raw = self.dump_dir.join(&file);
                            let png = self.out_dir.join(format!("{name}.png"));
                            let (factor, keep) = (self.factor, self.keep_raw);
                            let kept = self.out_dir.join(format!("{name}_{file}"));
                            self.workers.push((
                                name.clone(),
                                std::thread::spawn(move || {
                                    let r = dump::convert(&raw, &png, factor, 90, 270).map(|c| c.checksum);
                                    if keep {
                                        let _ = std::fs::rename(&raw, &kept).or_else(|_| std::fs::copy(&raw, &kept).and_then(|_| std::fs::remove_file(&raw)));
                                    } else {
                                        let _ = std::fs::remove_file(&raw);
                                    }
                                    r
                                }),
                            ));
                            self.idx += 1;
                            self.phase = Phase::Settling;
                            Step::Captured(name)
                        }
                        None => self.skip(format!("{}: the compositor removed the trigger but wrote no dump (is XRT_COMPOSITOR_DUMP_DIR set to {}?)", self.names[self.idx], self.dump_dir.display())),
                    };
                }
                if now.duration_since(since) > self.timeout {
                    let _ = std::fs::remove_file(&self.trigger);
                    return self.skip(format!("{}: no dump after {:?} (monado-service needs XRT_COMPOSITOR_DUMP_DIR and XRT_COMPOSITOR_DUMP_TRIGGER={})", self.names[self.idx], self.timeout, self.trigger.display()));
                }
                Step::Idle
            }
        }
    }

    fn skip(&mut self, why: String) -> Step {
        self.idx += 1;
        self.phase = Phase::Settling;
        Step::TimedOut(why)
    }

    /// Wait for the conversions; one entry per captured pattern.
    pub fn finish(self) -> Vec<(String, Result<u64, String>)> {
        self.workers
            .into_iter()
            .map(|(n, h)| {
                (
                    n,
                    h.join()
                        .unwrap_or_else(|_| Err("converter thread panicked".into())),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(p: Pattern) -> usize {
        match p {
            Pattern::Channels => 4,
            Pattern::FovEdge => 3,
            Pattern::Square100 | Pattern::Square25 => 6,
            _ => 1,
        }
    }

    #[test]
    fn plan_covers_every_step_and_the_single_eye_rings() {
        let t = plan(&Pattern::ALL, &steps);
        assert_eq!(t.len(), 1 + 1 + 2 + 4 + 3 + 6 + 6 + 1 + 1 + 1 + 1 + 1);
        assert_eq!(t.iter().filter(|x| x.pattern == Pattern::Rings).count(), 3);
        assert!(t
            .iter()
            .any(|x| x.pattern == Pattern::Rings && x.eye == EyeSelect::Left));
        assert!(t
            .iter()
            .any(|x| x.pattern == Pattern::Rings && x.eye == EyeSelect::Right));
        assert_eq!(
            t[0],
            Task {
                pattern: Pattern::Grid,
                step: 0,
                eye: EyeSelect::Both
            }
        );
        // without rings there are no single-eye tasks
        let u = plan(&[Pattern::Grid, Pattern::Corners], &steps);
        assert_eq!(u.len(), 2);
        assert_eq!(plan(&[Pattern::Corners], &|_| 0).len(), 1);
    }

    #[test]
    fn file_stems() {
        let t = |pattern, step, eye| Task { pattern, step, eye };
        assert_eq!(
            file_stem(0, &t(Pattern::Grid, 0, EyeSelect::Both), ""),
            "01-grid"
        );
        assert_eq!(
            file_stem(6, &t(Pattern::Channels, 1, EyeSelect::Both), "RED ONLY"),
            "07-channels-red-only"
        );
        assert_eq!(
            file_stem(
                11,
                &t(Pattern::Square25, 5, EyeSelect::Both),
                "EDGE 47.5 DEG RIGHT"
            ),
            "12-square25-edge-47.5-deg-right"
        );
        assert_eq!(
            file_stem(2, &t(Pattern::Rings, 0, EyeSelect::Left), ""),
            "03-rings-left-eye"
        );
        assert_eq!(
            file_stem(3, &t(Pattern::Channels, 0, EyeSelect::Both), "ALL (WHITE)"),
            "04-channels-all-white"
        );
    }

    /// A fake compositor: removes the trigger and writes a small BGRA dump.
    fn fake_dump(dir: &Path, frame: u32, rgb: [u8; 3]) {
        let mut raw = Vec::new();
        for _ in 0..8 * 4 {
            raw.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
        std::fs::write(dir.join(format!("frame{frame}_8x4_fmt44.raw")), raw).unwrap();
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lenstest-cap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn full_cycle_with_a_fake_compositor() {
        let root = tmp("cycle");
        let (dumps, out) = (root.join("dumps"), root.join("png"));
        std::fs::create_dir_all(&dumps).unwrap();
        fake_dump(&dumps, 1, [1, 2, 3]); // stale dump from an earlier run: must be ignored
        let trigger = root.join("trigger");
        let tasks = vec![
            Task {
                pattern: Pattern::Grid,
                step: 0,
                eye: EyeSelect::Both,
            },
            Task {
                pattern: Pattern::Corners,
                step: 0,
                eye: EyeSelect::Both,
            },
        ];
        let names = vec!["01-grid".to_string(), "02-corners".to_string()];
        let mut cap = Capture::new(
            tasks,
            names,
            trigger.clone(),
            dumps.clone(),
            out.clone(),
            5,
            2,
            false,
        )
        .unwrap();
        let t0 = Instant::now();
        assert_eq!(cap.current().unwrap().pattern, Pattern::Grid);
        // not settled: nothing happens
        for f in 0..5 {
            assert_eq!(cap.frame(f, t0), Step::Idle);
            assert!(!trigger.exists());
        }
        assert_eq!(cap.frame(5, t0), Step::Triggered);
        assert!(trigger.exists());
        assert_eq!(
            cap.frame(6, t0),
            Step::Idle,
            "still waiting for the compositor"
        );
        // the compositor answers
        fake_dump(&dumps, 17, [200, 10, 10]);
        std::fs::remove_file(&trigger).unwrap();
        assert_eq!(cap.frame(7, t0), Step::Captured("01-grid".to_string()));
        assert_eq!(cap.current().unwrap().pattern, Pattern::Corners);
        // second task
        assert_eq!(cap.frame(5, t0), Step::Triggered);
        fake_dump(&dumps, 40, [10, 200, 10]);
        std::fs::remove_file(&trigger).unwrap();
        assert_eq!(cap.frame(6, t0), Step::Captured("02-corners".to_string()));
        assert!(cap.done());
        assert_eq!(cap.frame(100, t0), Step::Idle);
        let results = cap.finish();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
        // PNGs exist, raw dumps were removed (the stale one stays), the second PNG is greener than the first
        let a = pimax_lens::png::decode(&std::fs::read(out.join("01-grid.png")).unwrap()).unwrap();
        let b =
            pimax_lens::png::decode(&std::fs::read(out.join("02-corners.png")).unwrap()).unwrap();
        assert_eq!((a.width, a.height), (2 + 4 + 2, 2)); // 8x4 dump /2 = 4x2: two 2x2 eyes, gap max(8/2, 2) = 4
        assert!(a.data[0] > 150 && b.data[1] > 150);
        let left: Vec<String> = list_raw(&dumps);
        assert_eq!(left, vec!["frame1_8x4_fmt44.raw".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_compositor_times_out_and_moves_on() {
        let root = tmp("timeout");
        let trigger = root.join("trigger");
        let tasks = vec![
            Task {
                pattern: Pattern::Grid,
                step: 0,
                eye: EyeSelect::Both,
            },
            Task {
                pattern: Pattern::Rings,
                step: 0,
                eye: EyeSelect::Both,
            },
        ];
        let mut cap = Capture::new(
            tasks,
            vec!["a".into(), "b".into()],
            trigger.clone(),
            root.clone(),
            root.join("o"),
            0,
            2,
            false,
        )
        .unwrap();
        cap.timeout = Duration::from_millis(50);
        let t0 = Instant::now();
        assert_eq!(cap.frame(0, t0), Step::Triggered);
        match cap.frame(0, t0 + Duration::from_millis(100)) {
            Step::TimedOut(why) => assert!(why.contains("XRT_COMPOSITOR_DUMP_TRIGGER"), "{why}"),
            s => panic!("{s:?}"),
        }
        assert!(!trigger.exists(), "the stale trigger is removed");
        assert_eq!(cap.index(), 1);
        // the compositor removes the trigger but writes nothing
        assert_eq!(cap.frame(0, t0), Step::Triggered);
        std::fs::remove_file(&trigger).unwrap();
        match cap.frame(0, t0) {
            Step::TimedOut(why) => assert!(why.contains("wrote no dump"), "{why}"),
            s => panic!("{s:?}"),
        }
        assert!(cap.done());
        assert!(cap.finish().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
