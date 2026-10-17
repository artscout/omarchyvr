//! Every path the app looks at or launches, derived from `$HOME` (and a few overrides). One place, so the checks, the
//! tools and the launcher plan agree. Defaults follow docs/INVENTORY.md and docs/LESSONS.md row 16-17.
use std::path::{Path, PathBuf};

/// Where a package puts the docs, scripts and udev rule (`docs/`, `analysis/tools/`, `linux/`).
pub const SYSTEM_DATA: &str = "/usr/share/omarchyvr";

/// The doctor's options for these paths: a package install reads the patches, hooks and udev rule from
/// `/usr/share/omarchyvr/linux` (unless `PIMAX_LINUX_DIR` says otherwise).
pub fn doctor_opts(p: &Paths, env: &dyn Fn(&str) -> Option<String>) -> omarchyvr_doctor::Opts {
    let mut o = omarchyvr_doctor::Opts::from_env(env);
    if p.repo == Path::new(SYSTEM_DATA) && env("PIMAX_LINUX_DIR").filter(|v| !v.is_empty()).is_none() {
        o.linux_dir = p.repo.join("linux");
    }
    o
}

/// Runs the app from a package (`/usr/bin`, with [`SYSTEM_DATA`] installed)?
pub fn is_system_install(exe: &Path) -> bool {
    exe.starts_with("/usr/bin") && Path::new(SYSTEM_DATA).join("docs/LESSONS.md").is_file()
}

/// One Monado build the app knows (LESSONS row 7: every build the app can start is checked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDir {
    /// Short key (`desk`, `slam`, `omarchyvr`).
    pub key: &'static str,
    /// Human label.
    pub label: &'static str,
    /// Build directory (contains `CMakeCache.txt`).
    pub dir: PathBuf,
}

impl BuildDir {
    /// `<dir>/src/xrt/targets/service/monado-service`.
    pub fn service(&self) -> PathBuf {
        service_of(&self.dir)
    }
}

/// The service binary of a build directory.
pub fn service_of(build: &Path) -> PathBuf {
    build.join("src/xrt/targets/service/monado-service")
}

/// All paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `$HOME`.
    pub home: PathBuf,
    /// Source/build trees of Monado, WayVR, Basalt, xrizer (`~/src`; `src_dir` in `~/.config/omarchyvr/paths.toml` or
    /// `PIMAX_SRC_DIR`).
    pub src: PathBuf,
    /// The repository (`PIMAX_REPO`, else the installed data copy `~/.local/share/omarchyvr/data`, else `~/omarchyvr`).
    pub repo: PathBuf,
    /// `<repo>/rust`.
    pub rust: PathBuf,
    /// `<repo>/rust/target/release`.
    pub release: PathBuf,
    /// Launcher session directory (`/tmp/omarchyvr-session` or `PIMAX_VR_SESSION_DIR`).
    pub session_dir: PathBuf,
    /// `~/.config/omarchyvr`.
    pub config: PathBuf,
    /// `~/.local/share/omarchyvr/slam-recordings`.
    pub slam_recordings: PathBuf,
    /// Installed launcher (`~/.local/bin/omarchyvr`).
    pub launcher_installed: PathBuf,
    /// Launcher build (`<release>/pimax`).
    pub launcher_built: PathBuf,
    /// Installed app binary (`~/.local/bin/omarchyvr-app`).
    pub app_installed: PathBuf,
    /// App build (`<release>/omarchyvr-app`).
    pub app_built: PathBuf,
    /// Desktop entry (`~/.local/share/applications/omarchyvr.desktop`).
    pub desktop_entry: PathBuf,
    /// Icon (`~/.local/share/icons/hicolor/scalable/apps/omarchyvr.svg`).
    pub icon: PathBuf,
    /// Monado builds, the main desk build first.
    pub builds: Vec<BuildDir>,
    /// WayVR binary (`~/.local/lib/omarchyvr/wayvr/wayvr`, else `~/.local/lib/omarchyvr/wayvr/wayvr`).
    pub wayvr_bin: PathBuf,
    /// WayVR source tree (`~/src/wayvr`).
    pub wayvr_src: PathBuf,
    /// Basalt (`~/.local/lib/omarchyvr/basalt/lib/libbasalt.so`, else `~/.local/lib/libbasalt.so`).
    pub basalt_lib: PathBuf,
    /// Parakeet model directory.
    pub voice_model: PathBuf,
    /// Silero VAD model.
    pub vad_model: PathBuf,
    /// sherpa-onnx C API library (default of omarchyvr-voice; `~/.local/lib/omarchyvr/sherpa-onnx/lib/`, else the old venv).
    pub sherpa_lib: PathBuf,
    /// The headset lock of the device tools.
    pub headset_lock: PathBuf,
    /// `~/.config/openvr/openvrpaths.vrpath`.
    pub openvr_paths: PathBuf,
    /// `~/.config/omarchyvr/ipd.env` (LESSONS row 18).
    pub ipd_env: PathBuf,
    /// `~/.config/omarchyvr/eye-calibration.toml` (LESSONS row 18).
    pub eye_calibration: PathBuf,
    /// App state (`~/.config/omarchyvr/gui-state.toml`: wizard done, last toggles).
    pub gui_state: PathBuf,
    /// `$XDG_RUNTIME_DIR` (or `/run/user/<uid>`).
    pub runtime_dir: PathBuf,
    /// The app's own state: logs, crash reports, problem reports, job outputs (`$XDG_STATE_HOME/omarchyvr`, else
    /// `~/.local/state/omarchyvr`).
    pub state_dir: PathBuf,
}

