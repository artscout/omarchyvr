//! Where events go: the Unix socket server (JSON lines to every subscriber), stdout, test collectors.

use crate::protocol::{self, Command, Event, State};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Receives every event (from the processing and the decoding thread).
pub trait Sink: Send + Sync {
    fn emit(&self, event: &Event);
}

/// Several sinks.
pub struct Tee(pub Vec<Arc<dyn Sink>>);

impl Sink for Tee {
    fn emit(&self, event: &Event) {
        for s in &self.0 {
            s.emit(event);
        }
    }
}

/// Keeps every event with its arrival time (tests, benchmarks).
#[derive(Default)]
pub struct Collect(pub Mutex<Vec<(Instant, Event)>>);

impl Collect {
    pub fn events(&self) -> Vec<Event> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }
}

impl Sink for Collect {
    fn emit(&self, event: &Event) {
        self.0.lock().unwrap().push((Instant::now(), event.clone()));
    }
}

/// Human readable (or JSON lines) on stdout. Partials are rewritten in place on a terminal.
pub struct Stdout {
    pub json: bool,
    pub levels: bool,
    tty: bool,
    partial_shown: Mutex<bool>,
}

impl Stdout {
    pub fn new(json: bool, levels: bool) -> Stdout {
        // SAFETY: isatty only inspects the descriptor.
        let tty = unsafe { libc::isatty(1) } == 1;
        Stdout {
            json,
            levels,
            tty,
            partial_shown: Mutex::new(false),
        }
    }

    /// The human readable line for an event (`None`: not shown).
    pub fn format(event: &Event, levels: bool) -> Option<String> {
        Some(match event {
            Event::Hello { engine, source, .. } => {
                format!("# engine: {engine}\n# source: {source}")
            }
            Event::Status { state, detail } => format!(
                "# {state:?}{}{detail}",
                if detail.is_empty() { "" } else { ": " }
            ),
            Event::Partial { text, .. } => format!("… {text}"),
            Event::Final { text, .. } if text.is_empty() => return None,
            Event::Final {
                text,
                lang,
                t0,
                latency_ms,
                decode_ms,
                ..
            } => format!(
                "[{t0:7.2}] {text}   ({}{latency_ms} ms after speech, decode {decode_ms} ms)",
                lang.as_deref()
                    .map(|l| format!("{l}, "))
                    .unwrap_or_default()
            ),
            Event::Intent { name, slots, .. } => {
                let s: Vec<String> = slots.iter().map(|(k, v)| format!("{k}={v:?}")).collect();
                format!("  -> intent {name} {}", s.join(" "))
            }
            Event::Level { db, speech } if levels => format!(
                "  level {db:6.1} dBFS{}",
                if *speech { "  speech" } else { "" }
            ),
            _ => return None,
        })
    }
}

impl Sink for Stdout {
    fn emit(&self, event: &Event) {
        let mut out = std::io::stdout().lock();
        if self.json {
            if !matches!(event, Event::Level { .. }) || self.levels {
                let _ = writeln!(out, "{}", protocol::encode(event));
            }
            let _ = out.flush();
            return;
        }
        let Some(line) = Stdout::format(event, self.levels) else {
            return;
        };
        let mut shown = self.partial_shown.lock().unwrap();
        let partial = matches!(event, Event::Partial { .. });
        if self.tty {
            if *shown {
                let _ = write!(out, "\r\x1b[K");
            }
            if partial {
                let _ = write!(out, "{line}");
            } else {
                let _ = writeln!(out, "{line}");
            }
            *shown = partial;
        } else {
            let _ = writeln!(out, "{line}");
        }
        let _ = out.flush();
    }
}

struct Client {
    stream: UnixStream,
}

struct Shared {
    clients: Mutex<Vec<Client>>,
    hello: Mutex<Event>,
    status: Mutex<Event>,
    stop: AtomicBool,
}

/// The socket server: every event is written to every connected client; a client that does not keep up (write
/// timeout 200 ms) is dropped. New clients get `hello` and the last `status` first. Client lines are parsed as
/// [`Command`]s; `ping` is answered directly, the others go to `commands`.
pub struct Server {
    path: PathBuf,
    shared: Arc<Shared>,
    accept: Option<std::thread::JoinHandle<()>>,
}

