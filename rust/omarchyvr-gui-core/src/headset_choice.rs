//! "Which headset do you have?": the first step of the automatic setup, and what it decides.
//!
//! The person's choice (saved as `headset = dream-air` / `other` in `gui-state.toml`) drives the root plan: a model
//! that needs the NVIDIA display patch (`display.needs` has `nvidia_2heads1or_patches`; the Dream Air) gets the patch
//! on a tested or allowed driver and the EDID step (`OVR_EDID=auto`); "Other / not listed" gets no headset-specific
//! system change. USB detection only PRE-SELECTS the choice; it never decides. A sleeping Dream Air hides
//! `34a4:0044` (real install, 2026-10-17), so its sleeping signature counts too.
//! The same rule is `headset_state` in `installer/lib.sh` (same test vectors).
use crate::headsets;

/// What the person chose (the file value is the model id, or `other`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadsetPick {
    /// Not chosen yet: the setup asks first.
    #[default]
    Unset,
    /// A supported model: the index in [`models`].
    Model(u8),
    /// "Other / not listed".
    Other,
}

/// The supported (real) headset models, in the order of the list on screen.
pub fn models() -> Vec<&'static dyn headsets::HeadsetModule> {
    headsets::all().iter().filter(|m| !m.is_mock()).map(|b| b.as_ref()).collect()
}

impl HeadsetPick {
    /// The file value.
    pub fn name(self) -> String {
        match self {
            HeadsetPick::Unset => "none".into(),
            HeadsetPick::Other => "other".into(),
            HeadsetPick::Model(i) => models().get(i as usize).map_or("other".into(), |m| m.id().to_string()),
        }
    }

    /// Parse a file value (an unknown model id is not a choice).
    pub fn parse(s: &str) -> HeadsetPick {
        match s.trim() {
            "other" => HeadsetPick::Other,
            id => models().iter().position(|m| m.id() == id).map_or(HeadsetPick::Unset, |i| HeadsetPick::Model(i as u8)),
        }
    }

    /// The model, when one is chosen.
    pub fn module(self) -> Option<&'static dyn headsets::HeadsetModule> {
        match self {
            HeadsetPick::Model(i) => models().get(i as usize).copied(),
            _ => None,
        }
    }

    /// The name on screen.
    pub fn title(self) -> String {
        match self {
            HeadsetPick::Unset => "not chosen".into(),
            HeadsetPick::Other => "Other / not listed".into(),
            HeadsetPick::Model(_) => self.module().map_or("?".into(), |m| m.descriptor().name.clone()),
        }
    }

    /// The model needs the NVIDIA display patch and the EDID override.
    pub fn needs_nvidia_patch(self) -> bool {
        self.module().is_some_and(|m| m.descriptor().caps.display_needs.iter().any(|n| n == "nvidia_2heads1or_patches"))
    }
}

/// What the USB bus says about the headset: the Dream Air (34a4:0044, with its sleeping signature) or the Crystal
/// Super (34a4:0040, on USB whenever it has power: Windows capture 2026-10-17, takes cold1/cold2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadsetUsb {
    /// `34a4:0044` is there.
    Awake,
    /// It sleeps: only its control device `34a4:0043` with the camera `34a4:0014` and/or the eye chip `2104:0561`.
    Asleep,
    /// Neither.
    Absent,
}

/// The rule (shared with `installer/lib.sh`).
pub fn headset_state_from_usb(usb: &[(u16, u16)]) -> HeadsetUsb {
    let has = |v: u16, p: u16| usb.contains(&(v, p));
    if has(0x34a4, 0x0044) || has(0x34a4, 0x0040) {
        HeadsetUsb::Awake
    } else if has(0x34a4, 0x0043) && (has(0x34a4, 0x0014) || has(0x2104, 0x0561)) {
        HeadsetUsb::Asleep
    } else {
        HeadsetUsb::Absent
    }
}

