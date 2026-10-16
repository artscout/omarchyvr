//! One long-lived Wayland virtual keyboard with ONE fixed keymap (replaces a `wtype` process per command).
//!
//! Why: `wtype` creates a new `zwp_virtual_keyboard_v1` with its own tiny keymap for every call. Each new
//! virtual keyboard makes Hyprland switch keymaps, and terminals lose or garble the leading character.
//! Here the voice process owns ONE keyboard for its whole life and uploads ONE fixed keymap once: every
//! printable ASCII char, Latin-1 (Spanish letters, inverted marks), the named keys we use (Return, Escape,
//! Tab, BackSpace, arrows, Home/End, Page up/down, Delete, F1..F12) and real modifier keys (Shift_L, Control_L,
//! Alt_L, Super_L, with `modifier_map` so the mask bits are the standard ones: Shift 1, Control 4, Alt 8,
//! Super 64). One keycode per keysym, one level (like wtype), all xkb keycodes <= 255 (Xwayland). About 20
//! keycodes stay spare: other Unicode chars rebind a spare keycode and upload a new keymap (rare, slower).
//!
//! Warm-up: when the keyboard is new or idle for more than 1.5 s, a Shift_L tap and a 60 ms pause come first,
//! so the compositor has switched to our keymap before the first real key (the leading-character fix).
//!
//! Fallback: any error (no seat, no manager, dead connection) is returned as `Err`; the caller drops the
//! keyboard and runs the real `wtype` program as before.
//!
//! Safety: the virtual keyboard has no target. The compositor delivers its keys to the focused surface only,
//! so typing still goes ONLY to the focused app.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd};
use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{zwp_virtual_keyboard_manager_v1, zwp_virtual_keyboard_v1};

/// An xkb keysym value.
pub type Keysym = u32;

/// A modifier key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mod {
    Shift,
    Ctrl,
    Alt,
    Logo,
}

impl Mod {
    pub fn parse(s: &str) -> Option<Mod> {
        match s.to_ascii_lowercase().as_str() {
            "shift" => Some(Mod::Shift),
            "ctrl" | "control" => Some(Mod::Ctrl),
            "alt" => Some(Mod::Alt),
            "logo" | "win" | "super" => Some(Mod::Logo),
            _ => None,
        }
    }
    /// The standard xkb modifier mask bit (Shift 1, Control 4, Mod1/Alt 8, Mod4/Super 64).
    pub fn mask(self) -> u32 {
        match self {
            Mod::Shift => 1,
            Mod::Ctrl => 4,
            Mod::Alt => 8,
            Mod::Logo => 64,
        }
    }
    pub fn keysym(self) -> Keysym {
        match self {
            Mod::Shift => KS_SHIFT_L,
            Mod::Ctrl => KS_CONTROL_L,
            Mod::Alt => KS_ALT_L,
            Mod::Logo => KS_SUPER_L,
        }
    }
}

/// One step of a wtype-style command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    ModDown(Mod),
    ModUp(Mod),
    /// Press and release one key.
    Key(Keysym),
    Text(String),
    /// Sleep (ms).
    Sleep(u32),
    /// Delay between keys from now on (ms).
    Delay(u32),
}

const KS_SHIFT_L: Keysym = 0xffe1;
const KS_CONTROL_L: Keysym = 0xffe3;
const KS_ALT_L: Keysym = 0xffe9;
const KS_SUPER_L: Keysym = 0xffeb;

