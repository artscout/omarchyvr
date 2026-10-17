//! Appearance: applies the app's colours choice (docs/GUI.md "Colours") to the running window and follows Omarchy.
//! Follow mode looks at the mtimes of Omarchy's `colors.toml` and `theme.name` about every 2 seconds (two `stat`
//! calls, no busy loop) and re-maps the palette only when one of them changed. Reading the files (the colours, the
//! list of installed themes) happens on a short-lived background thread: the UI thread never reads a file.
use crate::theme::{self, Colourway};
use eframe::egui;
use omarchyvr_gui_core::colors::{Colours, OmarchyPaths, Resolved, Stamp, ThemeEntry};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often Follow mode looks at Omarchy's files.
const POLL: Duration = Duration::from_secs(2);
/// How long the list of installed themes is kept before the folders are read again.
const THEMES_TTL: Duration = Duration::from_secs(10);

type Slot<T> = Arc<Mutex<Option<T>>>;

/// The colours state of the window.
pub struct Appearance {
    paths: OmarchyPaths,
    /// What is applied now: the choice and the Omarchy files' stamp.
    applied: Option<(Colours, Stamp)>,
    checked: Option<Instant>,
    /// A background read of the colours is running.
    resolving: bool,
    resolved: Slot<(Colours, Stamp, Resolved)>,
    /// One plain sentence about what is in use (the setting shows it).
    pub note: String,
    themes: Vec<ThemeEntry>,
    current: Option<String>,
    themes_at: Option<Instant>,
    loading: bool,
    loaded: Slot<(Vec<ThemeEntry>, Option<String>)>,
    /// Pictures only: colours to apply to the screenshot context (see `shots.rs`).
    pub demo: Option<Colourway>,
    /// Off in demo mode and in tests: the real Omarchy files are not looked at.
    pub enabled: bool,
}

impl Appearance {
    /// For the real window (`enabled`) or for demo data and tests (off).
    pub fn new(enabled: bool) -> Appearance {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        Appearance::with_paths(OmarchyPaths::from_home(&home), enabled)
    }

    /// With explicit Omarchy folders (tests, pictures).
    pub fn with_paths(paths: OmarchyPaths, enabled: bool) -> Appearance {
        Appearance {
            paths,
            applied: None,
            checked: None,
            resolving: false,
            resolved: Arc::default(),
            note: String::new(),
            themes: Vec::new(),
            current: None,
            themes_at: None,
            loading: false,
            loaded: Arc::default(),
            demo: None,
            enabled,
        }
    }

    /// Pictures: show this list of themes without reading anything.
    pub fn set_themes(&mut self, themes: Vec<ThemeEntry>, current: Option<String>) {
        self.themes = themes;
        self.current = current;
        self.themes_at = Some(Instant::now());
    }

    /// The Omarchy theme that was current when the themes were last read.
    pub fn omarchy_current(&self) -> Option<String> {
        self.current.clone()
    }

    /// The installed Omarchy themes with colour files. Read on a background thread, again after a few seconds; empty
    /// until the first read is done (the window repaints when it is).
    pub fn themes(&mut self, ctx: &egui::Context) -> &[ThemeEntry] {
        if let Some((t, c)) = self.loaded.lock().ok().and_then(|mut s| s.take()) {
            self.themes = t;
            self.current = c;
            self.loading = false;
        }
        if !self.loading && self.themes_at.is_none_or(|t| t.elapsed() > THEMES_TTL) {
            self.loading = true;
            self.themes_at = Some(Instant::now());
            let (paths, slot, ctx) = (self.paths.clone(), self.loaded.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = (paths.themes(), paths.current_name());
                if let Ok(mut s) = slot.lock() {
                    *s = Some(r);
                }
                ctx.request_repaint();
            });
        }
        &self.themes
    }

    /// Once per frame: apply the choice when it (or Omarchy's theme) changed.
    pub fn tick(&mut self, ctx: &egui::Context, choice: Colours) {
        if !self.enabled || !theme::installed(ctx) {
            return;
        }
        if let Some((c, stamp, r)) = self.resolved.lock().ok().and_then(|mut s| s.take()) {
            self.resolving = false;
            let cw = match &r.colors {
                Some(colors) => Colourway::omarchy(colors),
                None => Colourway::default_ways(ctx.global_style().visuals.dark_mode),
            };
            theme::set_colourway(ctx, &cw);
            self.note = r.note;
            self.applied = Some((c, stamp));
        }
        if self.resolving {
            return;
        }
        let following = choice == Colours::Follow;
        if self.applied.as_ref().is_some_and(|(c, _)| *c == choice) && (!following || self.checked.is_some_and(|t| t.elapsed() < POLL)) {
            return;
        }
        self.checked = Some(Instant::now());
        // while following, wake up in time for the next look even if nothing else repaints
        if following {
            ctx.request_repaint_after(POLL);
        }
        let stamp = if following { self.paths.stamp() } else { (None, None) };
        if self.applied.as_ref() == Some(&(choice, stamp)) {
            return;
        }
        self.resolving = true;
        let (paths, slot, ctx) = (self.paths.clone(), self.resolved.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = paths.resolve(choice);
            if let Ok(mut s) = slot.lock() {
                *s = Some((choice, stamp, r));
            }
            ctx.request_repaint();
        });
    }
}