fn send_line(stream: &mut UnixStream, event: &Event) -> bool {
    let mut line = protocol::encode(event);
    line.push('\n');
    stream.write_all(line.as_bytes()).is_ok()
}

impl Server {
    /// Bind `path` (a stale socket file is replaced; a live one is an error: another listener runs).
    pub fn bind(path: &Path, hello: Event, commands: Sender<Command>) -> Result<Server, String> {
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                return Err(format!(
                    "{} is in use: is another pimax-voice listening?",
                    path.display()
                ));
            }
            std::fs::remove_file(path)
                .map_err(|e| format!("remove stale {}: {e}", path.display()))?;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let listener =
            UnixListener::bind(path).map_err(|e| format!("bind {}: {e}", path.display()))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let shared = Arc::new(Shared {
            clients: Mutex::new(Vec::new()),
            hello: Mutex::new(hello),
            status: Mutex::new(Event::Status {
                state: State::Listening,
                detail: String::new(),
            }),
            stop: AtomicBool::new(false),
        });
        let sh = Arc::clone(&shared);
        let accept = std::thread::Builder::new()
            .name("voice-accept".into())
            .spawn(move || accept_loop(&listener, &sh, &commands))
            .map_err(|e| e.to_string())?;
        Ok(Server {
            path: path.to_path_buf(),
            shared,
            accept: Some(accept),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of connected clients.
    pub fn clients(&self) -> usize {
        self.shared.clients.lock().unwrap().len()
    }

    /// Replace the `hello` event sent to new clients.
    pub fn set_hello(&self, hello: Event) {
        *self.shared.hello.lock().unwrap() = hello;
    }
}

fn accept_loop(listener: &UnixListener, sh: &Arc<Shared>, commands: &Sender<Command>) {
    while !sh.stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                // read first: a one-shot client (`pimax-voice pause`) may already be gone when we greet it
                if let Ok(reader) = stream.try_clone() {
                    let commands = commands.clone();
                    let _ = std::thread::Builder::new()
                        .name("voice-client".into())
                        .spawn(move || client_reader(reader, &commands));
                }
                let hello = sh.hello.lock().unwrap().clone();
                let status = sh.status.lock().unwrap().clone();
                if send_line(&mut stream, &hello) && send_line(&mut stream, &status) {
                    sh.clients.lock().unwrap().push(Client { stream });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

fn client_reader(stream: UnixStream, commands: &Sender<Command>) {
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        match protocol::decode_command(&line) {
            Some(Command::Ping) => {
                if send_line(&mut writer, &Event::Pong) {
                    continue;
                }
                return;
            }
            Some(c) => {
                if commands.send(c).is_ok() {
                    continue;
                }
                return;
            }
            None => {}
        }
    }
}

impl Sink for Server {
    fn emit(&self, event: &Event) {
        if matches!(event, Event::Status { .. }) {
            *self.shared.status.lock().unwrap() = event.clone();
        }
        let mut line = protocol::encode(event);
        line.push('\n');
        let mut clients = self.shared.clients.lock().unwrap();
        clients.retain_mut(|c| c.stream.write_all(line.as_bytes()).is_ok());
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
        for c in self.shared.clients.lock().unwrap().drain(..) {
            let _ = c.stream.shutdown(std::net::Shutdown::Both);
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Connect to a listener and call `f` for every event until it returns false or the server goes away.
pub fn subscribe(path: &Path, mut f: impl FnMut(Event) -> bool) -> Result<(), String> {
    let stream =
        UnixStream::connect(path).map_err(|e| format!("connect {}: {e}", path.display()))?;
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if let Some(ev) = protocol::decode(&line) {
            if !f(ev) {
                break;
            }
        }
    }
    Ok(())
}

/// Send one command line to a listener.
pub fn send_command(path: &Path, cmd: Command) -> Result<(), String> {
    let mut stream =
        UnixStream::connect(path).map_err(|e| format!("connect {}: {e}", path.display()))?;
    let mut line = protocol::encode_command(cmd);
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn tmp_sock(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pimax-voice-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("s.sock")
    }

    fn hello() -> Event {
        Event::Hello {
            engine: "test".into(),
            source: "wav".into(),
            rate: 16000,
        }
    }

    fn read_line(r: &mut BufReader<UnixStream>) -> Event {
        let mut s = String::new();
        r.read_line(&mut s).unwrap();
        protocol::decode(&s).unwrap_or_else(|| panic!("bad line {s:?}"))
    }

    #[test]
    fn clients_get_hello_status_events_and_commands_flow_back() {
        let path = tmp_sock("flow");
        let (tx, rx) = mpsc::channel();
        let server = Server::bind(&path, hello(), tx).unwrap();
        server.emit(&Event::Status {
            state: State::Paused,
            detail: String::new(),
        });
        let stream = UnixStream::connect(&path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut w = stream.try_clone().unwrap();
        let mut r = BufReader::new(stream);
        assert_eq!(read_line(&mut r), hello());
        assert_eq!(
            read_line(&mut r),
            Event::Status {
                state: State::Paused,
                detail: String::new()
            }
        );
        // wait until the server registered the client, then broadcast
        let t0 = Instant::now();
        while server.clients() == 0 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let fin = Event::Final {
            seg: 1,
            text: "hola".into(),
            lang: Some("es".into()),
            t0: 0.0,
            t1: 1.0,
            latency_ms: 5,
            decode_ms: 1,
        };
        server.emit(&fin);
        assert_eq!(read_line(&mut r), fin);
        // commands
        w.write_all(b"{\"cmd\":\"pause\"}\ngarbage\n{\"cmd\":\"ping\"}\n")
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::Pause
        );
        assert_eq!(read_line(&mut r), Event::Pong);
        send_command(&path, Command::Resume).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::Resume
        );
        // a second server on the same live path is refused
        let (tx2, _rx2) = mpsc::channel();
        assert!(Server::bind(&path, hello(), tx2)
            .err()
            .unwrap()
            .contains("in use"));
        drop(server);
        assert!(!path.exists(), "socket file removed on drop");
    }

    #[test]
    fn stale_socket_file_is_replaced_and_dead_clients_are_dropped() {
        let path = tmp_sock("stale");
        {
            let _l = UnixListener::bind(&path).unwrap();
        } // listener gone, file stays
        assert!(path.exists());
        let (tx, _rx) = mpsc::channel();
        let server = Server::bind(&path, hello(), tx).unwrap();
        let c = UnixStream::connect(&path).unwrap();
        let t0 = Instant::now();
        while server.clients() == 0 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(c);
        for _ in 0..3 {
            server.emit(&Event::Level {
                db: -50.0,
                speech: false,
            });
        }
        assert_eq!(server.clients(), 0);
    }

    #[test]
    fn subscribe_reads_until_the_callback_stops() {
        let path = tmp_sock("sub");
        let (tx, _rx) = mpsc::channel();
        let server = Server::bind(&path, hello(), tx).unwrap();
        let p2 = path.clone();
        let h = std::thread::spawn(move || {
            let mut got = Vec::new();
            subscribe(&p2, |e| {
                got.push(e);
                got.len() < 3
            })
            .unwrap();
            got
        });
        let t0 = Instant::now();
        while server.clients() == 0 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        server.emit(&Event::SpeechStart { seg: 4, t: 1.0 });
        let got = h.join().unwrap();
        assert_eq!(got[2], Event::SpeechStart { seg: 4, t: 1.0 });
    }

    #[test]
    fn human_format() {
        let f = |e: &Event| Stdout::format(e, false);
        assert_eq!(
            f(&Event::Partial {
                seg: 1,
                text: "open".into(),
                t: 1.0
            })
            .unwrap(),
            "… open"
        );
        let fin = Event::Final {
            seg: 1,
            text: "Open.".into(),
            lang: Some("en".into()),
            t0: 1.5,
            t1: 2.0,
            latency_ms: 500,
            decode_ms: 60,
        };
        assert_eq!(
            f(&fin).unwrap(),
            "[   1.50] Open.   (en, 500 ms after speech, decode 60 ms)"
        );
        let empty = Event::Final {
            seg: 1,
            text: String::new(),
            lang: None,
            t0: 0.0,
            t1: 0.0,
            latency_ms: 0,
            decode_ms: 0,
        };
        assert_eq!(f(&empty), None);
        assert_eq!(
            f(&Event::Level {
                db: -40.0,
                speech: false
            }),
            None
        );
        assert!(Stdout::format(
            &Event::Level {
                db: -40.0,
                speech: true
            },
            true
        )
        .unwrap()
        .contains("speech"));
    }
}
