//! Minimal `log` backend: `HH:MM:SS message` on stderr (INFO and up, DEBUG with PIMAX_LOG=debug).
use log::{Level, LevelFilter, Log, Metadata, Record};

struct L;

impl Log for L {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= log::max_level()
    }
    fn log(&self, r: &Record) {
        if self.enabled(r.metadata()) {
            let tag = if r.level() == Level::Info {
                String::new()
            } else {
                format!("{}: ", r.level().as_str().to_lowercase())
            };
            eprintln!("{} {tag}{}", pimax::sys::local_hms(), r.args());
        }
    }
    fn flush(&self) {}
}

/// Install the logger once.
pub fn init() {
    let lvl = match std::env::var("PIMAX_LOG").as_deref() {
        Ok("debug") => LevelFilter::Debug,
        Ok("trace") => LevelFilter::Trace,
        Ok("warn") => LevelFilter::Warn,
        Ok("off") => LevelFilter::Off,
        _ => LevelFilter::Info,
    };
    static LOGGER: L = L;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(lvl);
}