/// Screenshots: apply the demo colours (if any) on the screenshot context.
pub fn apply_demo(ctx: &egui::Context, a: &Appearance) {
    if let Some(cw) = &a.demo {
        theme::install_with(ctx, cw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omarchyvr_gui_core::colors::OmarchyPaths;

    const TOKYO: &str = include_str!("../../fixtures/omarchy-themes/tokyo-night/colors.toml");
    const LATTE: &str = include_str!("../../fixtures/omarchy-themes/catppuccin-latte/colors.toml");

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-appearance-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("state/current/theme")).unwrap();
        d
    }

    fn ready_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install(&ctx, true);
        // the theme counts as installed from the pass after install
        for _ in 0..2 {
            let mut out = ctx.run_ui(Default::default(), |_| {});
            out.textures_delta.clear();
        }
        ctx
    }

    /// One tick, then wait for the background read and apply it.
    fn tick_wait(a: &mut Appearance, ctx: &egui::Context, c: Colours) {
        a.tick(ctx, c);
        for _ in 0..200 {
            if !a.resolving {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
            a.tick(ctx, c);
        }
        panic!("the background read did not finish");
    }

    fn paths_in(d: &std::path::Path) -> OmarchyPaths {
        OmarchyPaths { state: d.join("state"), user_themes: d.join("u"), system_themes: d.join("s") }
    }

    fn touch(f: &std::path::Path, secs: u64, base: std::time::SystemTime) {
        let f = std::fs::File::options().write(true).open(f).unwrap();
        f.set_modified(base + Duration::from_secs(secs)).unwrap();
    }

    #[test]
    fn follows_the_omarchy_theme_when_the_file_changes() {
        let d = dir("follow");
        let colors = d.join("state/current/theme/colors.toml");
        std::fs::write(&colors, TOKYO).unwrap();
        std::fs::write(d.join("state/current/theme.name"), "tokyo-night").unwrap();
        let mut a = Appearance::with_paths(paths_in(&d), true);
        let ctx = ready_ctx();
        tick_wait(&mut a, &ctx, Colours::Follow);
        assert!(a.note.contains("tokyo-night"), "{}", a.note);
        assert_eq!(theme::pal_ctx(&ctx).bg, egui::Color32::from_rgb(0x1a, 0x1b, 0x26));
        // within the poll time nothing is looked at again
        std::fs::write(&colors, LATTE).unwrap();
        let base = std::fs::metadata(&colors).unwrap().modified().unwrap();
        tick_wait(&mut a, &ctx, Colours::Follow);
        assert!(theme::pal_ctx(&ctx).dark);
        // after it, the change is picked up (the stamp changes with the new mtime)
        a.checked = Some(Instant::now() - Duration::from_secs(3));
        touch(&colors, 5, base);
        tick_wait(&mut a, &ctx, Colours::Follow);
        assert!(!theme::pal_ctx(&ctx).dark, "light theme: light palette");
        assert!(!ctx.global_style().visuals.dark_mode);
        // a broken file: back to the omarchyvr colours, with a note, no panic
        std::fs::write(&colors, "rubbish").unwrap();
        touch(&colors, 10, base);
        a.checked = Some(Instant::now() - Duration::from_secs(3));
        tick_wait(&mut a, &ctx, Colours::Follow);
        assert!(a.note.contains("could not be used"), "{}", a.note);
        assert_eq!(theme::pal_ctx(&ctx).bg, theme::LIGHT.bg);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn missing_file_and_default_choice() {
        let d = dir("missing");
        let _ = std::fs::remove_dir_all(d.join("state/current"));
        let mut a = Appearance::with_paths(paths_in(&d), true);
        let ctx = ready_ctx();
        tick_wait(&mut a, &ctx, Colours::Follow);
        assert!(a.note.contains("not found"), "{}", a.note);
        assert_eq!(theme::pal_ctx(&ctx), theme::DARK);
        tick_wait(&mut a, &ctx, Colours::Default);
        assert_eq!(theme::pal_ctx(&ctx), theme::DARK);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn theme_list_is_read_in_the_background() {
        let d = dir("list");
        std::fs::create_dir_all(d.join("s/tokyo-night")).unwrap();
        std::fs::write(d.join("s/tokyo-night/colors.toml"), TOKYO).unwrap();
        std::fs::write(d.join("state/current/theme.name"), "tokyo-night").unwrap();
        let mut a = Appearance::with_paths(paths_in(&d), true);
        let ctx = ready_ctx();
        assert!(a.themes(&ctx).is_empty(), "the first call only starts the read");
        for _ in 0..200 {
            if !a.themes(&ctx).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(a.themes(&ctx).len(), 1);
        assert_eq!(a.omarchy_current().as_deref(), Some("tokyo-night"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn disabled_does_nothing() {
        let mut a = Appearance::with_paths(OmarchyPaths { state: "/nonexistent".into(), user_themes: "/nonexistent".into(), system_themes: "/nonexistent".into() }, false);
        let ctx = ready_ctx();
        a.tick(&ctx, Colours::Follow);
        assert!(a.note.is_empty() && !a.resolving);
    }
}
