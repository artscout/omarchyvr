//! PipeWire capture: a stream that asks for F32 mono 16 kHz (PipeWire's adapter converts from the device's
//! s16 stereo 48 kHz), linked to the selected `Audio/Source` node.
//!
//! Selection: exact `node.name`, else a case-insensitive substring of name / description / nick (default "Pimax"),
//! else the default source (with a warning). WirePlumber moves the stream if the node goes away.

use pipewire as pw;
use pw::spa;
use spa::pod::Pod;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// An `Audio/Source` node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub nick: String,
}

/// Pick the node for `want` (see the module docs). `None`: use the default source.
pub fn select<'a>(sources: &'a [SourceInfo], want: &str) -> Option<&'a SourceInfo> {
    if want.is_empty() || want.eq_ignore_ascii_case("default") {
        return None;
    }
    if let Some(s) = sources.iter().find(|s| s.name == want) {
        return Some(s);
    }
    let w = want.to_lowercase();
    sources.iter().find(|s| {
        s.name.to_lowercase().contains(&w)
            || s.description.to_lowercase().contains(&w)
            || s.nick.to_lowercase().contains(&w)
    })
}

fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(pw::init);
}

/// List the `Audio/Source` nodes (a registry round trip; needs a running PipeWire).
pub fn list_sources() -> Result<Vec<SourceInfo>, String> {
    init();
    let mainloop =
        pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("pipewire main loop: {e}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| format!("pipewire context: {e}"))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("cannot connect to PipeWire: {e}"))?;
    list_on(&mainloop, &core)
}

fn list_on(
    mainloop: &pw::main_loop::MainLoopRc,
    core: &pw::core::CoreRc,
) -> Result<Vec<SourceInfo>, String> {
    let registry = core.get_registry().map_err(|e| e.to_string())?;
    let found = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(std::cell::Cell::new(false));
    let pending = core.sync(0).map_err(|e| e.to_string())?;
    let (d2, l2) = (done.clone(), mainloop.clone());
    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                d2.set(true);
                l2.quit();
            }
        })
        .register();
    let f2 = found.clone();
    let _reg_listener = registry
        .add_listener_local()
        .global(move |g| {
            if g.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(props) = g.props else { return };
            if props.get("media.class") != Some("Audio/Source") {
                return;
            }
            let get = |k: &str| props.get(k).unwrap_or("").to_string();
            f2.borrow_mut().push(SourceInfo {
                id: g.id,
                name: get("node.name"),
                description: get("node.description"),
                nick: get("node.nick"),
            });
        })
        .register();
    while !done.get() {
        mainloop.run();
    }
    let v = found.borrow().clone();
    Ok(v)
}

/// What the capture thread reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureNote {
    /// Streaming from this source (description or name; "default source" when not selected by name).
    Started(String),
    /// The wanted device was not found; the default source is used.
    Fallback(String),
    Error(String),
}

/// Capture until `stop` is set: 16 kHz mono f32 chunks go to `tx`. Runs the PipeWire loop in its own thread.
pub fn spawn(
    want: String,
    tx: Sender<Vec<f32>>,
    notes: Sender<CaptureNote>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("voice-pipewire".into())
        .spawn(move || {
            if let Err(e) = run(&want, tx, &notes, &stop) {
                let _ = notes.send(CaptureNote::Error(e));
            }
        })
        .expect("spawn capture thread")
}

fn run(
    want: &str,
    tx: Sender<Vec<f32>>,
    notes: &Sender<CaptureNote>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    init();
    let mainloop =
        pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("pipewire main loop: {e}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| format!("pipewire context: {e}"))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("cannot connect to PipeWire: {e}"))?;
    let sources = list_on(&mainloop, &core)?;
    let chosen = select(&sources, want).cloned();
    let label = match &chosen {
        Some(s) => {
            if s.description.is_empty() {
                s.name.clone()
            } else {
                s.description.clone()
            }
        }
        None => {
            if !want.is_empty() && !want.eq_ignore_ascii_case("default") {
                let names: Vec<&str> = sources.iter().map(|s| s.name.as_str()).collect();
                let _ = notes.send(CaptureNote::Fallback(format!(
                    "no source matches {want:?} (have: {}); using the default source",
                    names.join(", ")
                )));
            }
            "default source".to_string()
        }
    };
    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::NODE_NAME => "pimax-voice",
        *pw::keys::APP_NAME => "pimax-voice",
    };
    if let Some(s) = &chosen {
        props.insert("target.object", s.name.as_str());
    }
    // ~20 ms quanta at 16 kHz
    props.insert("node.latency", "320/16000");
    let stream = pw::stream::StreamBox::new(&core, "pimax-voice capture", props)
        .map_err(|e| format!("pipewire stream: {e}"))?;
    let tx2 = tx.clone();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.is_empty() {
                return;
            }
            let d = &mut datas[0];
            let size = d.chunk().size() as usize;
            let offset = d.chunk().offset() as usize;
            if let Some(bytes) = d.data() {
                let end = (offset + size).min(bytes.len());
                let b = &bytes[offset.min(end)..end];
                let samples: Vec<f32> = b
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect();
                if !samples.is_empty() {
                    let _ = tx2.send(samples);
                }
            }
        })
        .register()
        .map_err(|e| format!("pipewire listener: {e}"))?;
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(16_000);
    info.set_channels(1);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let values: Vec<u8> = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )
    .map_err(|e| format!("format pod: {e:?}"))?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or("format pod")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .map_err(|e| format!("pipewire connect: {e}"))?;
    let _ = notes.send(CaptureNote::Started(label));
    let ml = mainloop.clone();
    let stop2 = Arc::clone(stop);
    let timer = mainloop.loop_().add_timer(move |_| {
        if stop2.load(Ordering::Relaxed) {
            ml.quit();
        }
    });
    let _ = timer.update_timer(
        Some(Duration::from_millis(100)),
        Some(Duration::from_millis(100)),
    );
    mainloop.run();
    let _ = stream.disconnect();
    drop(tx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: u32, name: &str, desc: &str) -> SourceInfo {
        SourceInfo {
            id,
            name: name.into(),
            description: desc.into(),
            nick: String::new(),
        }
    }

    #[test]
    fn selection_rules() {
        let s = vec![
            src(
                37,
                "alsa_input.pci-0000_00_1f.3.analog-stereo",
                "Built-in Audio Analog Stereo",
            ),
            src(
                58,
                "alsa_input.usb-Pimax_Corporation_Pimax_Dream_Air_P511-04.analog-stereo",
                "Pimax Dream Air Analog Stereo",
            ),
        ];
        assert_eq!(select(&s, "Pimax").map(|x| x.id), Some(58));
        assert_eq!(select(&s, "dream air").map(|x| x.id), Some(58));
        assert_eq!(
            select(&s, "alsa_input.pci-0000_00_1f.3.analog-stereo").map(|x| x.id),
            Some(37)
        );
        assert_eq!(select(&s, "built-in").map(|x| x.id), Some(37));
        assert_eq!(select(&s, "default"), None);
        assert_eq!(select(&s, ""), None);
        assert_eq!(select(&s, "webcam"), None);
        assert_eq!(select(&[], "Pimax"), None);
    }
}
