//! System checks and the Fix catalogue.
//!
//! A [`Check`] is pure: it reads the machine through `omarchyvr_doctor::Host` (real or fake) and returns findings. It
//! never changes anything. A finding may point to a [`Fix`] **by id** from the fixed [`CATALOGUE`]; the fix's
//! commands are templates whose `{arg}` slots are filled only with values that pass [`safe_arg`]. That catalogue is
//! also the only thing the Autofix LLM may propose (see [`crate::autofix`]), so no free-form shell ever reaches the
//! user's terminal from a model. Root fixes are shown for the user to run; this crate runs nothing.

use crate::module::HeadsetModule;
use omarchyvr_doctor::{Host, Opts};

/// Which part of the system a check covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Device,
    Drivers,
    Gpu,
    Cpu,
    Audio,
    Usb,
    Runtime,
}

impl Area {
    pub fn id(self) -> &'static str {
        match self {
            Area::Device => "device",
            Area::Drivers => "drivers",
            Area::Gpu => "gpu",
            Area::Cpu => "cpu",
            Area::Audio => "audio",
            Area::Usb => "usb",
            Area::Runtime => "runtime",
        }
    }
}

/// Severity, same order as `omarchyvr_doctor::Level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Ok,
    Warn,
    Fail,
}

impl Severity {
    pub fn id(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Ok => "ok",
            Severity::Warn => "warn",
            Severity::Fail => "fail",
        }
    }
}

impl From<omarchyvr_doctor::Level> for Severity {
    fn from(l: omarchyvr_doctor::Level) -> Self {
        match l {
            omarchyvr_doctor::Level::Info => Severity::Info,
            omarchyvr_doctor::Level::Ok => Severity::Ok,
            omarchyvr_doctor::Level::Warn => Severity::Warn,
            omarchyvr_doctor::Level::Fail => Severity::Fail,
        }
    }
}

/// A described, reversible action. Commands are templates; `{name}` is replaced by a checked argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fix {
    pub id: &'static str,
    pub why: &'static str,
    pub root: bool,
    pub reboot: bool,
    pub commands: &'static [&'static str],
    /// Empty only for actions that change no state (then `reversible` is still true) or cannot be undone.
    pub undo: &'static [&'static str],
    pub reversible: bool,
    /// Argument names the templates use.
    pub args: &'static [&'static str],
}

/// The only fixes that exist. Adding one is a reviewed code change (see AGENTS.md).
pub const CATALOGUE: &[Fix] = &[
    Fix {
        id: "cpu.setcap_monado",
        why: "Grant cap_sys_nice to this exact monado-service binary (realtime compositor thread; lost on every relink).",
        root: true,
        reboot: false,
        commands: &["sudo setcap cap_sys_nice+ep {path}"],
        undo: &["sudo setcap -r {path}"],
        reversible: true,
        args: &["path"],
    },
    Fix {
        id: "cpu.governor_performance",
        why: "Use the performance CPU governor while in VR (fewer frame-time spikes).",
        root: true,
        reboot: false,
        commands: &["sudo cpupower frequency-set -g performance"],
        undo: &["sudo cpupower frequency-set -g {previous}"],
        reversible: true,
        args: &["previous"],
    },
    Fix {
        id: "gpu.nvidia_patches_reapply",
        why: "Re-apply the three NVIDIA open-module patches and rebuild with DKMS (needed after every driver update).",
        root: true,
        reboot: true,
        commands: &["sudo {repo}/linux/nvidia-patch-reapply.sh --rebuild"],
        undo: &["sudo pacman -S nvidia-open-dkms"],
        reversible: true,
        args: &["repo"],
    },
    Fix {
        id: "udev.install_rules",
        why: "Let your user open the headset's HID, camera and eye-tracker nodes (uaccess).",
        root: true,
        reboot: false,
        commands: &[
            "sudo install -Dm644 {repo}/linux/70-pimax.rules /etc/udev/rules.d/70-pimax.rules",
            "sudo udevadm control --reload",
            "sudo udevadm trigger",
        ],
        undo: &["sudo rm /etc/udev/rules.d/70-pimax.rules", "sudo udevadm control --reload"],
        reversible: true,
        args: &["repo"],
    },
    Fix {
        id: "user.keepalive_enable",
        why: "Start the user keepalive service so the headset stays attached.",
        root: false,
        reboot: false,
        commands: &["systemctl --user enable --now omarchyvr-keepalived"],
        undo: &["systemctl --user disable --now omarchyvr-keepalived"],
        reversible: true,
        args: &[],
    },
    Fix {
        id: "runtime.stop_stale_monado",
        why: "A monado-service from an earlier run is still alive; stop it cleanly (SIGINT, 30 s grace, never SIGKILL).",
        root: false,
        reboot: false,
        commands: &["omarchyvr vr stop --grace 30"],
        undo: &[],
        reversible: true,
        args: &[],
    },
    Fix {
        id: "runtime.steam_vr_games",
        why: "Let every Steam game reach Monado without per-game launch options (PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 for Steam itself). Quit and start Steam once afterwards (or log out and in).",
        root: false,
        reboot: false,
        commands: &["omarchyvr vr steam-setup on"],
        undo: &["omarchyvr vr steam-setup off"],
        reversible: true,
        args: &[],
    },
    Fix {
        id: "system.reboot",
        why: "The GPU logged an Xid or display-engine timeout this boot; reboot before the next headset test.",
        root: false,
        reboot: true,
        commands: &["systemctl reboot"],
        undo: &[],
        reversible: true,
        args: &[],
    },
];

