//! The checks. Each one appends to a [`Report`]; none writes anything.
use crate::host::Host;
use crate::report::{Fix, Level, Report};
use crate::{DREAM_AIR_PID, PIMAX_VID, TOBII_PID, TOBII_VID};
use omarchyvr_config::{ProfileId, Store};
use std::path::{Path, PathBuf};

/// One of our NVIDIA source patches.
pub struct PatchInfo {
    /// Short id.
    pub id: &'static str,
    /// File in the `linux/` directory.
    pub file: &'static str,
    /// What it does.
    pub what: &'static str,
}

/// The three patches the Dream Air needs on NVIDIA (`linux/DISPLAY.md`, report 1.3).
pub const PATCHES: [PatchInfo; 3] = [
    PatchInfo {
        id: "viewport",
        file: "nvidia-2h1or-viewport.patch",
        what: "viewport x offset of merged heads (else the left half shows in both eyes)",
    },
    PatchInfo {
        id: "flip-fix",
        file: "nvidia-2h1or-flip-fix.patch",
        what: "merged-head flip sync (module parameter merged_head_sync)",
    },
    PatchInfo {
        id: "bounded-wait",
        file: "nvidia-evo-makeroom-bounded-wait.patch",
        what: "bounded wait in the EVO make-room path (no display-engine hang)",
    },
];

/// Inputs of the checks besides the machine.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Monado build directory.
    pub monado_build: PathBuf,
    /// Directory with the NVIDIA patches and the udev rules (`linux/` of the repository).
    pub linux_dir: PathBuf,
    /// `~/.config/omarchyvr`.
    pub config_root: PathBuf,
    /// `~/.cache/omarchyvr`.
    pub cache_root: PathBuf,
    /// Directory with the `nvidia-<version>` source trees.
    pub nvidia_src_root: PathBuf,
    /// Connector the user configured (checked against the one found by EDID).
    pub connector: Option<String>,
}

impl Opts {
    /// Defaults from the environment: `PIMAX_MONADO_BUILD`, `PIMAX_LINUX_DIR`, `XDG_*`, `NVIDIA_SRC_ROOT`.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Opts {
        let home = PathBuf::from(env("HOME").unwrap_or_else(|| ".".into()));
        let store = Store::from_env(env);
        let linux_dir = env("PIMAX_LINUX_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../linux");
                if built.is_dir() {
                    built.canonicalize().unwrap_or(built)
                } else {
                    home.join("pimax-re/linux")
                }
            });
        Opts {
            monado_build: env("PIMAX_MONADO_BUILD")
                .filter(|v| !v.is_empty())
                .map_or_else(|| omarchyvr_config::paths::src_dir(&home).join("monado/build"), PathBuf::from),
            linux_dir,
            config_root: store.root().to_path_buf(),
            cache_root: store.cache().to_path_buf(),
            nvidia_src_root: env("NVIDIA_SRC_ROOT")
                .filter(|v| !v.is_empty())
                .map_or_else(|| "/usr/src".into(), PathBuf::from),
            connector: env("PIMAX_CONNECTOR").filter(|v| !v.is_empty()),
        }
    }
}

/// A USB device from sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDev {
    /// sysfs name (`1-5.2.4.1`).
    pub name: String,
    /// Vendor id.
    pub vid: u16,
    /// Product id.
    pub pid: u16,
    /// Serial string.
    pub serial: Option<String>,
    /// bcdDevice (hardware revision string).
    pub bcd: Option<String>,
    /// `/dev/bus/usb/BBB/DDD` node.
    pub node: Option<PathBuf>,
}

/// All USB devices with a vendor id.
pub fn usb_devices(h: &dyn Host) -> Vec<UsbDev> {
    let base = Path::new("/sys/bus/usb/devices");
    let hex = |s: Option<String>| s.and_then(|s| u16::from_str_radix(&s, 16).ok());
    h.list(base)
        .into_iter()
        .filter_map(|name| {
            let d = base.join(&name);
            let vid = hex(h.read_str(&d.join("idVendor")))?;
            let pid = hex(h.read_str(&d.join("idProduct")))?;
            let num = |f: &str| h.read_str(&d.join(f)).and_then(|s| s.parse::<u32>().ok());
            let node = match (num("busnum"), num("devnum")) {
                (Some(b), Some(n)) => Some(PathBuf::from(format!("/dev/bus/usb/{b:03}/{n:03}"))),
                _ => None,
            };
            Some(UsbDev {
                name,
                vid,
                pid,
                serial: h.read_str(&d.join("serial")),
                bcd: h.read_str(&d.join("bcdDevice")),
                node,
            })
        })
        .collect()
}