/// Named (non-character) keys of the fixed keymap.
const NAMED: &[(&str, Keysym)] = &[
    ("Return", 0xff0d),
    ("Escape", 0xff1b),
    ("Tab", 0xff09),
    ("BackSpace", 0xff08),
    ("Delete", 0xffff),
    ("Next", 0xff56),
    ("Prior", 0xff55),
    ("Home", 0xff50),
    ("End", 0xff57),
    ("Left", 0xff51),
    ("Up", 0xff52),
    ("Right", 0xff53),
    ("Down", 0xff54),
    ("F1", 0xffbe),
    ("F2", 0xffbf),
    ("F3", 0xffc0),
    ("F4", 0xffc1),
    ("F5", 0xffc2),
    ("F6", 0xffc3),
    ("F7", 0xffc4),
    ("F8", 0xffc5),
    ("F9", 0xffc6),
    ("F10", 0xffc7),
    ("F11", 0xffc8),
    ("F12", 0xffc9),
    ("Shift_L", KS_SHIFT_L),
    ("Control_L", KS_CONTROL_L),
    ("Alt_L", KS_ALT_L),
    ("Super_L", KS_SUPER_L),
];

/// Keysym of a `-k` argument: a named key, an F-key, or a single character.
pub fn keysym_from_name(name: &str) -> Option<Keysym> {
    if let Some((_, k)) = NAMED.iter().find(|(n, _)| *n == name) {
        return Some(*k);
    }
    if name.eq_ignore_ascii_case("space") {
        return Some(0x20);
    }
    let mut it = name.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(char_keysym(c)),
        _ => None,
    }
}

/// Keysym of a character (Latin-1 keysyms equal the code point; other chars use the Unicode keysym range).
pub fn char_keysym(c: char) -> Keysym {
    let u = c as u32;
    if (0x20..=0x7e).contains(&u) || (0xa0..=0xff).contains(&u) {
        u
    } else if c == '\n' {
        0xff0d
    } else if c == '\t' {
        0xff09
    } else {
        0x0100_0000 + u
    }
}

/// Parse the wtype subset (arguments WITHOUT the leading "wtype").
pub fn parse_wtype(args: &[String]) -> Result<Vec<Op>, String> {
    let mut ops = Vec::new();
    let mut i = 0;
    let need = |i: usize, flag: &str| args.get(i + 1).ok_or_else(|| format!("wtype: {flag} needs a value"));
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-M" | "-m" => {
                let v = need(i, a)?;
                let m = Mod::parse(v).ok_or_else(|| format!("wtype: unknown modifier {v}"))?;
                ops.push(if a == "-M" { Op::ModDown(m) } else { Op::ModUp(m) });
                i += 2;
            }
            "-k" => {
                let v = need(i, a)?;
                ops.push(Op::Key(keysym_from_name(v).ok_or_else(|| format!("wtype: unknown key {v}"))?));
                i += 2;
            }
            "-s" | "-d" => {
                let v = need(i, a)?;
                let ms: u32 = v.parse().map_err(|_| format!("wtype: bad number {v}"))?;
                ops.push(if a == "-s" { Op::Sleep(ms) } else { Op::Delay(ms) });
                i += 2;
            }
            "--" => {
                if i + 1 < args.len() {
                    ops.push(Op::Text(args[i + 1..].join(" ")));
                }
                i = args.len();
            }
            _ if a.starts_with('-') && a.len() > 1 => return Err(format!("wtype: unsupported option {a}")),
            _ => {
                ops.push(Op::Text(a.to_string()));
                i += 1;
            }
        }
    }
    Ok(ops)
}

/// Keysyms of the fixed keymap, in keycode order (xkb keycode = `FIRST_KEYCODE + index`).
fn base_keysyms() -> Vec<Keysym> {
    let mut v: Vec<Keysym> = (0x20..=0x7eu32).collect();
    v.extend(0xa0..=0xffu32);
    v.extend(NAMED.iter().map(|(_, k)| *k));
    v
}

const FIRST_KEYCODE: u32 = 9;
const MAX_KEYCODE: u32 = 255;
const SPARE: u32 = 20;

/// The keymap layout: fixed keysyms plus the spare keycodes currently bound to extra keysyms.
struct Layout {
    base: Vec<Keysym>,
    index: HashMap<Keysym, u32>,
    extra: Vec<Keysym>,
}