pub fn fix(id: &str) -> Option<&'static Fix> {
    CATALOGUE.iter().find(|f| f.id == id)
}

/// Arguments may only be plain paths, words or versions: no spaces, quotes, `$`, `;`, `|`, backticks, `..`.
pub fn safe_arg(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 256
        && !v.contains("..")
        && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | ':' | '~'))
}

/// A finding's link to a catalogue fix, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixRef {
    pub id: &'static str,
    pub args: Vec<(String, String)>,
}

/// A fix ready to show: rendered commands and undo lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub fix: &'static Fix,
    pub commands: Vec<String>,
    pub undo: Vec<String>,
}

impl FixRef {
    pub fn new(id: &'static str, args: &[(&str, &str)]) -> FixRef {
        FixRef { id, args: args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    /// Render the templates. `Err` for an unknown id, a missing or extra argument, or an unsafe value.
    pub fn plan(&self) -> Result<Planned, String> {
        let f = fix(self.id).ok_or_else(|| format!("unknown fix {}", self.id))?;
        for (k, v) in &self.args {
            if !f.args.contains(&k.as_str()) {
                return Err(format!("{}: unexpected argument {k}", f.id));
            }
            if !safe_arg(v) {
                return Err(format!("{}: unsafe value for {k}", f.id));
            }
        }
        let fill = |t: &str| -> Result<String, String> {
            let mut s = t.to_string();
            for a in f.args {
                let pat = format!("{{{a}}}");
                if s.contains(&pat) {
                    let v = self.args.iter().find(|(k, _)| k == a).ok_or_else(|| format!("{}: missing {a}", f.id))?;
                    s = s.replace(&pat, &v.1);
                }
            }
            Ok(s)
        };
        Ok(Planned {
            fix: f,
            commands: f.commands.iter().map(|c| fill(c)).collect::<Result<_, _>>()?,
            undo: f.undo.iter().map(|c| fill(c)).collect::<Result<_, _>>()?,
        })
    }
}

/// One result line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: String,
    pub area: Area,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub fix: Option<FixRef>,
}

/// What a check may look at.
pub struct Ctx<'a> {
    pub host: &'a dyn Host,
    pub opts: &'a Opts,
    /// The matched headset module, if any (checks may depend on its needs, e.g. NVIDIA merge mode).
    pub module: Option<&'a dyn HeadsetModule>,
}

/// A pure system check.
pub trait Check: Send + Sync {
    fn id(&self) -> &'static str;
    fn area(&self) -> Area;
    fn run(&self, cx: &Ctx) -> Vec<Finding>;

    /// Helper for implementors.
    fn finding(&self, severity: Severity, title: &str, detail: impl Into<String>, fix: Option<FixRef>) -> Finding
    where
        Self: Sized,
    {
        Finding { check: self.id().into(), area: self.area(), severity, title: title.into(), detail: detail.into(), fix }
    }
}

/// Run checks in order; one check's panic is not caught here (checks must not panic; tests cover them).
pub fn run_all(checks: &[&dyn Check], cx: &Ctx) -> Vec<Finding> {
    checks.iter().flat_map(|c| c.run(cx)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_templates_only_take_safe_args() {
        let ok = FixRef::new("cpu.setcap_monado", &[("path", "/home/u/src/monado/build/src/xrt/targets/service/monado-service")]);
        let p = ok.plan().unwrap();
        assert_eq!(p.commands, vec!["sudo setcap cap_sys_nice+ep /home/u/src/monado/build/src/xrt/targets/service/monado-service"]);
        assert!(p.undo[0].starts_with("sudo setcap -r /home/u/"));
        for bad in ["/x; rm -rf ~", "$(id)", "a b", "`x`", "../../etc", "a|b", ""] {
            assert!(FixRef::new("cpu.setcap_monado", &[("path", bad)]).plan().is_err(), "{bad:?} accepted");
        }
        assert!(FixRef::new("cpu.setcap_monado", &[]).plan().is_err(), "missing arg accepted");
        assert!(FixRef::new("cpu.setcap_monado", &[("path", "/x"), ("evil", "y")]).plan().is_err());
        assert!(FixRef::new("shell.anything", &[]).plan().is_err());
    }

    #[test]
    fn catalogue_is_consistent() {
        let mut ids: Vec<_> = CATALOGUE.iter().map(|f| f.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), CATALOGUE.len(), "duplicate fix id");
        for f in CATALOGUE {
            for line in f.commands.iter().chain(f.undo) {
                for a in line.split('{').skip(1) {
                    let name = a.split('}').next().unwrap();
                    assert!(f.args.contains(&name), "{}: {{{name}}} not declared", f.id);
                }
            }
            if f.commands.iter().any(|c| c.starts_with("sudo ")) {
                assert!(f.root, "{} uses sudo but is not marked root", f.id);
            }
            assert!(!f.commands.iter().any(|c| c.contains("/sys/class/drm") || c.contains("pkill") || c.contains("kill -9")), "{}: forbidden action", f.id);
        }
    }
}