/// The Dream Air interface device (34a4:0044), if present.
pub fn find_headset(devs: &[UsbDev]) -> Option<&UsbDev> {
    devs.iter()
        .find(|d| d.vid == PIMAX_VID && d.pid == DREAM_AIR_PID)
}

/// A DRM connector with its EDID identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    /// `DP-1`.
    pub name: String,
    /// `card1`.
    pub card: String,
    /// `connected` / `disconnected`.
    pub status: String,
    /// EDID manufacturer id (`PVR`).
    pub mfr: Option<String>,
    /// EDID product code.
    pub product: Option<u16>,
    /// Mode lines from sysfs (duplicates kept).
    pub modes: Vec<String>,
}

impl Connector {
    /// Is this the Pimax panel (EDID manufacturer PVR)?
    pub fn is_pimax(&self) -> bool {
        self.mfr.as_deref() == Some("PVR")
    }
}

/// All connectors under `/sys/class/drm`.
pub fn connectors(h: &dyn Host) -> Vec<Connector> {
    let base = Path::new("/sys/class/drm");
    h.list(base)
        .into_iter()
        .filter_map(|e| {
            let (card, name) = e.split_once('-')?;
            if !card.starts_with("card") {
                return None;
            }
            let d = base.join(&e);
            let status = h.read_str(&d.join("status"))?;
            let (mfr, product) = h
                .read(&d.join("edid"))
                .map_or((None, None), |b| parse_edid_id(&b));
            let modes = h
                .read_str(&d.join("modes"))
                .map(|m| m.lines().map(str::to_string).collect())
                .unwrap_or_default();
            Some(Connector {
                name: name.to_string(),
                card: card.to_string(),
                status,
                mfr,
                product,
                modes,
            })
        })
        .collect()
}

/// Manufacturer letters and product code from an EDID base block.
pub fn parse_edid_id(b: &[u8]) -> (Option<String>, Option<u16>) {
    if b.len() < 12 || b[..8] != [0, 255, 255, 255, 255, 255, 255, 0] {
        return (None, None);
    }
    let id = u16::from_be_bytes([b[8], b[9]]);
    let l = |s: u16| char::from(b'A' - 1 + ((id >> s) & 31) as u8);
    (
        Some([l(10), l(5), l(0)].iter().collect()),
        Some(u16::from_le_bytes([b[10], b[11]])),
    )
}

/// Run every check.
pub fn run_checks(h: &dyn Host, o: &Opts) -> Report {
    let mut r = Report::default();
    let devs = usb_devices(h);
    usb(&devs, &mut r);
    udev(h, o, &devs, &mut r);
    keepalive(h, &mut r);
    nvidia(h, o, &mut r);
    connector(h, o, &mut r);
    lease(h, &mut r);
    setcap(h, o, &mut r);
    kernel_log(h, &mut r);
    stale_service(h, &mut r);
    lens_conf(
        o,
        find_headset(&devs).and_then(|d| d.serial.as_deref()),
        &mut r,
    );
    r
}