impl Layout {
    fn new() -> Self {
        let base = base_keysyms();
        let index = base.iter().enumerate().map(|(i, k)| (*k, FIRST_KEYCODE + i as u32)).collect();
        Layout { base, index, extra: Vec::new() }
    }
    fn spare_first(&self) -> u32 {
        FIRST_KEYCODE + self.base.len() as u32
    }
    fn spare_count(&self) -> u32 {
        (MAX_KEYCODE + 1 - self.spare_first()).min(SPARE)
    }
    /// xkb keycode of a keysym, if bound.
    fn keycode(&self, k: Keysym) -> Option<u32> {
        self.index.get(&k).copied().or_else(|| self.extra.iter().position(|e| *e == k).map(|p| self.spare_first() + p as u32))
    }
    /// Bind a keysym to a spare keycode (the oldest is recycled when all are used). Returns the keycode.
    fn bind_extra(&mut self, k: Keysym) -> u32 {
        let n = self.spare_count() as usize;
        if self.extra.len() < n {
            self.extra.push(k);
        } else {
            self.extra.rotate_left(1);
            *self.extra.last_mut().unwrap() = k;
        }
        self.spare_first() + (self.extra.len() - 1) as u32
    }
    fn keymap_text(&self) -> String {
        let mut codes = String::new();
        let mut syms = String::new();
        let all = self.base.iter().enumerate().map(|(i, k)| (FIRST_KEYCODE + i as u32, *k)).chain(self.extra.iter().enumerate().map(|(i, k)| (self.spare_first() + i as u32, *k)));
        for (code, k) in all {
            codes.push_str(&format!("<K{code}> = {code};\n"));
            syms.push_str(&format!("key <K{code}> {{[ {} ]}};\n", sym_text(k)));
        }
        let mut mods = String::new();
        for (m, name) in [(Mod::Shift, "Shift"), (Mod::Ctrl, "Control"), (Mod::Alt, "Mod1"), (Mod::Logo, "Mod4")] {
            if let Some(c) = self.index.get(&m.keysym()) {
                mods.push_str(&format!("modifier_map {name} {{ <K{c}> }};\n"));
            }
        }
        format!(
            "xkb_keymap {{\nxkb_keycodes \"pimax\" {{\nminimum = 8;\nmaximum = {MAX_KEYCODE};\n{codes}}};\nxkb_types \"pimax\" {{ include \"complete\" }};\nxkb_compat \"pimax\" {{ include \"complete\" }};\nxkb_symbols \"pimax\" {{\n{syms}{mods}}};\n}};\n"
        )
    }
}

/// How a keysym is written in the keymap: a name for named keys and letters, hex otherwise
/// (xkbcommon accepts a hex number as a keysym; digits are hex too, which avoids the 0-9 special case).
fn sym_text(k: Keysym) -> String {
    if let Some((n, _)) = NAMED.iter().find(|(_, v)| *v == k) {
        return (*n).to_string();
    }
    if let Some(c) = char::from_u32(k) {
        if c.is_ascii_alphabetic() {
            return c.to_string();
        }
    }
    format!("0x{k:04x}")
}

/// Text to (char, keycode) steps against a layout without binding anything (None = not in the fixed map).
#[cfg(test)]
fn text_keycodes(layout: &Layout, text: &str) -> Vec<(char, Option<u32>)> {
    text.chars().map(|c| (c, layout.keycode(char_keysym(c)))).collect()
}

