//! "Report a problem": one `omarchyvr-report-<utc>.tar.gz` in `~/.local/state/omarchyvr/reports/` with what is needed to look
//! at a problem: a summary (version, system, what runs), the doctor report, the setup checklist, the desk preflight,
//! this boot's GPU kernel events, the app log, the session logs, the last job outputs and the crash reports. Every
//! text goes through [`Redactor`]. Minidumps (raw process memory) are only added when the user asks for them.
//!
//! The file stays on this machine; the user shares it by hand (see [`crate::upload`]).
use crate::redact::Redactor;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Everything a report is made of (collected on the UI thread from its snapshots; files are read in the worker).
#[derive(Debug, Clone)]
pub struct Input {
    /// `~/.local/state/omarchyvr`.
    pub state_dir: PathBuf,
    /// Where the report goes.
    pub out_dir: PathBuf,
    /// Compact UTC time for the name.
    pub stamp: String,
    /// summary.txt.
    pub summary: String,
    /// The doctor report as text and JSON.
    pub doctor: Option<(String, String)>,
    /// checklist.txt.
    pub checklist: String,
    /// preflight.txt.
    pub preflight: String,
    /// kernel-gpu.txt.
    pub kernel: String,
    /// Session logs to add (their tails).
    pub session_logs: Vec<PathBuf>,
    /// Add minidumps.
    pub include_minidumps: bool,
    /// Redaction.
    pub redactor: Redactor,
}

/// Bytes of each log kept (from the end).
pub const LOG_TAIL: u64 = 1 << 20;

/// A minimal ustar + gzip writer (regular files only).
pub struct TarGz<W: Write> {
    enc: flate2::write::GzEncoder<W>,
    mtime: u64,
}

fn octal(field: &mut [u8], v: u64) {
    let w = field.len() - 1;
    let s = format!("{v:0w$o}");
    let b = s.as_bytes();
    let b = &b[b.len().saturating_sub(w)..];
    field[..w].copy_from_slice(b);
    field[w] = 0;
}

/// A ustar header for a regular file (`name` up to 100 bytes, or split into prefix/name at a `/`).
pub fn tar_header(name: &str, size: u64, mtime: u64) -> Result<[u8; 512], String> {
    let mut h = [0u8; 512];
    let (prefix, base) = if name.len() <= 100 {
        ("", name)
    } else {
        let cut = name[..name.len().min(155)].rfind('/').ok_or_else(|| format!("name too long for tar: {name}"))?;
        (&name[..cut], &name[cut + 1..])
    };
    if base.len() > 100 || prefix.len() > 155 {
        return Err(format!("name too long for tar: {name}"));
    }
    h[..base.len()].copy_from_slice(base.as_bytes());
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], mtime);
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    Ok(h)
}

impl<W: Write> TarGz<W> {
    /// Start an archive.
    pub fn new(w: W, mtime: u64) -> TarGz<W> {
        TarGz {
            enc: flate2::write::GzEncoder::new(w, flate2::Compression::default()),
            mtime,
        }
    }

    /// Add a file.
    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), String> {
        let h = tar_header(name, data.len() as u64, self.mtime)?;
        let e = |e: std::io::Error| e.to_string();
        self.enc.write_all(&h).map_err(e)?;
        self.enc.write_all(data).map_err(e)?;
        let pad = (512 - data.len() % 512) % 512;
        self.enc.write_all(&vec![0u8; pad]).map_err(e)?;
        Ok(())
    }

    /// End the archive (two empty blocks) and flush.
    pub fn finish(mut self) -> Result<W, String> {
        self.enc.write_all(&[0u8; 1024]).map_err(|e| e.to_string())?;
        self.enc.finish().map_err(|e| e.to_string())
    }
}

/// The last `max` bytes of a file as text (cut at the first line break when shortened).
pub fn tail_text(path: &Path, max: u64) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(max);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut b = Vec::new();
    f.take(max).read_to_end(&mut b).ok()?;
    let mut s = String::from_utf8_lossy(&b).into_owned();
    if from > 0 {
        if let Some(i) = s.find('\n') {
            s = format!("[... {from} earlier bytes not included ...]\n{}", &s[i + 1..]);
        }
    }
    Some(s)
}