/// What the "Get the headset ready" guide waits for. `need_camera` false: the control chip `34a4:0044` (the setup's
/// first card; same as [`headset_state_from_usb`]). True: also the cameras `34a4:0014` (a Work/Play start or the setup
/// found them missing; real start 2026-10-17 13:07: the control chip was there, the cameras were not, and only a power
/// cycle of the headset brought them back). The control chip there but a needed part missing counts as asleep: the
/// guide's steps (power button, then the power cable as the last resort) apply.
pub fn guide_state(usb: &[(u16, u16)], need_camera: bool) -> HeadsetUsb {
    let s = headset_state_from_usb(usb);
    if !need_camera || s != HeadsetUsb::Awake {
        return s;
    }
    if usb.contains(&(0x34a4, 0x0014)) {
        HeadsetUsb::Awake
    } else {
        HeadsetUsb::Asleep
    }
}

/// The model to pre-select from USB, and the line that says why ("Found on USB: Pimax Dream Air (asleep)").
/// Nothing detected: nothing is pre-selected.
pub fn preselect(usb: &[(u16, u16)]) -> Option<(HeadsetPick, String)> {
    let id = if usb.contains(&(0x34a4, 0x0040)) { "crystal-super" } else { "dream-air" };
    let i = models().iter().position(|m| m.id() == id)?;
    let name = models()[i].descriptor().name.clone();
    let name = name.split(" (").next().unwrap_or(&name).to_string();
    match headset_state_from_usb(usb) {
        HeadsetUsb::Awake => Some((HeadsetPick::Model(i as u8), format!("Found on USB: {name}"))),
        HeadsetUsb::Asleep => Some((HeadsetPick::Model(i as u8), format!("Found on USB: {name} (asleep)"))),
        HeadsetUsb::Absent => None,
    }
}

/// A line of the "Get the headset ready" card: the step in plain words and, where the PC can tell, whether it is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyStep {
    pub text: String,
    /// `Some(true)` seen, `Some(false)` not seen yet, `None` the PC cannot tell.
    pub seen: Option<bool>,
}

/// The steps for the chosen model. `connector`: the headset's DisplayPort connector is there (doctor check).
pub fn ready_steps(pick: HeadsetPick, usb: &[(u16, u16)], connector: Option<bool>) -> Vec<ReadyStep> {
    let s = |t: &str, seen| ReadyStep { text: t.into(), seen };
    if pick.module().is_none_or(|m| m.id() != "dream-air") {
        return vec![];
    }
    let on_usb = headset_state_from_usb(usb) != HeadsetUsb::Absent;
    vec![
        s("Connect the DisplayPort cable to the graphics card (not the motherboard).", connector),
        s("Connect the USB cable.", Some(on_usb)),
        s("Power the headset on. It can stay on the desk.", None),
        s("It may go to sleep. That is fine: it wakes up during the setup.", None),
    ]
}

/// The plan the root job reads, for the chosen headset. `usb` only tells the job how long to wait for the display
/// (a sleeping or unseen headset 60 s, an awake one 30 s). The plan from `./install.sh` is never trusted: the GPU and
/// driver verdict it recorded (`OVR_NVIDIA_WANT`; an old plan without the key: a non-empty driver version) stays,
/// the headset part is rewritten from the choice.
pub fn plan_for_pick(text: &str, pick: HeadsetPick, usb: HeadsetUsb) -> String {
    match pick {
        HeadsetPick::Unset => text.to_string(),
        p if !p.needs_nvidia_patch() => {
            // "Other / not listed" (or a model without display patches): no NVIDIA and no EDID step
            let t = plan_set(text, "OVR_HEADSET", "none");
            let t = plan_set(&t, "OVR_NVIDIA", "skip");
            plan_set(&t, "OVR_EDID", "")
        }
        _ => {
            let mut out = plan_set(text, "OVR_HEADSET", if usb == HeadsetUsb::Awake { "awake" } else { "asleep" });
            let want = match plan_value(&out, "OVR_NVIDIA_WANT") {
                Some(w) => w == "patch",
                None => plan_value(&out, "OVR_NVIDIA_VERSION").is_some_and(|v| !v.is_empty()),
            };
            if want {
                out = plan_set(&out, "OVR_NVIDIA", "patch");
                if plan_value(&out, "OVR_EDID").is_none_or(str::is_empty) {
                    out = plan_set(&out, "OVR_EDID", "auto");
                }
            }
            out
        }
    }
}

fn plan_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| l.trim().strip_prefix(key).and_then(|r| r.strip_prefix('=')).map(|v| v.trim().trim_matches('"')))
}