struct State;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, ()> for State {
    fn event(_: &mut Self, _: &zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, _: zwp_virtual_keyboard_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, ()> for State {
    fn event(_: &mut Self, _: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, _: zwp_virtual_keyboard_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// The persistent virtual keyboard.
pub struct Keyboard {
    conn: Connection,
    queue: EventQueue<State>,
    kb: zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
    layout: Layout,
    t0: Instant,
    last_use: Option<Instant>,
    mods: u32,
}

const WARMUP_IDLE: Duration = Duration::from_millis(1500);
const SLEEP_CAP_MS: u32 = 200;
const DEFAULT_DELAY_MS: u32 = 4;

impl Keyboard {
    /// Connect to the compositor (WAYLAND_DISPLAY), create the keyboard and upload the fixed keymap.
    pub fn connect() -> Result<Self, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("wayland connect: {e}"))?;
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
        let qh = queue.handle();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("wl_seat: {e}"))?;
        let mgr: zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("zwp_virtual_keyboard_manager_v1: {e}"))?;
        let kb = mgr.create_virtual_keyboard(&seat, &qh, ());
        let mut me = Keyboard { conn, queue, kb, layout: Layout::new(), t0: Instant::now(), last_use: None, mods: 0 };
        me.upload_keymap()?;
        Ok(me)
    }