const README: &str = "omarchyvr problem report
========================

Made by the Pimax app (\"Report a problem\"). It stayed on your computer; share it by hand if you want help.

What is inside:
  summary.txt        app version, system (kernel, GPU driver, desktop), what was running, the desk preflight verdict
  doctor.txt/.json   the setup checks (omarchyvr-doctor)
  checklist.txt      the Setup page items
  preflight.txt      the desk mode preflight (docs/LESSONS.md rows)
  kernel-gpu.txt     this boot's GPU kernel events (Xid, display engine)
  app/               the app's own log
  session/           the end of the desk/VR session logs (/tmp/omarchyvr-session)
  jobs/              the output of the last actions the app ran
  crashes/           crash, hang and renderer reports of the app
  minidumps/         only if you ticked \"include crash dumps\": raw memory of the crashed window process

Redacted in every text file: your home directory (shown as ~), user and host names, the headset and eye chip
serial numbers, the machine id, e-mail addresses, IP and MAC addresses and file system UUIDs. Minidumps cannot be
redacted.
";

/// Build the report; returns its path.
pub fn build(i: &Input) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&i.out_dir).map_err(|e| format!("{}: {e}", i.out_dir.display()))?;
    let name = format!("omarchyvr-report-{}", i.stamp);
    let path = i.out_dir.join(format!("{name}.tar.gz"));
    let tmp = i.out_dir.join(format!("{name}.tar.gz.part"));
    let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mtime = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut t = TarGz::new(std::io::BufWriter::new(file), mtime);
    let r = &i.redactor;
    let add_text = |t: &mut TarGz<_>, rel: &str, text: &str| t.add(&format!("{name}/{rel}"), r.text(text).as_bytes());
    add_text(&mut t, "README.txt", README)?;
    add_text(&mut t, "summary.txt", &i.summary)?;
    if let Some((txt, json)) = &i.doctor {
        add_text(&mut t, "doctor.txt", txt)?;
        add_text(&mut t, "doctor.json", json)?;
    }
    add_text(&mut t, "checklist.txt", &i.checklist)?;
    add_text(&mut t, "preflight.txt", &i.preflight)?;
    add_text(&mut t, "kernel-gpu.txt", &i.kernel)?;
    let logs = i.state_dir.join("logs");
    for f in ["omarchyvr-app.log", "omarchyvr-app.log.1", "window-stderr.log"] {
        if let Some(s) = tail_text(&logs.join(f), LOG_TAIL) {
            add_text(&mut t, &format!("app/{f}"), &s)?;
        }
    }
    for p in &i.session_logs {
        if let (Some(s), Some(f)) = (tail_text(p, LOG_TAIL / 2), p.file_name()) {
            add_text(&mut t, &format!("session/{}", f.to_string_lossy()), &s)?;
        }
    }
    let mut jobs: Vec<PathBuf> = std::fs::read_dir(i.state_dir.join("jobs"))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect())
        .unwrap_or_default();
    jobs.sort();
    for p in jobs.iter().rev().take(5) {
        if let (Some(s), Some(f)) = (tail_text(p, LOG_TAIL / 4), p.file_name()) {
            add_text(&mut t, &format!("jobs/{}", f.to_string_lossy()), &s)?;
        }
    }
    for (dir, _, _) in crate::crash::list(&i.state_dir).into_iter().take(8) {
        let Some(dn) = dir.file_name().map(|d| d.to_string_lossy().into_owned()) else { continue };
        if let Some(s) = tail_text(&dir.join("report.txt"), LOG_TAIL) {
            add_text(&mut t, &format!("crashes/{dn}/report.txt"), &s)?;
        }
        if i.include_minidumps {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "dmp") {
                        if let Ok(b) = std::fs::read(&p) {
                            t.add(&format!("{name}/minidumps/{dn}-{}", e.file_name().to_string_lossy()), &b)?;
                        }
                    }
                }
            }
        }
    }
    let mut w = t.finish()?;
    w.flush().map_err(|e| e.to_string())?;
    drop(w);
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    prune(&i.out_dir, 10);
    Ok(path)
}

