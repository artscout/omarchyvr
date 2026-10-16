//! Every path the app looks at or launches, derived from `$HOME` (and a few overrides). One place, so the checks, the
//! tools and the launcher plan agree. Defaults follow docs/INVENTORY.md and docs/LESSONS.md row 16-17.
use std::path::{Path, PathBuf};

/// Where a package puts the docs, scripts and udev rule (`docs/`, `analysis/tools/`, `linux/`).
pub const SYSTEM_DATA: &str = "/usr/share/pimax";

/// The doctor's options for these paths: a package install reads the patches, hooks and udev rule from
/// `/usr/share/pimax/linux` (unless `PIMAX_LINUX_DIR` says otherwise).
pub fn doctor_opts(p: &Paths, env: &dyn Fn(&str) -> Option<String>) -> pimax_doctor::Opts {
    let mut o = pimax_doctor::Opts::from_env(env);
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
    /// Short key (`desk`, `slam`, `pimax`).
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
    /// Source/build trees of Monado, WayVR, Basalt, xrizer (`~/src`; `src_dir` in `~/.config/pimax/paths.toml` or
    /// `PIMAX_SRC_DIR`).
    pub src: PathBuf,
    /// The repository (`~/pimax-re`, or `PIMAX_REPO`).
    pub repo: PathBuf,
    /// `<repo>/rust`.
    pub rust: PathBuf,
    /// `<repo>/rust/target/release`.
    pub release: PathBuf,
    /// Launcher session directory (`/tmp/pimax-vr-session` or `PIMAX_VR_SESSION_DIR`).
    pub session_dir: PathBuf,
    /// `~/.config/pimax`.
    pub config: PathBuf,
    /// `~/.local/share/pimax/slam-recordings`.
    pub slam_recordings: PathBuf,
    /// Installed launcher (`~/.local/bin/pimax`).
    pub launcher_installed: PathBuf,
    /// Launcher build (`<release>/pimax`).
    pub launcher_built: PathBuf,
    /// Installed app binary (`~/.local/bin/pimax-setup`).
    pub app_installed: PathBuf,
    /// App build (`<release>/pimax-setup`).
    pub app_built: PathBuf,
    /// Desktop entry (`~/.local/share/applications/pimax.desktop`).
    pub desktop_entry: PathBuf,
    /// Icon (`~/.local/share/icons/hicolor/scalable/apps/pimax.svg`).
    pub icon: PathBuf,
    /// Monado builds, the main desk build first.
    pub builds: Vec<BuildDir>,
    /// WayVR binary (`~/src/target-desk/release/wayvr`).
    pub wayvr_bin: PathBuf,
    /// WayVR source tree (`~/src/wayvr`).
    pub wayvr_src: PathBuf,
    /// Basalt (`~/.local/lib/libbasalt.so`).
    pub basalt_lib: PathBuf,
    /// Parakeet model directory.
    pub voice_model: PathBuf,
    /// Silero VAD model.
    pub vad_model: PathBuf,
    /// sherpa-onnx C API library (default of pimax-voice).
    pub sherpa_lib: PathBuf,
    /// The headset lock of the device tools.
    pub headset_lock: PathBuf,
    /// `~/.config/openvr/openvrpaths.vrpath`.
    pub openvr_paths: PathBuf,
    /// `~/.config/pimax/ipd.env` (LESSONS row 18).
    pub ipd_env: PathBuf,
    /// `~/.config/pimax/eye-calibration.toml` (LESSONS row 18).
    pub eye_calibration: PathBuf,
    /// App state (`~/.config/pimax/gui-state.toml`: wizard done, last toggles).
    pub gui_state: PathBuf,
    /// `$XDG_RUNTIME_DIR` (or `/run/user/<uid>`).
    pub runtime_dir: PathBuf,
    /// The app's own state: logs, crash reports, problem reports, job outputs (`$XDG_STATE_HOME/pimax`, else
    /// `~/.local/state/pimax`).
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
                home.join("pimax-re")
            }
        });
        let session_dir = get("PIMAX_VR_SESSION_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(pimax_tools::vr::config::SESSION_DIR));
        let runtime_dir = get("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/run/user/1000"));
        // a package install (/usr/bin/pimax-setup with /usr/share/pimax): docs and scripts from there, the tools
        // in /usr/bin; the Monado and WayVR builds stay in the user's ~/src
        let exe = std::env::current_exe().ok();
        let system = get("PIMAX_REPO").is_none() && exe.as_deref().is_some_and(is_system_install);
        let repo = if system { PathBuf::from(SYSTEM_DATA) } else { repo };
        let mut p = Paths::new(&home, &repo, &session_dir, &runtime_dir);
        // the source trees: paths.toml / PIMAX_SRC_DIR (pimax_config::paths), default ~/src
        let src = get("PIMAX_SRC_DIR").map(PathBuf::from).unwrap_or_else(|| pimax_config::paths::src_dir(&home));
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
        let rust = repo.join("rust");
        let release = rust.join("target/release");
        let src = home.join("src");
        Paths {
            src: src.clone(),
            home: home.to_path_buf(),
            repo: repo.to_path_buf(),
            rust: rust.clone(),
            release: release.clone(),
            session_dir: session_dir.to_path_buf(),
            config: home.join(".config/pimax"),
            slam_recordings: home.join(".local/share/pimax/slam-recordings"),
            launcher_installed: home.join(".local/bin/pimax"),
            launcher_built: release.join("pimax"),
            app_installed: home.join(".local/bin/pimax-setup"),
            app_built: release.join("pimax-setup"),
            desktop_entry: home.join(".local/share/applications/pimax.desktop"),
            icon: home.join(".local/share/icons/hicolor/scalable/apps/pimax.svg"),
            builds: vec![
                BuildDir {
                    key: "desk",
                    label: "Monado desk (eye + hands + 6DoF, main build)",
                    dir: src.join("monado-eye/build"),
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
            wayvr_bin: src.join("target-desk/release/wayvr"),
            wayvr_src: src.join("wayvr"),
            basalt_lib: home.join(".local/lib/libbasalt.so"),
            voice_model: home.join("models/parakeet-tdt-0.6b-v3-int8"),
            vad_model: home.join("models/vad/silero_vad.onnx"),
            sherpa_lib: src.join("venv-voice/lib/python3.14/site-packages/sherpa_onnx/lib/libsherpa-onnx-c-api.so"),
            headset_lock: PathBuf::from("/tmp/pimax-headset.lock"),
            openvr_paths: home.join(".config/openvr/openvrpaths.vrpath"),
            ipd_env: home.join(".config/pimax/ipd.env"),
            eye_calibration: home.join(".config/pimax/eye-calibration.toml"),
            gui_state: home.join(".config/pimax/gui-state.toml"),
            runtime_dir: runtime_dir.to_path_buf(),
            state_dir: home.join(".local/state/pimax"),
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
        self.launcher_installed = bin.join("pimax");
        self.launcher_built = bin.join("pimax");
        self.app_installed = bin.join("pimax-setup");
        self.app_built = bin.join("pimax-setup");
        self.desktop_entry = PathBuf::from("/usr/share/applications/pimax.desktop");
        self.icon = PathBuf::from("/usr/share/icons/hicolor/scalable/apps/pimax.svg");
    }

    /// The main desk build (LESSONS row 16: `~/src/monado-eye/build`, branch `desk`).
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
        self.runtime_dir.join("pimax-cam-broker.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_inventory() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "PIMAX_REPO" => Some("/home/u/pimax-re".to_string()),
            _ => None,
        };
        let p = Paths::from_env(&env);
        assert_eq!(p.session_dir, PathBuf::from("/tmp/pimax-vr-session"));
        assert_eq!(p.desk_build().dir, PathBuf::from("/home/u/src/monado-eye/build"));
        assert_eq!(
            p.desk_build().service(),
            PathBuf::from("/home/u/src/monado-eye/build/src/xrt/targets/service/monado-service")
        );
        assert_eq!(p.builds.len(), 3);
        assert_eq!(p.bin("pimax-eye"), PathBuf::from("/home/u/pimax-re/rust/target/release/pimax-eye"));
        assert_eq!(p.script("run_slam_view.sh"), PathBuf::from("/home/u/pimax-re/analysis/tools/run_slam_view.sh"));
        assert_eq!(p.launcher(&|_| false), PathBuf::from("/home/u/pimax-re/rust/target/release/pimax"));
        assert_eq!(p.launcher(&|_| true), PathBuf::from("/home/u/.local/bin/pimax"));
        assert_eq!(p.broker_socket(), PathBuf::from("/run/user/1000/pimax-cam-broker.sock"));
        let env2 = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "PIMAX_VR_SESSION_DIR" => Some("/s".to_string()),
            "XDG_RUNTIME_DIR" => Some("/run/user/7".to_string()),
            _ => None,
        };
        let p = Paths::from_env(&env2);
        assert_eq!(p.session_dir, PathBuf::from("/s"));
        assert_eq!(p.runtime_dir, PathBuf::from("/run/user/7"));
        assert_eq!(p.state_dir, PathBuf::from("/home/u/.local/state/pimax"));
        let env3 = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "PIMAX_MONADO_BUILD" => Some("/opt/monado/build".to_string()),
            "XDG_STATE_HOME" => Some("/st".to_string()),
            _ => None,
        };
        let p = Paths::from_env(&env3);
        assert_eq!(p.builds.len(), 4, "the custom build is inspected too");
        assert_eq!(p.build("custom").unwrap().dir, PathBuf::from("/opt/monado/build"));
        assert_eq!(p.state_dir, PathBuf::from("/st/pimax"));
        let mut p = Paths::new(Path::new("/home/u"), Path::new(SYSTEM_DATA), Path::new("/s"), Path::new("/r"));
        p.as_system_install();
        assert_eq!(p.launcher(&|_| true), PathBuf::from("/usr/bin/pimax"));
        assert_eq!(p.bin("pimax-eye"), PathBuf::from("/usr/bin/pimax-eye"));
        assert_eq!(p.doc("LESSONS.md"), PathBuf::from("/usr/share/pimax/docs/LESSONS.md"));
        assert_eq!(p.desk_build().dir, PathBuf::from("/home/u/src/monado-eye/build"), "builds stay in ~/src");
        assert!(!is_system_install(Path::new("/home/u/.local/bin/pimax-setup")));
    }
}