fn plan_set(text: &str, key: &str, val: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| {
            if l.trim().strip_prefix(key).is_some_and(|r| r.starts_with('=')) {
                found = true;
                format!("{key}={val}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{key}={val}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DREAM: HeadsetPick = HeadsetPick::Model(0);

    #[test]
    fn the_dream_air_and_the_crystal_super_are_listed_and_the_choice_round_trips() {
        assert_eq!(models().iter().map(|m| m.id()).collect::<Vec<_>>(), ["dream-air", "crystal-super"]);
        let crystal = HeadsetPick::Model(1);
        assert_eq!(HeadsetPick::parse("crystal-super"), crystal);
        assert_eq!(crystal.name(), "crystal-super");
        assert!(!crystal.needs_nvidia_patch(), "not known for the Crystal Super yet");
        assert_eq!(DREAM.name(), "dream-air");
        assert_eq!(HeadsetPick::parse("dream-air"), DREAM);
        assert_eq!(HeadsetPick::parse("other"), HeadsetPick::Other);
        assert_eq!(HeadsetPick::parse("none"), HeadsetPick::Unset);
        assert_eq!(HeadsetPick::parse("some-mock"), HeadsetPick::Unset, "an unknown id is not a choice");
        assert!(DREAM.needs_nvidia_patch());
        assert!(!HeadsetPick::Other.needs_nvidia_patch());
    }

    #[test]
    fn the_headset_state_from_usb_ids() {
        let u = headset_state_from_usb;
        assert_eq!(u(&[(0x1d6b, 3), (0x34a4, 0x43), (0x34a4, 0x44), (0x34a4, 0x14), (0x2104, 0x561)]), HeadsetUsb::Awake);
        assert_eq!(u(&[(0x34a4, 0x44)]), HeadsetUsb::Awake);
        assert_eq!(u(&[(0x34a4, 0x40), (0x34a4, 0x41), (0x34a4, 0x14)]), HeadsetUsb::Awake, "Crystal Super");
        assert_eq!(u(&[(0x34a4, 0x41)]), HeadsetUsb::Absent, "its bridge alone is not a headset");
        assert_eq!(u(&[(0x34a4, 0x43), (0x34a4, 0x14), (0x2104, 0x561)]), HeadsetUsb::Asleep);
        assert_eq!(u(&[(0x34a4, 0x43), (0x34a4, 0x14)]), HeadsetUsb::Asleep);
        assert_eq!(u(&[(0x34a4, 0x43), (0x2104, 0x561)]), HeadsetUsb::Asleep);
        assert_eq!(u(&[(0x34a4, 0x14)]), HeadsetUsb::Absent, "the camera alone is not a headset");
        assert_eq!(u(&[(0x34a4, 0x43)]), HeadsetUsb::Absent);
        assert_eq!(u(&[(0x2104, 0x561)]), HeadsetUsb::Absent);
        assert_eq!(u(&[]), HeadsetUsb::Absent);
    }

    #[test]
    fn the_guide_waits_for_the_control_chip_and_when_asked_for_the_cameras() {
        let awake_no_cam = [(0x34a4, 0x43), (0x34a4, 0x44)];
        let all = [(0x34a4, 0x43), (0x34a4, 0x44), (0x34a4, 0x14), (0x2104, 0x561)];
        assert_eq!(guide_state(&awake_no_cam, false), HeadsetUsb::Awake, "the first card only needs the control chip");
        assert_eq!(guide_state(&awake_no_cam, true), HeadsetUsb::Asleep, "cameras missing: the guide's steps apply");
        assert_eq!(guide_state(&all, true), HeadsetUsb::Awake);
        // the eye tracker is switched on by the session (or the setup's eye step): not a reason for a power cycle
        assert_eq!(guide_state(&[(0x34a4, 0x43), (0x34a4, 0x44), (0x34a4, 0x14)], true), HeadsetUsb::Awake);
        for need in [false, true] {
            assert_eq!(guide_state(&[(0x34a4, 0x43), (0x34a4, 0x14)], need), HeadsetUsb::Asleep);
            assert_eq!(guide_state(&[], need), HeadsetUsb::Absent);
        }
    }

    #[test]
    fn detection_only_preselects() {
        let (p, why) = preselect(&[(0x34a4, 0x44)]).expect("awake");
        assert_eq!((p, why.as_str()), (DREAM, "Found on USB: Pimax Dream Air"));
        let (p, why) = preselect(&[(0x34a4, 0x43), (0x34a4, 0x14), (0x2104, 0x561)]).expect("asleep");
        assert_eq!((p, why.as_str()), (DREAM, "Found on USB: Pimax Dream Air (asleep)"));
        let (p, why) = preselect(&[(0x34a4, 0x40), (0x34a4, 0x41)]).expect("crystal");
        assert_eq!((p.name().as_str(), why.as_str()), ("crystal-super", "Found on USB: Pimax Crystal Super"));
        assert!(preselect(&[]).is_none(), "nothing detected: nothing pre-selected");
        assert!(preselect(&[(0x34a4, 0x14)]).is_none());
    }

    #[test]
    fn the_ready_card_ticks_come_from_detection() {
        assert!(ready_steps(HeadsetPick::Other, &[], None).is_empty());
        let none = ready_steps(DREAM, &[], Some(false));
        assert_eq!(none[0].seen, Some(false));
        assert_eq!(none[1].seen, Some(false));
        let asleep = ready_steps(DREAM, &[(0x34a4, 0x43), (0x34a4, 0x14)], Some(true));
        assert_eq!((asleep[0].seen, asleep[1].seen, asleep[2].seen), (Some(true), Some(true), None));
        assert_eq!(ready_steps(DREAM, &[], None)[0].seen, None, "the PC cannot tell");
    }

    #[test]
    fn the_plan_follows_the_choice_not_the_install_time_guess() {
        // ./install.sh wrote this with the headset asleep: "NVIDIA not needed"
        let stale = "OVR_USER=u\nOVR_NVIDIA=skip\nOVR_NVIDIA_VERSION=610.57.04\nOVR_EDID=\nOVR_NVIDIA_WANT=patch\nOVR_HEADSET=app\nOVR_SNAPSHOT=yes\n";
        for usb in [HeadsetUsb::Asleep, HeadsetUsb::Absent] {
            let t = plan_for_pick(stale, DREAM, usb);
            assert!(t.contains("OVR_NVIDIA=patch\n") && t.contains("OVR_EDID=auto\n") && t.contains("OVR_HEADSET=asleep\n"), "{t}");
        }
        assert!(plan_for_pick(stale, DREAM, HeadsetUsb::Awake).contains("OVR_HEADSET=awake\n"));
        // other: no NVIDIA, no EDID, even if a key was there
        let keyed = "OVR_NVIDIA=patch\nOVR_EDID=override.[0000:01:00.0].DP-0\n";
        let t = plan_for_pick(keyed, HeadsetPick::Other, HeadsetUsb::Awake);
        assert!(t.contains("OVR_NVIDIA=skip\n") && t.contains("OVR_EDID=\n") && t.contains("OVR_HEADSET=none\n"), "{t}");
        // not chosen: untouched
        assert_eq!(plan_for_pick(stale, HeadsetPick::Unset, HeadsetUsb::Awake), stale);
        // the GPU said no (no NVIDIA / Secure Boot): the choice cannot override it
        let no_gpu = "OVR_NVIDIA=skip\nOVR_NVIDIA_VERSION=\nOVR_NVIDIA_WANT=skip\nOVR_EDID=\n";
        let t = plan_for_pick(no_gpu, DREAM, HeadsetUsb::Asleep);
        assert!(t.contains("OVR_NVIDIA=skip\n") && t.contains("OVR_EDID=\n"), "{t}");
        // an old plan without the new keys: judged by the driver version
        let old = "OVR_NVIDIA=skip\nOVR_NVIDIA_VERSION=610.57.04\nOVR_EDID=\n";
        assert!(plan_for_pick(old, DREAM, HeadsetUsb::Asleep).contains("OVR_NVIDIA=patch\n"));
        // a detected key stays
        let t = plan_for_pick(keyed, DREAM, HeadsetUsb::Awake);
        assert!(t.contains("OVR_EDID=override.[0000:01:00.0].DP-0\n"), "{t}");
        // the root job accepts only safe characters
        assert!(plan_for_pick(stale, DREAM, HeadsetUsb::Asleep).lines().all(|l| l.chars().all(|c| c.is_ascii_alphanumeric() || "_=./:@+ -".contains(c))));
    }
}