    fn upload_keymap(&mut self) -> Result<(), String> {
        let mut text = self.layout.keymap_text().into_bytes();
        text.push(0);
        let name = std::ffi::CString::new("pimax-vkbd").unwrap();
        // SAFETY: plain memfd_create; the fd is owned by the File below.
        let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(format!("memfd_create: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: fd is a fresh, valid, owned descriptor.
        let mut f = unsafe { File::from_raw_fd(fd) };
        f.write_all(&text).map_err(|e| format!("keymap write: {e}"))?;
        self.kb.keymap(1, f.as_fd(), text.len() as u32); // XKB_KEYMAP_FORMAT_TEXT_V1
        self.roundtrip()
    }

    fn roundtrip(&mut self) -> Result<(), String> {
        self.queue.roundtrip(&mut State).map(|_| ()).map_err(|e| format!("wayland: {e}"))
    }

    fn flush(&self) -> Result<(), String> {
        self.conn.flush().map_err(|e| format!("wayland flush: {e}"))
    }

    fn now(&self) -> u32 {
        self.t0.elapsed().as_millis() as u32
    }

    fn key(&mut self, keycode: u32, press: bool) {
        // wire keycodes are evdev codes: xkb keycode - 8
        self.kb.key(self.now(), keycode - 8, u32::from(press));
    }

    fn send_mods(&mut self) {
        self.kb.modifiers(self.mods, 0, 0, 0);
    }

    fn tap(&mut self, keycode: u32, delay: u32) -> Result<(), String> {
        self.key(keycode, true);
        self.flush()?;
        std::thread::sleep(Duration::from_millis(u64::from(delay)));
        self.key(keycode, false);
        self.flush()?;
        std::thread::sleep(Duration::from_millis(u64::from(delay)));
        Ok(())
    }

    fn type_keysym(&mut self, k: Keysym, delay: u32) -> Result<(), String> {
        let code = match self.layout.keycode(k) {
            Some(c) => c,
            None => {
                // rare path: rebind a spare keycode, upload the new keymap, give the compositor time
                let c = self.layout.bind_extra(k);
                self.upload_keymap()?;
                std::thread::sleep(Duration::from_millis(60));
                c
            }
        };
        self.tap(code, delay)
    }

    /// Run the ops. Err means the connection is unusable: the caller drops the keyboard.
    pub fn run(&mut self, ops: &[Op]) -> Result<(), String> {
        let r = self.run_inner(ops);
        // never leave a modifier down
        if self.mods != 0 {
            for m in [Mod::Shift, Mod::Ctrl, Mod::Alt, Mod::Logo] {
                if self.mods & m.mask() != 0 {
                    if let Some(c) = self.layout.keycode(m.keysym()) {
                        self.key(c, false);
                    }
                }
            }
            self.mods = 0;
            self.send_mods();
            let _ = self.flush();
        }
        r?;
        self.last_use = Some(Instant::now());
        self.roundtrip()
    }

    fn run_inner(&mut self, ops: &[Op]) -> Result<(), String> {
        if self.last_use.is_none_or(|t| t.elapsed() > WARMUP_IDLE) {
            if let Some(c) = self.layout.keycode(KS_SHIFT_L) {
                self.tap(c, DEFAULT_DELAY_MS)?;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        let mut delay = DEFAULT_DELAY_MS;
        for op in ops {
            match op {
                Op::Delay(d) => delay = *d,
                Op::Sleep(ms) => {
                    self.flush()?;
                    std::thread::sleep(Duration::from_millis(u64::from((*ms).min(SLEEP_CAP_MS))));
                }
                Op::ModDown(m) | Op::ModUp(m) => {
                    let down = matches!(op, Op::ModDown(_));
                    let c = self.layout.keycode(m.keysym()).ok_or("modifier not in keymap")?;
                    self.key(c, down);
                    if down {
                        self.mods |= m.mask();
                    } else {
                        self.mods &= !m.mask();
                    }
                    self.send_mods();
                    self.flush()?;
                    std::thread::sleep(Duration::from_millis(u64::from(delay)));
                }
                Op::Key(k) => self.type_keysym(*k, delay)?,
                Op::Text(t) => {
                    for c in t.chars() {
                        self.type_keysym(char_keysym(c), delay)?;
                    }
                }
            }
        }
        self.flush()
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.kb.destroy();
        let _ = self.conn.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_every_desk_form() {
        assert_eq!(parse_wtype(&a(&["-s", "80", "-d", "4", "--", "hola mundo"])).unwrap(), vec![Op::Sleep(80), Op::Delay(4), Op::Text("hola mundo".into())]);
        assert_eq!(parse_wtype(&a(&["-k", "Return"])).unwrap(), vec![Op::Key(0xff0d)]);
        assert_eq!(
            parse_wtype(&a(&["-M", "ctrl", "-M", "shift", "-k", "c", "-m", "shift", "-m", "ctrl"])).unwrap(),
            vec![Op::ModDown(Mod::Ctrl), Op::ModDown(Mod::Shift), Op::Key('c' as u32), Op::ModUp(Mod::Shift), Op::ModUp(Mod::Ctrl)]
        );
        assert_eq!(parse_wtype(&a(&["-M", "ctrl", "-k", "Tab", "-m", "ctrl"])).unwrap(), vec![Op::ModDown(Mod::Ctrl), Op::Key(0xff09), Op::ModUp(Mod::Ctrl)]);
        assert_eq!(parse_wtype(&a(&["-M", "alt", "-k", "Left", "-m", "alt"])).unwrap()[1], Op::Key(0xff51));
        for k in ["Escape", "BackSpace", "space", "Next", "Prior", "Home", "End", "F5", "Right", "Up", "Down", "Delete", "a", "7"] {
            assert!(parse_wtype(&a(&["-k", k])).is_ok(), "{k}");
        }
        assert_eq!(parse_wtype(&a(&["hello"])).unwrap(), vec![Op::Text("hello".into())]);
        assert!(parse_wtype(&a(&["-k"])).is_err());
        assert!(parse_wtype(&a(&["-k", "Nope"])).is_err());
        assert!(parse_wtype(&a(&["-M", "hyper"])).is_err());
        assert!(parse_wtype(&a(&["-x"])).is_err());
        assert_eq!(parse_wtype(&a(&["-M", "win", "-m", "super"])).unwrap(), vec![Op::ModDown(Mod::Logo), Op::ModUp(Mod::Logo)]);
    }

    /// xkbcommon itself compiles the keymap (skipped where xkbcli is not installed), and the compiled map still
    /// has the keysyms written as hex numbers (punctuation, Latin-1) and the modifier map.
    #[test]
    fn xkbcommon_compiles_the_keymap() {
        let Ok(st) = std::process::Command::new("xkbcli").arg("--help").output() else { return };
        let _ = st;
        let mut l = Layout::new();
        l.bind_extra(char_keysym('€'));
        let f = std::env::temp_dir().join(format!("pimax-vkbd-{}.xkb", std::process::id()));
        std::fs::write(&f, l.keymap_text()).unwrap();
        let o = std::process::Command::new("xkbcli").args(["compile-keymap", "--keymap"]).arg(&f).output().unwrap();
        let _ = std::fs::remove_file(&f);
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success() && out.contains("xkb_symbols"), "{}", String::from_utf8_lossy(&o.stderr));
        assert!(out.contains("EuroSign") || out.contains("U20AC"), "the rebound spare key");
        for sym in ["ntilde", "questiondown", "exclamdown", "comma", "Return", "Control_L"] {
            assert!(out.contains(sym), "{sym} missing in the compiled keymap");
        }
        assert!(out.contains("modifier_map Control") || out.contains("Control"), "modifiers");
    }

    #[test]
    fn keymap_is_valid_and_complete() {
        let l = Layout::new();
        let text = l.keymap_text();
        let mut seen = std::collections::HashSet::new();
        let mut max = 0u32;
        for line in text.lines().filter(|l| l.starts_with("<K")) {
            let code: u32 = line.trim_end_matches(';').rsplit(' ').next().unwrap().parse().unwrap();
            assert!(seen.insert(code), "duplicate keycode {code}");
            max = max.max(code);
        }
        assert!(max <= 255);
        assert!(l.spare_count() >= 20, "spare {}", l.spare_count());
        assert!(l.spare_first() + l.spare_count() - 1 <= 255);
        assert_eq!(l.base.len(), l.index.len(), "duplicate keysyms");
        for c in (0x20u8..=0x7e).map(|b| b as char).chain("ñáéíóúüÑÁÉÍÓÚÜ¿¡".chars()) {
            assert!(l.keycode(char_keysym(c)).is_some(), "missing {c:?}");
        }
        for (n, _) in NAMED {
            assert!(l.keycode(keysym_from_name(n).unwrap()).is_some(), "{n}");
        }
        for m in ["Shift", "Control", "Mod1", "Mod4"] {
            assert!(text.contains(&format!("modifier_map {m} ")), "{m}");
        }
        assert!(text.contains("xkb_symbols") && text.contains("include \"complete\""));
    }

    #[test]
    fn text_maps_to_keycodes_without_shift() {
        let l = Layout::new();
        let v = text_keycodes(&l, "Hello, ñ!");
        assert_eq!(v.len(), 9);
        assert!(v.iter().all(|(_, k)| k.is_some()));
        let codes: Vec<u32> = v.iter().map(|(_, k)| k.unwrap()).collect();
        assert_ne!(codes[0], codes[1]); // 'H' and 'e' are different keys
        assert_eq!(codes[2], codes[3]); // 'l' twice
        assert!(text_keycodes(&l, "€")[0].1.is_none());
    }

    #[test]
    fn extra_chars_use_spare_keycodes() {
        let mut l = Layout::new();
        let k = char_keysym('€');
        assert_eq!(k, 0x0100_20ac);
        let c = l.bind_extra(k);
        assert_eq!(l.keycode(k), Some(c));
        assert!(c <= 255);
        for i in 0..40u32 {
            let c = l.bind_extra(0x0100_3000 + i);
            assert!((l.spare_first()..=255).contains(&c));
        }
        assert!(l.keymap_text().contains("0x1003027"));
    }

    #[test]
    fn modifier_masks() {
        assert_eq!(Mod::Shift.mask(), 1);
        assert_eq!(Mod::Ctrl.mask(), 4);
        assert_eq!(Mod::Alt.mask(), 8);
        assert_eq!(Mod::Logo.mask(), 64);
        assert_eq!(Mod::parse("super"), Some(Mod::Logo));
    }
}
