//! The socket subscriber: connects to the `omarchyvr-voice` Unix socket, reads JSON lines without ever blocking, and
//! reconnects every second when the server is gone. Driven by [`Client::poll`], once per frame.

use crate::protocol::{self, Event};
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Longest line accepted; a longer one (garbage, or a stuck writer) is skipped up to its newline.
pub const MAX_LINE: usize = 64 * 1024;
/// Most bytes read in one poll, so a flooding server cannot stall the frame loop.
const MAX_PER_POLL: usize = 256 * 1024;

/// Splits a byte stream into lines. Partial lines are kept until their newline arrives.
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
    /// Inside a line that already exceeded [`MAX_LINE`]: drop bytes up to the next newline.
    skipping: bool,
}

impl LineBuffer {
    pub fn new() -> LineBuffer {
        LineBuffer::default()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.skipping = false;
    }

    /// Bytes of an incomplete line that are waiting for their newline.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Feed bytes; every complete non-empty line (without `\n` / `\r\n`) is appended to `out` as lossy UTF-8.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        for &b in bytes {
            if b == b'\n' {
                if self.skipping {
                    self.skipping = false;
                } else {
                    if self.buf.last() == Some(&b'\r') {
                        self.buf.pop();
                    }
                    if !self.buf.is_empty() {
                        out.push(String::from_utf8_lossy(&self.buf).into_owned());
                    }
                }
                self.buf.clear();
            } else if !self.skipping {
                if self.buf.len() >= MAX_LINE {
                    self.skipping = true;
                    self.buf.clear();
                } else {
                    self.buf.push(b);
                }
            }
        }
    }
}

/// What a poll reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// A connection was established (a new server session: the server sends `hello` next).
    Connected,
    /// The connection was lost, or (once per outage) the first attempt failed. Retries continue silently.
    Down(String),
    Event(Event),
    /// A raw line as received, before parsing (only with [`Client::set_raw_tap`]); for `dump --raw`.
    Raw(String),
}

/// Non-blocking subscriber with automatic reconnection.
pub struct Client {
    path: PathBuf,
    retry: Duration,
    stream: Option<UnixStream>,
    lines: LineBuffer,
    next_attempt: Option<Instant>,
    down_reported: bool,
    raw_tap: bool,
}

impl Client {
    pub fn new(path: PathBuf, retry: Duration) -> Client {
        Client {
            path,
            retry,
            stream: None,
            lines: LineBuffer::new(),
            next_attempt: None,
            down_reported: false,
            raw_tap: false,
        }
    }