fn usb(devs: &[UsbDev], r: &mut Report) {
    match find_headset(devs) {
        Some(d) => {
            let other: Vec<String> = devs
                .iter()
                .filter(|x| x.vid == PIMAX_VID && x.pid != DREAM_AIR_PID)
                .map(|x| format!("{:04x}:{:04x}", x.vid, x.pid))
                .collect();
            r.add(
                "headset",
                "Headset on USB",
                Level::Ok,
                format!(
                    "34a4:0044 serial {} (bcd {}){}",
                    d.serial.as_deref().unwrap_or("?"),
                    d.bcd.as_deref().unwrap_or("?"),
                    if other.is_empty() {
                        String::new()
                    } else {
                        format!("; other 34a4 devices: {}", other.join(" "))
                    }
                ),
                None,
            );
        }
        None => {
            let other: Vec<String> = devs
                .iter()
                .filter(|x| x.vid == PIMAX_VID)
                .map(|x| format!("{:04x}:{:04x}", x.vid, x.pid))
                .collect();
            let detail = if other.is_empty() {
                "no 34a4:0044 on the USB bus".to_string()
            } else {
                format!(
                    "no 34a4:0044; found {} (a different model/mode is not supported yet)",
                    other.join(" ")
                )
            };
            r.add(
                "headset",
                "Headset on USB",
                Level::Fail,
                detail,
                Some(Fix::new(
                    "The headset is not enumerated: check power, the USB cable/hub and that the link box is on.",
                    false,
                    &["lsusb | grep -i 34a4", "journalctl -k -b --no-pager | grep -i usb | tail -20"],
                    &[],
                )),
            );
        }
    }
    match devs
        .iter()
        .find(|d| d.vid == TOBII_VID && d.pid == TOBII_PID)
    {
        Some(d) => r.add(
            "tobii",
            "Tobii eye chip",
            Level::Ok,
            format!("2104:0561 serial {}", d.serial.as_deref().unwrap_or("?")),
            None,
        ),
        None => r.add(
            "tobii",
            "Tobii eye chip",
            Level::Warn,
            "2104:0561 not found: eye tracking unavailable (VR itself works)",
            None,
        ),
    }
}

fn udev(h: &dyn Host, o: &Opts, devs: &[UsbDev], r: &mut Report) {
    let mut bad = Vec::new();
    let mut seen = 0;
    for n in h.list(Path::new("/sys/class/hidraw")) {
        let u = h
            .read_str(
                &Path::new("/sys/class/hidraw")
                    .join(&n)
                    .join("device/uevent"),
            )
            .unwrap_or_default();
        if u.to_ascii_uppercase().contains("HID_ID=0003:000034A4:") {
            seen += 1;
            let node = Path::new("/dev").join(&n);
            if !h.can_rw(&node) {
                bad.push(node.display().to_string());
            }
        }
    }
    for d in devs
        .iter()
        .filter(|d| (d.vid == PIMAX_VID) || (d.vid == TOBII_VID && d.pid == TOBII_PID))
    {
        if let Some(n) = &d.node {
            seen += 1;
            if !h.can_rw(n) {
                bad.push(n.display().to_string());
            }
        }
    }
    let rules = o.linux_dir.join("70-pimax.rules");
    let rules_s = rules.display().to_string();
    if seen == 0 {
        r.add(
            "udev",
            "udev / uaccess",
            Level::Info,
            "no Pimax device nodes to test",
            None,
        );
    } else if bad.is_empty() {
        r.add(
            "udev",
            "udev / uaccess",
            Level::Ok,
            format!("{seen} device nodes are read/write for this user"),
            None,
        );
    } else {
        let install = format!("sudo install -m644 {rules_s} /etc/udev/rules.d/70-pimax.rules");
        r.add(
            "udev",
            "udev / uaccess",
            Level::Fail,
            format!("no access to: {}", bad.join(", ")),
            Some(Fix::new(
                "The udev rule gives the logged-in user access (TAG+=uaccess) to 34a4:* and 2104:0561 without root.",
                true,
                &[
                    &install,
                    "sudo udevadm control --reload",
                    "sudo udevadm trigger --subsystem-match=hidraw --subsystem-match=usb --action=change",
                ],
                &["sudo rm /etc/udev/rules.d/70-pimax.rules", "sudo udevadm control --reload"],
            )),
        );
    }
}

fn keepalive(h: &dyn Host, r: &mut Report) {
    let s = h
        .run("systemctl", &["--user", "is-active", "omarchyvr-keepalived"])
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_else(|| "unknown (systemctl missing)".into());
    if s == "active" {
        r.add("keepalive", "omarchyvr-keepalived", Level::Ok, "active", None);
    } else {
        r.add(
            "keepalive",
            "omarchyvr-keepalived",
            Level::Fail,
            format!("service is {s}; the display shuts off without it"),
            Some(Fix::new(
                "The keepalive service keeps the headset display powered over USB.",
                false,
                &["systemctl --user enable --now omarchyvr-keepalived"],
                &["systemctl --user disable --now omarchyvr-keepalived"],
            )),
        );
    }
}

