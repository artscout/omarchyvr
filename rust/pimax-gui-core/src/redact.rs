//! Redaction of everything that goes into a problem report: the home directory becomes `~`, the user and host names
//! `<user>` / `<host>`, the headset and eye chip serial numbers `<serial>`, the machine id `<machine-id>`, e-mail
//! addresses `<email>`, IP and MAC addresses `<ip>` / `<mac>`, file system UUIDs `<uuid>`, API keys `<api-key>` (`sk-ant-...`, OpenRouter/OpenAI/GitHub/Hugging Face/Google/AWS/Slack keys, bearer tokens, `api_key = ...`,
//! long values after token/secret/password, plus the key the Autofix page holds). What a report contains is listed in its README.txt, and the
//! user looks at it before sharing it by hand.
use regex_lite::Regex;
use std::sync::OnceLock;

/// Replaces personal values in text.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    /// Literal values replaced anywhere (longest first).
    literal: Vec<(String, &'static str)>,
    /// Values replaced only as whole words.
    words: Vec<(String, &'static str)>,
}

struct Patterns {
    email: Regex,
    ipv4: Regex,
    mac: Regex,
    uuid_eq: Regex,
    by_uuid: Regex,
    serial: Regex,
    api_key: Regex,
    tokens: Vec<Regex>,
    bearer: Regex,
    assign: Regex,
    secret_word: Regex,
    pimax_serial: Regex,
    ctrl_serial: Regex,
    slam_serial: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        email: Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").expect("email"),
        ipv4: Regex::new(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b").expect("ipv4"),
        mac: Regex::new(r"\b[0-9A-Fa-f]{2}(?::[0-9A-Fa-f]{2}){5}\b").expect("mac"),
        uuid_eq: Regex::new(r"UUID=[0-9A-Fa-f-]{4,}").expect("uuid"),
        by_uuid: Regex::new(r"by-uuid/[0-9A-Fa-f-]{4,}").expect("by-uuid"),
        serial: Regex::new(r"(?i)\b(i?serial(?:[ _]?number)?\s*[:=]\s*)[^\s,;)]+").expect("serial"),
        api_key: Regex::new(r"sk-ant-[A-Za-z0-9_-]{8,}").expect("api key"),
        // provider keys with a distinctive prefix (OpenRouter, OpenAI, GitHub, Hugging Face, Google, AWS, Slack)
        tokens: [
            r"sk-or-v1-[A-Za-z0-9]{8,}",
            r"sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}",
            r"github_pat_[A-Za-z0-9_]{20,}",
            r"gh[pousr]_[A-Za-z0-9]{20,}",
            r"hf_[A-Za-z0-9]{20,}",
            r"AIza[0-9A-Za-z_-]{30,}",
            r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
            r"xox[abprs]-[A-Za-z0-9-]{10,}",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("token"))
        .collect(),
        bearer: Regex::new(r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]{8,}").expect("bearer"),
        // api_key = "..." / key: '...' / "api-key": "..." (ai.toml, JSON, headers)
        assign: Regex::new(r#"(?i)\b((?:api[_-]?key|key|x-api-key)["']?\s*[:=]\s*["']?)[^\s"',;]{6,}"#).expect("assign"),
        // a long hex/base64 value after token / secret / password
        secret_word: Regex::new(r#"(?i)\b((?:[a-z_-]*(?:token|secret|passw(?:or)?d))["']?\s*[:=]\s*["']?)[A-Za-z0-9+/_=.-]{16,}"#).expect("secret"),
        // Pimax headset serials (P511G0D... also inside USB names) and Windows slam/<model>/<serial> folders
        pimax_serial: Regex::new(r"P\d{3}[A-Z]\d[A-Z][0-9A-Z]{6,}").expect("pimax serial"),
        // Pimax controller serials (H301..., as in the Windows slam/H1/<serial>/ folders and the pairing logs)
        ctrl_serial: Regex::new(r"\bH\d{5}[A-Z]\d{10,}\b").expect("controller serial"),
        slam_serial: Regex::new(r"(slam/[A-Za-z0-9]+/)[A-Za-z0-9]{6,}").expect("slam serial"),
    })
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn replace_word(text: &str, word: &str, with: &str) -> String {
    if word.is_empty() || !text.contains(word) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(word) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + word.len()..].chars().next();
        out.push_str(&rest[..i]);
        if before.is_some_and(is_word) || after.is_some_and(is_word) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[i + word.len()..];
    }
    out.push_str(rest);
    out
}

impl Redactor {
    /// From explicit values (`secrets` are serial numbers and other ids replaced anywhere).
    pub fn new(home: &str, user: &str, host: &str, machine_id: &str, secrets: &[String]) -> Redactor {
        let mut r = Redactor::default();
        let home = home.trim_end_matches('/');
        if home.len() > 1 {
            r.literal.push((home.to_string(), "~"));
        }
        if !machine_id.trim().is_empty() {
            r.literal.push((machine_id.trim().to_string(), "<machine-id>"));
        }
        for s in secrets {
            let s = s.trim();
            if s.len() >= 4 {
                r.literal.push((s.to_string(), "<serial>"));
            }
        }
        r.literal.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        // short names would replace parts of ordinary words; whole words only, and not 1-2 letter names
        if user.len() >= 3 {
            r.words.push((user.to_string(), "<user>"));
        }
        if host.len() >= 3 && host != "localhost" {
            r.words.push((host.to_string(), "<host>"));
        }
        r
    }

    /// From this system: `$HOME`, `$USER`, `/etc/hostname`, `/etc/machine-id`, plus serial numbers.
    pub fn from_system(env: &dyn Fn(&str) -> Option<String>, serials: &[String]) -> Redactor {
        let read = |p: &str| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
        let host = env("HOSTNAME").filter(|h| !h.is_empty()).unwrap_or_else(|| read("/etc/hostname"));
        Redactor::new(
            &env("HOME").unwrap_or_default(),
            &env("USER").or_else(|| env("LOGNAME")).unwrap_or_default(),
            &host,
            &read("/etc/machine-id"),
            serials,
        )
    }

    /// Also replace this secret anywhere (an API key: `<api-key>`).
    pub fn with_secret(mut self, secret: &str) -> Redactor {
        let s = secret.trim();
        if s.len() >= 8 {
            self.literal.push((s.to_string(), "<api-key>"));
            self.literal.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        }
        self
    }

    /// Redact a text.
    pub fn text(&self, s: &str) -> String {
        let p = patterns();
        // keys first (a key may look like anything else), then e-mail: it contains the user name and host-like parts
        let mut t = s.to_string();
        for (v, with) in self.literal.iter().filter(|(_, w)| *w == "<api-key>") {
            if t.contains(v.as_str()) {
                t = t.replace(v.as_str(), with);
            }
        }
        t = p.api_key.replace_all(&t, "<api-key>").into_owned();
        for re in &p.tokens {
            t = re.replace_all(&t, "<api-key>").into_owned();
        }
        t = p.bearer.replace_all(&t, "${1}<api-key>").into_owned();
        t = p.assign.replace_all(&t, "${1}<api-key>").into_owned();
        t = p.secret_word.replace_all(&t, "${1}<api-key>").into_owned();
        t = p.email.replace_all(&t, "<email>").into_owned();
        for (v, with) in &self.literal {
            if t.contains(v.as_str()) {
                t = t.replace(v.as_str(), with);
            }
        }
        for (w, with) in &self.words {
            t = replace_word(&t, w, with);
        }
        t = p.serial.replace_all(&t, "${1}<serial>").into_owned();
        t = p.pimax_serial.replace_all(&t, "<serial>").into_owned();
        t = p.ctrl_serial.replace_all(&t, "<serial>").into_owned();
        t = p.slam_serial.replace_all(&t, "${1}<serial>").into_owned();
        t = p.uuid_eq.replace_all(&t, "UUID=<uuid>").into_owned();
        t = p.by_uuid.replace_all(&t, "by-uuid/<uuid>").into_owned();
        t = p.mac.replace_all(&t, "<mac>").into_owned();
        t = p
            .ipv4
            .replace_all(&t, |c: &regex_lite::Captures| {
                let o: Vec<u32> = (1..=4).map(|i| c[i].parse().unwrap_or(999)).collect();
                let keep = o.iter().any(|x| *x > 255) || o[0] == 127 || o == [0, 0, 0, 0] || o == [255, 255, 255, 255];
                if keep {
                    c[0].to_string()
                } else {
                    "<ip>".to_string()
                }
            })
            .into_owned();
        t
    }

    /// Redact lines.
    pub fn lines(&self, v: &[String]) -> Vec<String> {
        v.iter().map(|l| self.text(l)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_personal_values() {
        let r = Redactor::new("/home/alice", "alice", "archbox", "0123456789abcdef0123456789abcdef", &["PA4F21X00123".into(), "ab".into()]);
        let t = r.text(
            "/home/alice/pimax-re/rust by alice on archbox; mail alice@example.com; headset PA4F21X00123 \
             serial=TOBII-77AB; iSerial: 0xdeadbeef1; machine 0123456789abcdef0123456789abcdef; mount UUID=1234-ABCD on \
             /dev/disk/by-uuid/0ef1-22; ip 192.168.1.20, lo 127.0.0.1, wlan 00:1a:2b:3c:4d:5e; driver 580.95.05; \
             mauricio_x stays; archboxes stays",
        );
        assert!(t.starts_with("~/pimax-re/rust by <user> on <host>; mail <email>; headset <serial> serial=<serial>; iSerial: <serial>;"), "{t}");
        assert!(t.contains("machine <machine-id>"), "{t}");
        assert!(t.contains("UUID=<uuid>") && t.contains("by-uuid/<uuid>"), "{t}");
        assert!(t.contains("ip <ip>, lo 127.0.0.1, wlan <mac>"), "{t}");
        assert!(t.contains("driver 580.95.05"), "versions stay: {t}");
        assert!(t.contains("mauricio_x stays; archboxes stays"), "only whole words: {t}");
    }

    #[test]
    fn short_names_are_left_alone() {
        let r = Redactor::new("/home/u", "u", "pc", "", &[]);
        assert_eq!(r.text("/home/u/x run by u on pc"), "~/x run by u on pc");
        assert_eq!(Redactor::default().text("1.2.3.400 is no ip"), "1.2.3.400 is no ip");
    }

    #[test]
    fn removes_api_keys() {
        let r = Redactor::new("/home/u", "u", "", "", &[]).with_secret("my-local-token-123456");
        let t = r.text("key sk-ant-api03-TEST-NOT-A-KEY-1 and my-local-token-123456 in a log");
        assert_eq!(t, "key <api-key> and <api-key> in a log");
        // too short to be a key: left alone
        assert_eq!(Redactor::default().with_secret("abc").text("abc"), "abc");
    }

    #[test]
    fn removes_provider_keys_and_secrets() {
        let r = Redactor::default();
        let fakes = [
            "sk-or-v1-TESTNOTAKEY000000000000000000000",
            "sk-AAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "sk-proj-AAAAAAAA_BBBBBBBB-CCCCCCCCCC",
            "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "github_pat_AAAAAAAAAAAAAAAAAAAAAAAA_BBBB",
            "gho_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "hf_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AIzaAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "AKIAAAAAAAAAAAAAAAAA",
            "xoxb-1111111111-2222222222-fakefake",
        ];
        for f in fakes {
            assert_eq!(r.text(&format!("k {f} end")), "k <api-key> end", "{f}");
        }
        assert_eq!(r.text("Authorization: Bearer fake.tok-en_12345678"), "Authorization: Bearer <api-key>");
        assert_eq!(r.text("api_key = \"fakevalue123\""), "api_key = \"<api-key>\"");
        assert_eq!(r.text("key = 'fakevalue123'"), "key = '<api-key>'");
        assert_eq!(r.text("password: 0123456789abcdef0123"), "password: <api-key>");
        assert_eq!(r.text("client_secret=ZmFrZWZha2VmYWtlZmFrZQ=="), "client_secret=<api-key>");
    }

    #[test]
    fn keeps_hashes_and_paths() {
        let r = Redactor::default();
        let s = "commit c64be01 full 88a81ec3ee3116395f0a8e5d7c1b2a3f4e5d6c7b /dev/dri/card1 /dev/hidraw3 key_count = 5 /dev/disk/by-id/usb-x token_count: 12";
        assert_eq!(r.text(s), s);
    }

    #[test]
    fn removes_controller_serials() {
        let r = Redactor::default();
        assert_eq!(r.text("paired H30100X000000000001 (left)"), "paired <serial> (left)");
        assert_eq!(r.text("hash H12 and 0xH30100"), "hash H12 and 0xH30100", "short ids stay");
    }

    #[test]
    fn removes_pimax_serials() {
        let r = Redactor::default();
        assert_eq!(r.text("serial P511X0X000000000001 usb-Pimax_Dream_Air_P511X0X000000000001-04"), "serial <serial> usb-Pimax_Dream_Air_<serial>-04");
        assert_eq!(r.text("C:/ProgramData/Pimax/slam/H1/FAKESERIAL01/x.json"), "C:/ProgramData/Pimax/slam/H1/<serial>/x.json");
        assert_eq!(r.text("controller 1 serial: FAKE-CTRL-1"), "controller 1 serial: <serial>");
    }
}
