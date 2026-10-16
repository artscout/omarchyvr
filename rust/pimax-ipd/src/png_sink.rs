//! A [`Sink`] that writes every distinct screen to `NN.png` (for `--simulate` and tests).

use omarchyvr_hud::{Screen, Sink};
use std::path::PathBuf;
use std::sync::Mutex;

pub struct PngSink {
    dir: PathBuf,
    state: Mutex<(u32, Option<Screen>)>,
}

impl PngSink {
    pub fn new(dir: PathBuf) -> std::io::Result<PngSink> {
        std::fs::create_dir_all(&dir)?;
        Ok(PngSink { dir, state: Mutex::new((0, None)) })
    }

    pub fn count(&self) -> u32 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).0
    }
}

impl Sink for PngSink {
    fn show(&self, screen: Screen) {
        let mut g = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Progress-only changes are collapsed to roughly every 25 %.
        if let Some(prev) = &g.1 {
            let same_text = prev.lines == screen.lines;
            let near = match (prev.progress, screen.progress) {
                (Some(a), Some(b)) => (a - b).abs() < 0.25 && b < 0.99,
                (a, b) => a == b,
            };
            if same_text && near {
                return;
            }
        }
        g.0 += 1;
        let p = self.dir.join(format!("{:02}.png", g.0));
        if let Err(e) = omarchyvr_hud::screen::write_png(&screen, &p) {
            eprintln!("cannot write {}: {e}", p.display());
        }
        g.1 = Some(screen);
    }
}