fn nvidia_present(h: &dyn Host) -> bool {
    h.list(Path::new("/sys/class/drm")).iter().any(|e| {
        !e.contains('-')
            && h.read_str(&Path::new("/sys/class/drm").join(e).join("device/vendor"))
                .as_deref()
                == Some("0x10de")
    })
}

/// Driver version from `/proc/driver/nvidia/version` and whether it is the open module.
pub fn nvidia_version(h: &dyn Host) -> Option<(String, bool)> {
    let t = h.read_str(Path::new("/proc/driver/nvidia/version"))?;
    let first = t.lines().next()?;
    let ver = first
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))?;
    Some((ver.to_string(), first.contains("Open Kernel Module")))
}

fn nvidia(h: &dyn Host, o: &Opts, r: &mut Report) {
    if !nvidia_present(h) {
        r.add(
            "gpu",
            "GPU / driver",
            Level::Info,
            "no NVIDIA GPU found: AMD/Intel are not supported yet (report section 2)",
            None,
        );
        return;
    }
    let (ver, open) = match nvidia_version(h) {
        Some(v) => v,
        None => {
            r.add(
                "gpu",
                "GPU / driver",
                Level::Fail,
                "NVIDIA GPU present but /proc/driver/nvidia/version is missing: kernel module not loaded",
                Some(Fix::new("Load the module (or reboot after installing nvidia-open-dkms).", true, &["sudo modprobe nvidia_drm modeset=1"], &[])),
            );
            return;
        }
    };
    if open {
        r.add(
            "gpu",
            "GPU / driver",
            Level::Ok,
            format!("NVIDIA open kernel module {ver}"),
            None,
        );
    } else {
        r.add(
            "gpu",
            "GPU / driver",
            Level::Fail,
            format!("NVIDIA proprietary module {ver}: the patches only exist for the open module"),
            Some(
                Fix::new(
                    "Install nvidia-open-dkms.",
                    true,
                    &["sudo pacman -S nvidia-open-dkms"],
                    &[],
                )
                .reboot(),
            ),
        );
    }

    // source-tree patches (what the next dkms build will contain)
    let mut trees: Vec<String> = h
        .list(&o.nvidia_src_root)
        .into_iter()
        .filter(|n| {
            n.strip_prefix("nvidia-")
                .is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        })
        .collect();
    if trees.iter().any(|t| *t == format!("nvidia-{ver}")) {
        trees.retain(|t| *t == format!("nvidia-{ver}"));
    }
    if trees.is_empty() {
        r.add(
            "patches",
            "NVIDIA patches",
            Level::Warn,
            format!(
                "no nvidia-<version> source tree in {}: cannot verify the 3 patches",
                o.nvidia_src_root.display()
            ),
            None,
        );
    } else {
        let mut missing: Vec<(String, &PatchInfo)> = Vec::new();
        let mut unknown = Vec::new();
        for t in &trees {
            let tree = o.nvidia_src_root.join(t);
            for p in &PATCHES {
                let pf = o.linux_dir.join(p.file);
                if !h.exists(&pf) {
                    unknown.push(p.id);
                    continue;
                }
                let out = h.run(
                    "patch",
                    &[
                        "-R",
                        "-p1",
                        "--dry-run",
                        "--fuzz=0",
                        "-s",
                        "-d",
                        &tree.display().to_string(),
                        "-i",
                        &pf.display().to_string(),
                    ],
                );
                match out {
                    Some(c) if c.code == Some(0) => {}
                    Some(_) => missing.push((t.clone(), p)),
                    None => unknown.push(p.id),
                }
            }
        }
        if !missing.is_empty() {
            let mut cmds: Vec<String> = Vec::new();
            let reapply = o
                .linux_dir
                .join("nvidia-patch-reapply.sh")
                .display()
                .to_string();
            if missing.iter().any(|(_, p)| p.id == "viewport") {
                cmds.push(format!("sudo {reapply} --rebuild"));
            }
            for (t, p) in missing.iter().filter(|(_, p)| p.id != "viewport") {
                cmds.push(format!(
                    "sudo patch -p1 -d {} -i {}",
                    o.nvidia_src_root.join(t).display(),
                    o.linux_dir.join(p.file).display()
                ));
            }
            if missing.iter().any(|(_, p)| p.id != "viewport") {
                cmds.push(format!("sudo dkms build nvidia/{ver} --force && sudo dkms install nvidia/{ver} --force"));
                cmds.push("sudo mkinitcpio -P   # or limine-mkinitcpio on Omarchy".into());
            }
            r.add(
                "patches",
                "NVIDIA patches",
                Level::Fail,
                format!(
                    "not applied to the source tree: {}",
                    missing.iter().map(|(t, p)| format!("{} ({t})", p.id)).collect::<Vec<_>>().join(", ")
                ),
                Some(
                    Fix::new(
                        "Without all three patches 7104x3840 shows one half in both eyes or hangs the display engine. Every driver update replaces the source tree, so they must be re-applied.",
                        true,
                        &cmds.iter().map(String::as_str).collect::<Vec<_>>(),
                        &["# patches are reversible: patch -R -p1 -d /usr/src/nvidia-<ver> -i <patch>"],
                    )
                    .reboot(),
                ),
            );
        } else if !unknown.is_empty() {
            r.add(
                "patches",
                "NVIDIA patches",
                Level::Warn,
                format!(
                    "could not verify: {} (patch file or `patch` tool missing in {})",
                    unknown.join(", "),
                    o.linux_dir.display()
                ),
                None,
            );
        } else {
            r.add(
                "patches",
                "NVIDIA patches",
                Level::Ok,
                format!("all 3 patches present in {} (source tree; a rebuilt module is needed for them to be live)", trees.join(", ")),
                None,
            );
        }
    }

    let p = Path::new("/sys/module/nvidia_modeset/parameters/merged_head_sync");
    match h.read_str(p).as_deref() {
        Some("1") | Some("Y") => r.add("merged-head-sync", "merged_head_sync", Level::Ok, "loaded module has the flip fix, value 1", None),
        Some(v) => r.add(
            "merged-head-sync",
            "merged_head_sync",
            Level::Warn,
            format!("parameter is {v}, expected 1"),
            Some(Fix::new(
                "Enable the merged-head flip sync.",
                true,
                &["echo 'options nvidia-modeset merged_head_sync=1' | sudo tee -a /etc/modprobe.d/nvidia-pimax.conf"],
                &["sudo sed -i '/merged_head_sync/d' /etc/modprobe.d/nvidia-pimax.conf"],
            )
            .reboot()),
        ),
        None => r.add(
            "merged-head-sync",
            "merged_head_sync",
            Level::Fail,
            "the loaded nvidia_modeset has no merged_head_sync parameter: the running module is not our patched build",
            Some(Fix::new(
                "The running module predates the patches. Rebuild it with all patches and reboot.",
                true,
                &[&format!("sudo {} --rebuild", o.linux_dir.join("nvidia-patch-reapply.sh").display())],
                &[],
            )
            .reboot()),
        ),
    }
}

