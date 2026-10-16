//! `omarchyvr-doctor info --json`: what this machine and headset look like (sysfs only, never talks to the headset).
use crate::checks::{connectors, find_headset, nvidia_version, usb_devices, Opts};
use crate::host::Host;
use crate::json::Json;
use crate::{TOBII_PID, TOBII_VID};
use omarchyvr_config::{gyro_bias_path, legacy_gyro_bias_path, Store};
use std::path::Path;

/// Collect the machine description. Firmware versions and live IPD need the HID protocol (`omarchyvr info`); they are
/// `null` here by design so this command stays side-effect free.
pub fn gather(h: &dyn Host, o: &Opts) -> Json {
    let devs = usb_devices(h);
    let hs = find_headset(&devs);
    let serial = hs.and_then(|d| d.serial.clone());
    let tobii = devs
        .iter()
        .find(|d| d.vid == TOBII_VID && d.pid == TOBII_PID);
    let store = Store::new(&o.config_root, &o.cache_root);
    let conns = connectors(h);
    let pim = conns.iter().find(|c| c.is_pimax());
    let (gpu_ver, gpu_open) =
        nvidia_version(h).map_or((None, None), |(v, open)| (Some(v), Some(open)));
    let svc = o
        .monado_build
        .join("src/xrt/targets/service/monado-service");
    Json::Obj(vec![
        (
            "headset".into(),
            Json::Obj(vec![
                ("present".into(), Json::Bool(hs.is_some())),
                ("usb_id".into(), Json::opt(hs.map(|_| "34a4:0044"))),
                ("serial".into(), Json::opt(serial.clone())),
                ("bcd_device".into(), Json::opt(hs.and_then(|d| d.bcd.clone()))),
                ("firmware".into(), Json::Null),
                ("ipd_mm".into(), Json::Null),
                ("note".into(), Json::s("firmware and live IPD need the HID protocol: run `omarchyvr info`")),
            ]),
        ),
        (
            "eye_tracker".into(),
            Json::Obj(vec![
                ("present".into(), Json::Bool(tobii.is_some())),
                ("serial".into(), Json::opt(tobii.and_then(|d| d.serial.clone()))),
            ]),
        ),
        (
            "gpu".into(),
            Json::Obj(vec![
                ("nvidia_driver".into(), Json::opt(gpu_ver)),
                ("open_module".into(), gpu_open.map_or(Json::Null, Json::Bool)),
                (
                    "merged_head_sync".into(),
                    Json::opt(h.read_str(Path::new("/sys/module/nvidia_modeset/parameters/merged_head_sync"))),
                ),
            ]),
        ),
        (
            "connector".into(),
            pim.map_or(Json::Null, |c| {
                let mut m = c.modes.clone();
                m.dedup();
                Json::Obj(vec![
                    ("name".into(), Json::s(&c.name)),
                    ("card".into(), Json::s(&c.card)),
                    ("status".into(), Json::s(&c.status)),
                    ("edid_manufacturer".into(), Json::opt(c.mfr.clone())),
                    ("edid_product".into(), c.product.map_or(Json::Null, |p| Json::Int(p.into()))),
                    ("modes".into(), Json::Arr(m.iter().map(Json::s).collect())),
                ])
            }),
        ),
        (
            "kernel".into(),
            Json::opt(h.read_str(Path::new("/proc/sys/kernel/osrelease"))),
        ),
        (
            "monado".into(),
            Json::Obj(vec![
                ("build".into(), Json::s(o.monado_build.display().to_string())),
                ("service_present".into(), Json::Bool(h.exists(&svc))),
            ]),
        ),
        (
            "config".into(),
            Json::Obj(vec![
                ("root".into(), Json::s(store.root().display().to_string())),
                (
                    "profiles".into(),
                    Json::Arr(store.list_profiles().iter().map(|p| Json::s(p.to_string())).collect()),
                ),
                ("active".into(), Json::opt(store.active().map(|p| p.to_string()))),
            ]),
        ),
        (
            "gyro_bias".into(),
            Json::Obj(vec![
                (
                    "proposed_path".into(),
                    Json::opt(serial.as_deref().map(|s| gyro_bias_path(store.cache(), s).display().to_string())),
                ),
                (
                    "legacy_path".into(),
                    Json::s(legacy_gyro_bias_path(store.cache()).display().to_string()),
                ),
                ("legacy_exists".into(), Json::Bool(legacy_gyro_bias_path(store.cache()).exists())),
                (
                    "note".into(),
                    Json::s("driver change pending: read <cache>/<serial>/gyro-bias, fall back to the legacy file"),
                ),
            ]),
        ),
    ])
}