impl Paths {
    /// From the environment.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Paths {
        let get = |k: &str| env(k).filter(|v| !v.is_empty());
        let home = PathBuf::from(get("HOME").unwrap_or_else(|| "/root".into()));
        let repo = get("PIMAX_REPO").map(PathBuf::from).unwrap_or_else(|| {
            let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            if built.join("docs/LESSONS.md").is_file() {
                built.canonicalize().unwrap_or(built)
            } else {
                omarchyvr_config::paths::repo_dir(&home)
            }
        });
        let session_dir = get("PIMAX_VR_SESSION_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(omarchyvr_tools::vr::config::SESSION_DIR));
        let runtime_dir = get("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/run/user/1000"));
        // a package install (/usr/bin/omarchyvr-app with /usr/share/omarchyvr): docs and scripts from there, the tools
        // in /usr/bin; the Monado and WayVR builds stay in the user's ~/src
        let exe = std::env::current_exe().ok();
        let system = get("PIMAX_REPO").is_none() && exe.as_deref().is_some_and(is_system_install);
        let repo = if system { PathBuf::from(SYSTEM_DATA) } else { repo };
        let mut p = Paths::new(&home, &repo, &session_dir, &runtime_dir);
        // the source trees: paths.toml / PIMAX_SRC_DIR (omarchyvr_config::paths), default ~/src
        let src = get("PIMAX_SRC_DIR").map(PathBuf::from).unwrap_or_else(|| omarchyvr_config::paths::src_dir(&home));
        if src != p.src {
            p.set_src(&src);
        }
        if system {
            p.as_system_install();
        }
        p.state_dir = crate::applog::state_dir(env);
        // a build the launcher is told to use (PIMAX_MONADO_BUILD) is checked like the known ones (LESSONS row 7)
        if let Some(b) = get("PIMAX_MONADO_BUILD").map(PathBuf::from) {
            if !p.builds.iter().any(|x| x.dir == b) {
                p.builds.push(BuildDir {
                    key: "custom",
                    label: "Monado (PIMAX_MONADO_BUILD)",
                    dir: b,
                });
            }
        }
        p
    }

    /// Explicit roots (tests).
    pub fn new(home: &Path, repo: &Path, session_dir: &Path, runtime_dir: &Path) -> Paths {
        Paths::new_with(home, repo, session_dir, runtime_dir, &|p| p.exists())
    }

    /// [`Paths::new`] with an injectable `exists` (which installed files are there: the installed Monado build, WayVR, ...).
    pub fn new_with(home: &Path, repo: &Path, session_dir: &Path, runtime_dir: &Path, exists: &dyn Fn(&Path) -> bool) -> Paths {
        use omarchyvr_config::paths as cp;
        let rust = repo.join("rust");
        // the tools of an installed set (install.sh puts every binary in ~/.local/bin) win over a repo build:
        // after a clean install there is no rust/target/release and the room setup said "not built" (2026-10-17)
        let installed = home.join(".local/bin");
        let release = if exists(&installed.join("omarchyvr")) { installed } else { rust.join("target/release") };
        let src = home.join("src");
        Paths {
            src: src.clone(),
            home: home.to_path_buf(),
            repo: repo.to_path_buf(),
            rust: rust.clone(),
            release: release.clone(),
            session_dir: session_dir.to_path_buf(),
            config: home.join(".config/omarchyvr"),
            slam_recordings: home.join(".local/share/omarchyvr/slam-recordings"),
            launcher_installed: home.join(".local/bin/omarchyvr"),
            launcher_built: rust.join("target/release/omarchyvr"),
            app_installed: home.join(".local/bin/omarchyvr-app"),
            app_built: rust.join("target/release/omarchyvr-app"),
            desktop_entry: home.join(".local/share/applications/omarchyvr.desktop"),
            icon: home.join(".local/share/icons/hicolor/scalable/apps/omarchyvr.svg"),
            builds: vec![
                BuildDir {
                    key: "desk",
                    label: "Monado desk (eye + hands + 6DoF, main build)",
                    dir: cp::monado_build_with(home, exists),
                },
                BuildDir {
                    key: "slam",
                    label: "Monado slam (6DoF branch)",
                    dir: src.join("monado-slam/build"),
                },
                BuildDir {
                    key: "pimax",
                    label: "Monado pimax (3DoF, plain VR)",
                    dir: src.join("monado/build"),
                },
            ],
            wayvr_bin: cp::wayvr_bin_with(home, exists),
            wayvr_src: src.join("wayvr"),
            basalt_lib: cp::basalt_lib_with(home, exists),
            voice_model: home.join("models/parakeet-tdt-0.6b-v3-int8"),
            vad_model: home.join("models/vad/silero_vad.onnx"),
            sherpa_lib: cp::sherpa_lib_with(home, exists),
            headset_lock: PathBuf::from("/tmp/omarchyvr-headset.lock"),
            openvr_paths: home.join(".config/openvr/openvrpaths.vrpath"),
            ipd_env: home.join(".config/omarchyvr/ipd.env"),
            eye_calibration: home.join(".config/omarchyvr/eye-calibration.toml"),
            gui_state: home.join(".config/omarchyvr/gui-state.toml"),
            runtime_dir: runtime_dir.to_path_buf(),
            state_dir: home.join(".local/state/omarchyvr"),
        }
    }

    /// Moves every path under the source root to `src` (the builds, WayVR, the sherpa library).
    pub fn set_src(&mut self, src: &Path) {
        let old = self.src.clone();
        let mv = |x: &mut PathBuf| {
            if let Ok(rest) = x.strip_prefix(&old) {
                *x = src.join(rest);
            }
        };
        for b in &mut self.builds {
            mv(&mut b.dir);
        }
        mv(&mut self.wayvr_bin);
        mv(&mut self.wayvr_src);
        mv(&mut self.sherpa_lib);
        self.src = src.to_path_buf();
    }

    /// Paths of a package install: binaries in `/usr/bin`, docs and scripts in [`SYSTEM_DATA`], the desktop entry
    /// and icon of the package.
    pub fn as_system_install(&mut self) {
        let bin = PathBuf::from("/usr/bin");
        self.release = bin.clone();
        self.launcher_installed = bin.join("omarchyvr");
        self.launcher_built = bin.join("omarchyvr");
        self.app_installed = bin.join("omarchyvr-app");
        self.app_built = bin.join("omarchyvr-app");
        self.desktop_entry = PathBuf::from("/usr/share/applications/omarchyvr.desktop");
        self.icon = PathBuf::from("/usr/share/icons/hicolor/scalable/apps/omarchyvr.svg");
    }

    /// The main desk build (LESSONS row 16: `~/.local/lib/omarchyvr/monado` of the installed set, else the lab's
    /// `~/.local/lib/omarchyvr/monado`, branch `desk`).
    pub fn desk_build(&self) -> &BuildDir {
        &self.builds[0]
    }

    /// A build by key.
    pub fn build(&self, key: &str) -> Option<&BuildDir> {
        self.builds.iter().find(|b| b.key == key)
    }

    /// A release binary of the workspace.
    pub fn bin(&self, name: &str) -> PathBuf {
        self.release.join(name)
    }

    /// An installed binary: `~/.local/bin/<name>`.
    pub fn installed_bin(&self, name: &str) -> PathBuf {
        self.home.join(".local/bin").join(name)
    }

    /// `<repo>/analysis/tools/<script>`.
    pub fn script(&self, name: &str) -> PathBuf {
        self.repo.join("analysis/tools").join(name)
    }

    /// `<repo>/docs/<name>`.
    pub fn doc(&self, name: &str) -> PathBuf {
        self.repo.join("docs").join(name)
    }

    /// The launcher the app calls: the installed one if present, else the workspace build.
    pub fn launcher(&self, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
        if exists(&self.launcher_installed) {
            self.launcher_installed.clone()
        } else {
            self.launcher_built.clone()
        }
    }

    /// The camera broker socket (as `pimax --6dof` and `run_slam_view.sh` use it).
    pub fn broker_socket(&self) -> PathBuf {
        self.runtime_dir.join("omarchyvr-cam-broker.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_layout_wins_over_the_lab_build() {
        let (h, repo, s, r) = (Path::new("/home/u"), Path::new("/home/u/omarchyvr"), Path::new("/s"), Path::new("/r"));
        let none = |_: &Path| false;
        let p = Paths::new_with(h, repo, s, r, &none);
        assert_eq!(p.desk_build().dir, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"), "legacy fallback");
        assert_eq!(p.wayvr_bin, PathBuf::from("/home/u/.local/lib/omarchyvr/wayvr/wayvr"));
        assert_eq!(p.basalt_lib, PathBuf::from("/home/u/.local/lib/libbasalt.so"));
        let installed = |f: &Path| f.starts_with("/home/u/.local/lib/omarchyvr");
        let p = Paths::new_with(h, repo, s, r, &installed);
        assert_eq!(p.desk_build().dir, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"));
        assert_eq!(
            p.desk_build().service(),
            PathBuf::from("/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service")
        );
        assert_eq!(p.wayvr_bin, PathBuf::from("/home/u/.local/lib/omarchyvr/wayvr/wayvr"));
        assert_eq!(p.basalt_lib, PathBuf::from("/home/u/.local/lib/omarchyvr/basalt/lib/libbasalt.so"));
        assert_eq!(p.sherpa_lib, PathBuf::from("/home/u/.local/lib/omarchyvr/sherpa-onnx/lib/libsherpa-onnx-c-api.so"));
        // only the build dir, without its CMakeCache.txt: still the lab build
        let dir_only = |f: &Path| f == Path::new("/home/u/.local/lib/omarchyvr/monado");
        assert_eq!(Paths::new_with(h, repo, s, r, &dir_only).desk_build().dir, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"));
    }

    #[test]
    fn defaults_follow_the_inventory() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "PIMAX_REPO" => Some("/home/u/omarchyvr".to_string()),
            _ => None,
        };
        let p = Paths::from_env(&env);
        assert_eq!(p.session_dir, PathBuf::from("/tmp/omarchyvr-session"));
        assert_eq!(p.desk_build().dir, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"));
        assert_eq!(
            p.desk_build().service(),
            PathBuf::from("/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service")
        );
        assert_eq!(p.builds.len(), 3);
        assert_eq!(p.bin("pimax-eye"), PathBuf::from("/home/u/omarchyvr/rust/target/release/pimax-eye"));
        assert_eq!(p.script("run_slam_view.sh"), PathBuf::from("/home/u/omarchyvr/analysis/tools/run_slam_view.sh"));
        assert_eq!(p.launcher(&|_| false), PathBuf::from("/home/u/omarchyvr/rust/target/release/omarchyvr"));
        assert_eq!(p.launcher(&|_| true), PathBuf::from("/home/u/.local/bin/omarchyvr"));
        assert_eq!(p.broker_socket(), PathBuf::from("/run/user/1000/omarchyvr-cam-broker.sock"));
        let env2 = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "PIMAX_VR_SESSION_DIR" => Some("/s".to_string()),
            "XDG_RUNTIME_DIR" => Some("/run/user/7".to_string()),
            _ => None,
        };
        let p = Paths::from_env(&env2);
        assert_eq!(p.session_dir, PathBuf::from("/s"));
        assert_eq!(p.runtime_dir, PathBuf::from("/run/user/7"));
        assert_eq!(p.state_dir, PathBuf::from("/home/u/.local/state/omarchyvr"));
        let env3 = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "PIMAX_MONADO_BUILD" => Some("/opt/monado/build".to_string()),
            "XDG_STATE_HOME" => Some("/st".to_string()),
            _ => None,
        };
        let p = Paths::from_env(&env3);
        assert_eq!(p.builds.len(), 4, "the custom build is inspected too");
        assert_eq!(p.build("custom").unwrap().dir, PathBuf::from("/opt/monado/build"));
        assert_eq!(p.state_dir, PathBuf::from("/st/omarchyvr"));
        let mut p = Paths::new(Path::new("/home/u"), Path::new(SYSTEM_DATA), Path::new("/s"), Path::new("/r"));
        p.as_system_install();
        assert_eq!(p.launcher(&|_| true), PathBuf::from("/usr/bin/omarchyvr"));
        assert_eq!(p.bin("pimax-eye"), PathBuf::from("/usr/bin/pimax-eye"));
        assert_eq!(p.doc("LESSONS.md"), PathBuf::from("/usr/share/omarchyvr/docs/LESSONS.md"));
        assert_eq!(p.desk_build().dir, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"), "builds stay in ~/src");
        assert!(!is_system_install(Path::new("/home/u/.local/bin/omarchyvr-app")));
    }

    #[test]
    fn an_installed_set_finds_its_tools_in_local_bin() {
        let home = Path::new("/home/u");
        let repo = Path::new("/home/u/omarchyvr");
        let installed = |p: &Path| p == Path::new("/home/u/.local/bin/omarchyvr");
        let p = Paths::new_with(home, repo, Path::new("/s"), Path::new("/r"), &installed);
        assert_eq!(p.bin("omarchyvr-roommap"), PathBuf::from("/home/u/.local/bin/omarchyvr-roommap"));
        assert_eq!(p.launcher_built, PathBuf::from("/home/u/omarchyvr/rust/target/release/omarchyvr"), "the repo build stays the install source");
        let p = Paths::new_with(home, repo, Path::new("/s"), Path::new("/r"), &|_| false);
        assert_eq!(p.bin("omarchyvr-roommap"), PathBuf::from("/home/u/omarchyvr/rust/target/release/omarchyvr-roommap"));
    }
}