fn connector(h: &dyn Host, o: &Opts, r: &mut Report) {
    let all = connectors(h);
    let pim: Vec<&Connector> = all.iter().filter(|c| c.is_pimax()).collect();
    let linux = o.linux_dir.display();
    let Some(c) = pim.iter().find(|c| c.status == "connected").or(pim.first()) else {
        let connected: Vec<String> = all
            .iter()
            .filter(|c| c.status == "connected")
            .map(|c| c.name.clone())
            .collect();
        r.add(
            "connector",
            "Headset connector",
            Level::Fail,
            format!("no connector with EDID PVR; connected: {}", if connected.is_empty() { "none".into() } else { connected.join(", ") }),
            Some(Fix::new(
                "The panel is not detected by its EDID. Check the DP cable and power; with the stock EDID the native modes are missing, see linux/DISPLAY.md (EDID override, needs a reboot).",
                false,
                &[&format!("# see {linux}/DISPLAY.md, section 'Setup'")],
                &[],
            )),
        );
        return;
    };
    let has_mode = c.modes.iter().any(|m| m == "7104x3840");
    let modes: Vec<&str> = {
        let mut m: Vec<&str> = c.modes.iter().map(String::as_str).collect();
        m.dedup();
        m
    };
    let prod = c.product.map_or("?".into(), |p| format!("{p:#06x}"));
    let mut detail = format!(
        "{} ({}) {} EDID PVR/{prod}, modes: {}",
        c.name,
        c.card,
        c.status,
        modes.join(" ")
    );
    let mut level = Level::Ok;
    let mut fix = None;
    if c.status != "connected" {
        level = Level::Fail;
        detail += "; not connected";
    } else if !has_mode {
        level = Level::Fail;
        detail += "; 7104x3840 missing";
        fix = Some(
            Fix::new(
                "Install the EDID override with the native modes (needs the initramfs rebuilt), see linux/DISPLAY.md.",
                true,
                &[
                    &format!("sudo install -D -m644 {linux}/pimax-dp-edid-native.bin /etc/pimax/dp-edid.bin"),
                    "echo 'override.[0000:01:00.0].DP-0=/etc/pimax/dp-edid.bin' | sudo tee /etc/pimax/nvidia-modeset.conf   # adjust PCI address / connector",
                    "echo 'options nvidia-modeset config_file=/etc/pimax/nvidia-modeset.conf' | sudo tee /etc/modprobe.d/nvidia-pimax.conf",
                    "echo 'FILES+=(/etc/pimax/nvidia-modeset.conf /etc/pimax/dp-edid.bin)' | sudo tee /etc/mkinitcpio.conf.d/pimax.conf",
                    "sudo mkinitcpio -P",
                ],
                &["sudo rm /etc/pimax/nvidia-modeset.conf /etc/modprobe.d/nvidia-pimax.conf /etc/mkinitcpio.conf.d/pimax.conf", "sudo mkinitcpio -P"],
            )
            .reboot(),
        );
    }
    if c.product != Some(0x1019) && level == Level::Ok {
        level = Level::Warn;
        detail += "; EDID product is not the known 0x1019 (other model/unit?)";
    }
    if let Some(cfg) = &o.connector {
        if *cfg != c.name && level != Level::Fail {
            level = Level::Warn;
            detail += &format!("; configured connector is {cfg}, headset is on {}", c.name);
        }
    }
    detail += "; non-desktop flag not read (needs a DRM ioctl, deliberately not done here)";
    r.add("connector", "Headset connector", level, detail, fix);
}