/// Keep the newest `keep` reports.
pub fn prune(dir: &Path, keep: usize) {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".tar.gz")).collect())
        .unwrap_or_default();
    v.sort();
    let n = v.len().saturating_sub(keep);
    for p in v.into_iter().take(n) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash::{Kind, Report};

    #[test]
    fn header_checksum_and_long_names() {
        let h = tar_header("dir/file.txt", 5, 0o17).unwrap();
        assert_eq!(&h[124..136], b"00000000005\0");
        assert_eq!(&h[257..263], b"ustar\0");
        let mut copy = h;
        copy[148..156].copy_from_slice(b"        ");
        let sum: u32 = copy.iter().map(|b| *b as u32).sum();
        assert_eq!(std::str::from_utf8(&h[148..154]).unwrap(), format!("{sum:06o}"));
        let long = format!("{}/{}", "d".repeat(120), "f".repeat(50));
        let h = tar_header(&long, 1, 0).unwrap();
        assert_eq!(&h[345..465], "d".repeat(120).as_bytes());
        assert!(tar_header(&"x".repeat(300), 1, 0).is_err());
    }

    #[test]
    fn builds_a_redacted_archive_that_tar_can_read() {
        let d = std::env::temp_dir().join(format!("pimax-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let state = d.join("state");
        std::fs::create_dir_all(state.join("logs")).unwrap();
        std::fs::create_dir_all(state.join("jobs")).unwrap();
        std::fs::write(state.join("logs/omarchyvr-app.log"), "line by alice in /home/alice/pimax-re\n").unwrap();
        std::fs::write(state.join("jobs/20261014-1-start.log"), "$ omarchyvr desk start\n").unwrap();
        let mut r = Report::new(Kind::Panic, "panic in thread 'main'", "x");
        r.log_tail = vec!["serial=SER123456".into()];
        let cd = crate::crash::write(&state, &r).unwrap();
        std::fs::write(cd.join("minidump.dmp"), [0u8, 1, 2]).unwrap();
        let session = d.join("monado.log");
        std::fs::write(&session, "x".repeat(10)).unwrap();
        let input = Input {
            state_dir: state.clone(),
            out_dir: state.join("reports"),
            stamp: "20261014T120000Z".into(),
            summary: "app 0.1.0 on archbox".into(),
            doctor: Some(("doctor text".into(), "{}".into())),
            checklist: "items".into(),
            preflight: "gate".into(),
            kernel: "none".into(),
            session_logs: vec![session],
            include_minidumps: false,
            redactor: Redactor::new("/home/alice", "alice", "archbox", "", &[]),
        };
        let p = build(&input).unwrap();
        assert!(p.ends_with("omarchyvr-report-20261014T120000Z.tar.gz"));
        let gz = std::fs::read(&p).unwrap();
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("line by <user> in ~/pimax-re"), "redacted log");
        assert!(text.contains("app 0.1.0 on <host>"));
        assert!(text.contains("serial=<serial>"));
        assert!(!text.contains("omarchyvr-report-20261014T120000Z/minidumps/"), "no dumps without opt-in");
        if std::process::Command::new("tar").arg("--version").output().is_ok() {
            let out = std::process::Command::new("tar").arg("-tzf").arg(&p).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let list = String::from_utf8_lossy(&out.stdout);
            for f in ["README.txt", "summary.txt", "doctor.json", "app/omarchyvr-app.log", "session/monado.log", "jobs/20261014-1-start.log"] {
                assert!(list.contains(&format!("omarchyvr-report-20261014T120000Z/{f}")), "{f} missing:\n{list}");
            }
            assert!(list.contains("/report.txt"));
        }
        let mut with = input.clone();
        with.include_minidumps = true;
        with.stamp = "20261014T120001Z".into();
        let p2 = build(&with).unwrap();
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&std::fs::read(&p2).unwrap()[..]).read_to_end(&mut raw).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("omarchyvr-report-20261014T120001Z/minidumps/"));
        let _ = std::fs::remove_dir_all(d);
    }
}