    /// Also report every raw line as [`Notice::Raw`] (before its parsed event, if it has one).
    pub fn set_raw_tap(&mut self, on: bool) {
        self.raw_tap = on;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn lose(&mut self, now: Instant, why: String, out: &mut Vec<Notice>) {
        self.stream = None;
        self.lines.clear();
        self.next_attempt = Some(now + self.retry);
        self.down_reported = true;
        out.push(Notice::Down(why));
    }

    /// Reconnect when due, read what is available, and append the notices to `out`. Never blocks.
    pub fn poll(&mut self, now: Instant, out: &mut Vec<Notice>) {
        if self.stream.is_none() {
            if self.next_attempt.is_some_and(|t| now < t) {
                return;
            }
            let attempt = UnixStream::connect(&self.path).and_then(|s| {
                s.set_nonblocking(true)?;
                Ok(s)
            });
            match attempt {
                Ok(s) => {
                    self.stream = Some(s);
                    self.lines.clear();
                    self.down_reported = false;
                    self.next_attempt = None;
                    out.push(Notice::Connected);
                }
                Err(e) => {
                    self.next_attempt = Some(now + self.retry);
                    if !self.down_reported {
                        self.down_reported = true;
                        out.push(Notice::Down(format!("{}: {e}", self.path.display())));
                    }
                    return;
                }
            }
        }

        let mut lines = Vec::new();
        let mut lost: Option<String> = None;
        let mut total = 0usize;
        let mut chunk = [0u8; 8192];
        if let Some(stream) = self.stream.as_mut() {
            while total < MAX_PER_POLL {
                match stream.read(&mut chunk) {
                    Ok(0) => {
                        lost = Some("connection closed by the server".to_string());
                        break;
                    }
                    Ok(n) => {
                        total += n;
                        self.lines.push(&chunk[..n], &mut lines);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        lost = Some(format!("read error: {e}"));
                        break;
                    }
                }
            }
        }
        // events read before the end of the stream are still delivered
        for line in &lines {
            if self.raw_tap {
                out.push(Notice::Raw(line.clone()));
            }
            if let Some(ev) = protocol::parse_line(line) {
                out.push(Notice::Event(ev));
            }
        }
        if let Some(why) = lost {
            self.lose(now, why, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::net::UnixListener;

    fn lines_of(chunks: &[&[u8]]) -> Vec<String> {
        let mut b = LineBuffer::new();
        let mut out = Vec::new();
        for c in chunks {
            b.push(c, &mut out);
        }
        out
    }

    #[test]
    fn splits_lines_across_chunks() {
        assert_eq!(lines_of(&[b"one\ntwo\n"]), ["one", "two"]);
        assert_eq!(lines_of(&[b"on", b"e\ntw", b"o\n"]), ["one", "two"]);
        assert_eq!(lines_of(&[b"one\ntwo"]), ["one"]); // "two" is incomplete
        assert_eq!(lines_of(&[b"a\r\nb\r\n"]), ["a", "b"]);
        assert_eq!(lines_of(&[b"\n\n  \nx\n"]), ["  ", "x"]);
        assert_eq!(lines_of(&[b""]), Vec::<String>::new());
    }

    #[test]
    fn multibyte_char_split_between_reads() {
        // "ñ" is C3 B1; split between the two bytes
        let lines = lines_of(&[b"a\xC3", b"\xB1b\n"]);
        assert_eq!(lines, ["añb"]);
        // invalid UTF-8 becomes replacement characters instead of an error
        let lines = lines_of(&[b"a\xFFb\n"]);
        assert_eq!(lines, ["a\u{FFFD}b"]);
    }

    #[test]
    fn splitting_does_not_depend_on_how_the_bytes_arrive() {
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            // random lines (with multi-byte characters, blanks, CRLF) joined by newlines
            let mut data: Vec<u8> = Vec::new();
            for _ in 0..(next() % 12) {
                let len = (next() % 40) as usize;
                for _ in 0..len {
                    let c = match next() % 6 {
                        0 => "ñ".to_string(),
                        1 => "¿".to_string(),
                        2 => " ".to_string(),
                        _ => char::from(b'a' + (next() % 26) as u8).to_string(),
                    };
                    data.extend_from_slice(c.as_bytes());
                }
                data.extend_from_slice(if next() % 3 == 0 { b"\r\n" } else { b"\n" });
            }
            let mut whole = Vec::new();
            LineBuffer::new().push(&data, &mut whole);
            // the same bytes in random pieces
            let mut b = LineBuffer::new();
            let mut pieces = Vec::new();
            let mut at = 0;
            while at < data.len() {
                let n = 1 + (next() as usize % 9);
                let end = (at + n).min(data.len());
                b.push(&data[at..end], &mut pieces);
                at = end;
            }
            assert_eq!(whole, pieces);
        }
    }

    #[test]
    fn overlong_line_is_skipped_and_the_next_one_survives() {
        let mut big = vec![b'x'; MAX_LINE + 10];
        big.push(b'\n');
        big.extend_from_slice(b"ok\n");
        let mut b = LineBuffer::new();
        let mut out = Vec::new();
        // fed in small pieces so the skipping state is carried over
        for c in big.chunks(1000) {
            b.push(c, &mut out);
        }
        assert_eq!(out, ["ok"]);
        assert_eq!(b.pending(), 0);
        // a line of exactly MAX_LINE bytes still passes
        let mut exact = vec![b'y'; MAX_LINE];
        exact.push(b'\n');
        let mut out = Vec::new();
        LineBuffer::new().push(&exact, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), MAX_LINE);
    }

    fn temp_socket(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omarchyvr-voicehud-test-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("v.sock")
    }

    /// Poll until `want` notices are collected (or two seconds pass).
    fn collect(c: &mut Client, now: &mut Instant, want: usize) -> Vec<Notice> {
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(2);
        while got.len() < want && Instant::now() < end {
            c.poll(*now, &mut got);
            std::thread::sleep(Duration::from_millis(5));
        }
        got
    }

    #[test]
    fn reads_events_and_ignores_garbage() {
        let path = temp_socket("read");
        let listener = UnixListener::bind(&path).unwrap();
        let mut c = Client::new(path.clone(), Duration::from_secs(1));
        let mut now = Instant::now();
        let mut got = Vec::new();
        c.poll(now, &mut got);
        assert_eq!(got, [Notice::Connected]);
        let (mut server, _) = listener.accept().unwrap();
        // a line split across two writes, garbage, an unknown type, then a valid one
        server.write_all(b"{\"v\":1,\"type\":\"speech_st").unwrap();
        server.flush().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        server
            .write_all(b"art\",\"seg\":3,\"t\":1.5}\nnot json\n{\"type\":\"pong\"}\n{\"type\":\"level\",\"db\":-40,\"speech\":true}\n")
            .unwrap();
        let got = collect(&mut c, &mut now, 2);
        assert_eq!(
            got,
            [
                Notice::Event(Event::SpeechStart { seg: 3, t: 1.5 }),
                Notice::Event(Event::Level {
                    db: -40.0,
                    speech: true
                })
            ]
        );
        assert!(c.is_connected());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn poll_without_server_does_not_block_and_retries_every_interval() {
        let path = temp_socket("down");
        let mut c = Client::new(path.clone(), Duration::from_secs(1));
        let t0 = Instant::now();
        let mut got = Vec::new();
        c.poll(t0, &mut got);
        // exactly one Down notice per outage, with the path in it
        assert_eq!(got.len(), 1);
        assert!(
            matches!(&got[0], Notice::Down(m) if m.contains("v.sock")),
            "{got:?}"
        );
        assert!(!c.is_connected());
        // inside the retry interval nothing happens even if the server appears
        let listener = UnixListener::bind(&path).unwrap();
        let mut got = Vec::new();
        c.poll(t0 + Duration::from_millis(500), &mut got);
        assert!(got.is_empty() && !c.is_connected());
        // after the interval it connects
        c.poll(t0 + Duration::from_millis(1001), &mut got);
        assert_eq!(got, [Notice::Connected]);
        drop(listener);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reconnects_after_the_server_restarts() {
        let path = temp_socket("restart");
        let listener = UnixListener::bind(&path).unwrap();
        let mut c = Client::new(path.clone(), Duration::from_secs(1));
        let mut now = Instant::now();
        let mut got = Vec::new();
        c.poll(now, &mut got);
        assert_eq!(got, [Notice::Connected]);
        let (mut server, _) = listener.accept().unwrap();
        server
            .write_all(b"{\"type\":\"speech_start\",\"seg\":1}\n")
            .unwrap();
        let got = collect(&mut c, &mut now, 1);
        assert_eq!(got, [Notice::Event(Event::SpeechStart { seg: 1, t: 0.0 })]);

        // the server goes away: the pending bytes of an unfinished line are discarded with the connection
        server.write_all(b"{\"type\":\"speech_st").unwrap();
        drop(server);
        drop(listener);
        std::fs::remove_file(&path).unwrap();
        let got = collect(&mut c, &mut now, 1);
        assert!(
            matches!(&got[..], [Notice::Down(m)] if m.contains("closed")),
            "{got:?}"
        );
        assert!(!c.is_connected());

        // still down 0.5 s later (no attempt), and the first attempt after 1 s fails silently (path missing)
        now += Duration::from_millis(500);
        let mut got = Vec::new();
        c.poll(now, &mut got);
        assert!(got.is_empty());
        now += Duration::from_millis(600);
        c.poll(now, &mut got);
        assert!(got.is_empty(), "outage already reported: {got:?}");

        // a new server on the same path: the next attempt (1 s later) connects, the stale partial line is gone
        let listener = UnixListener::bind(&path).unwrap();
        now += Duration::from_millis(1100);
        let mut got = Vec::new();
        c.poll(now, &mut got);
        assert_eq!(got, [Notice::Connected]);
        let (mut server, _) = listener.accept().unwrap();
        server
            .write_all(b"{\"type\":\"hello\",\"engine\":\"e\",\"source\":\"s\",\"rate\":16000}\n")
            .unwrap();
        let got = collect(&mut c, &mut now, 1);
        assert_eq!(
            got,
            [Notice::Event(Event::Hello {
                engine: "e".into(),
                source: "s".into(),
                rate: 16000
            })]
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_flooding_server_is_bounded_per_poll() {
        let path = temp_socket("flood");
        let listener = UnixListener::bind(&path).unwrap();
        let mut c = Client::new(path.clone(), Duration::from_secs(1));
        let now = Instant::now();
        let mut got = Vec::new();
        c.poll(now, &mut got);
        let (server, _) = listener.accept().unwrap();
        server.set_nonblocking(true).unwrap();
        let mut server = server;
        let line = b"{\"type\":\"level\",\"db\":-50,\"speech\":false}\n";
        let mut written = 0usize;
        // fill the socket buffer
        while written < 2 * 1024 * 1024 {
            match server.write(line) {
                Ok(n) => written += n,
                Err(_) => break,
            }
        }
        let t = Instant::now();
        let mut got = Vec::new();
        c.poll(now, &mut got);
        assert!(!got.is_empty());
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "{:?}",
            t.elapsed()
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