fn lease(h: &dyn Host, r: &mut Report) {
    if h.env("WAYLAND_DISPLAY").filter(|v| !v.is_empty()).is_none() {
        r.add(
            "lease",
            "DRM lease protocol",
            Level::Warn,
            "WAYLAND_DISPLAY is not set: run this from inside the Wayland session",
            None,
        );
        return;
    }
    match h.run("wayland-info", &[]) {
        None => r.add(
            "lease",
            "DRM lease protocol",
            Level::Warn,
            "wayland-info not installed: cannot list compositor protocols",
            Some(Fix::new("wayland-info lists the registry (no lease is taken).", true, &["sudo pacman -S wayland-utils"], &["sudo pacman -R wayland-utils"])),
        ),
        Some(o) if o.stdout.contains("wp_drm_lease_device_v1") => {
            r.add("lease", "DRM lease protocol", Level::Ok, "compositor offers wp_drm_lease_device_v1", None)
        }
        Some(_) => r.add(
            "lease",
            "DRM lease protocol",
            Level::Fail,
            "compositor does not offer wp_drm_lease_device_v1: direct mode is impossible (Hyprland, KDE, Sway and Gamescope do)",
            None,
        ),
    }
}

fn setcap(h: &dyn Host, o: &Opts, r: &mut Report) {
    let svc = o
        .monado_build
        .join("src/xrt/targets/service/monado-service");
    let s = svc.display().to_string();
    if !h.exists(&svc) {
        r.add(
            "setcap",
            "Monado build + setcap",
            Level::Fail,
            format!("{s} not found (PIMAX_MONADO_BUILD / setup.toml monado_build)"),
            None,
        );
        return;
    }
    match h.run("getcap", &[&s]) {
        Some(c) if c.stdout.contains("cap_sys_nice") => {
            r.add("setcap", "Monado build + setcap", Level::Ok, format!("{s} has cap_sys_nice"), None)
        }
        Some(_) => r.add(
            "setcap",
            "Monado build + setcap",
            Level::Fail,
            format!("{s} lacks cap_sys_nice (needed for the realtime compositor thread; the capability is lost on every relink)"),
            Some(Fix::new(
                "Grant the capability to this exact binary.",
                true,
                &[&format!("sudo setcap cap_sys_nice+ep {s}")],
                &[&format!("sudo setcap -r {s}")],
            )),
        ),
        None => r.add("setcap", "Monado build + setcap", Level::Warn, "getcap not available (libcap), cannot check", None),
    }
}

/// Kernel log lines of this boot that mean the GPU or the display engine is in trouble.
pub fn is_gpu_error_line(l: &str) -> bool {
    l.contains("Xid")
        || l.contains("Idling display engine timed out")
        || l.contains("Idling")
        || l.contains("Stuck layer channel")
}

fn kernel_log(h: &dyn Host, r: &mut Report) {
    let out = h
        .run("journalctl", &["-k", "-b", "--no-pager", "-q"])
        .filter(|o| o.code == Some(0))
        .or_else(|| h.run("dmesg", &[]).filter(|o| o.code == Some(0)));
    let Some(out) = out else {
        r.add("kernel-log", "Kernel log (this boot)", Level::Warn, "cannot read the kernel log (journalctl -k / dmesg); add yourself to the systemd-journal group", None);
        return;
    };
    let bad: Vec<&str> = out
        .stdout
        .lines()
        .filter(|l| is_gpu_error_line(l))
        .collect();
    if bad.is_empty() {
        r.add(
            "kernel-log",
            "Kernel log (this boot)",
            Level::Ok,
            "no Xid / Idling / Stuck layer channel lines",
            None,
        );
    } else {
        let tail: Vec<&str> = bad.iter().rev().take(2).rev().copied().collect();
        r.add(
            "kernel-log",
            "Kernel log (this boot)",
            Level::Fail,
            format!("{} GPU error lines this boot, last: {}", bad.len(), tail.join(" | ")),
            Some(Fix::new(
                "The display engine or GPU is in a bad state; VR on top of it can hang the machine. Reboot before starting VR.",
                true,
                &["systemctl reboot"],
                &[],
            )),
        );
    }
}

fn stale_service(h: &dyn Host, r: &mut Report) {
    let pids: Vec<String> = h
        .list(Path::new("/proc"))
        .into_iter()
        .filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
        .filter(|n| {
            h.read_str(&Path::new("/proc").join(n).join("comm"))
                .as_deref()
                == Some("monado-service")
        })
        .collect();
    if pids.is_empty() {
        r.add(
            "stale-service",
            "monado-service",
            Level::Ok,
            "not running",
            None,
        );
    } else {
        r.add(
            "stale-service",
            "monado-service",
            Level::Warn,
            format!("running (pid {}); stop it with `omarchyvr vr stop --grace 30`, never kill -9", pids.join(", ")),
            Some(Fix::new(
                "A running service holds the display lease. Stop it politely (SIGINT with a grace period).",
                false,
                &["omarchyvr vr stop --grace 30"],
                &[],
            )),
        );
    }
}

fn lens_conf(o: &Opts, serial: Option<&str>, r: &mut Report) {
    let store = Store::new(&o.config_root, &o.cache_root);
    let path = o.config_root.join("lens.conf");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            r.add(
                "lens-conf",
                "lens.conf",
                Level::Warn,
                format!("{} missing: shipped defaults are used", path.display()),
                None,
            );
            return;
        }
    };
    let (_, w) = pimax_lens::config::parse(&text, 1.0);
    let hdr = omarchyvr_config::store::serial_in_comments(&text);
    let mut level = Level::Ok;
    let mut detail = format!("{} parses", path.display());
    if !w.is_empty() {
        level = Level::Warn;
        detail = format!(
            "{} warnings: {}",
            w.len(),
            w.iter()
                .take(2)
                .map(|w| format!("line {}: {}", w.line, w.message))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    if let (Some(h), Some(s)) = (&hdr, serial) {
        if h != s {
            level = Level::Warn;
            detail += &format!("; written for headset {h}, connected is {s}");
        }
    }
    if let Some(s) = serial {
        if let Ok(id) = ProfileId::new(s, "default") {
            if !store.profile_dir(&id).is_dir() {
                detail += "; no profile for this serial yet (import it on the Profiles page)";
            }
        }
    }
    r.add("lens-conf", "lens.conf", level, detail, None);
}
