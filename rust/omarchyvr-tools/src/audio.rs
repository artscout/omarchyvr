//! `keepalived --follow ... --audio`: while the VR runtime runs, the headset's USB audio is the default output and
//! input (PipeWire, through `pactl`); when it is gone, the previous defaults come back, unless the user picked
//! another device in the meantime. The headset's nodes are found by name prefix (no serial in here).
use std::process::Command;

/// Name prefix of the headset's output node (the Crystal Super's USB audio, docs/CRYSTAL-SUPER.md step 7).
pub const SINK_PREFIX: &str = "alsa_output.usb-Pimax_Corporation_";
/// Name prefix of the headset's input node.
pub const SOURCE_PREFIX: &str = "alsa_input.usb-Pimax_Corporation_";

/// The first node of `pactl list short sinks|sources` whose name starts with `prefix` (monitors left out).
pub fn pick(list: &str, prefix: &str) -> Option<String> {
    list.lines()
        .filter_map(|l| l.split('\t').nth(1))
        .find(|n| n.starts_with(prefix) && !n.ends_with(".monitor"))
        .map(str::to_string)
}

fn pactl(args: &[&str]) -> Option<String> {
    let o = Command::new("pactl").args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// What was the default before the headset took over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Before {
    pub sink: Option<String>,
    pub source: Option<String>,
    /// What the headset was set to (restored only while these are still the defaults).
    pub our_sink: Option<String>,
    pub our_source: Option<String>,
}

/// Make the headset the default output and input; a log line and what to restore later.
pub fn to_headset() -> (String, Before) {
    let mut b = Before {
        sink: pactl(&["get-default-sink"]),
        source: pactl(&["get-default-source"]),
        ..Before::default()
    };
    let sink = pactl(&["list", "short", "sinks"]).and_then(|l| pick(&l, SINK_PREFIX));
    let source = pactl(&["list", "short", "sources"]).and_then(|l| pick(&l, SOURCE_PREFIX));
    let mut done = Vec::new();
    if let Some(s) = &sink {
        if pactl(&["set-default-sink", s]).is_some() {
            done.push("output");
            b.our_sink = sink.clone();
        }
    }
    if let Some(s) = &source {
        if pactl(&["set-default-source", s]).is_some() {
            done.push("input");
            b.our_source = source.clone();
        }
    }
    let msg = if done.is_empty() {
        "audio: no headset output/input found (pactl list short sinks)".to_string()
    } else {
        format!("audio: headset is the default {}", done.join(" and "))
    };
    (msg, b)
}

/// Give the defaults back (each only if the headset is still the default, i.e. the user did not change it).
pub fn restore(b: &Before) -> String {
    let mut done = Vec::new();
    let cur_sink = pactl(&["get-default-sink"]);
    if let (Some(prev), Some(ours)) = (&b.sink, &b.our_sink) {
        if cur_sink.as_ref() == Some(ours) && prev != ours && pactl(&["set-default-sink", prev]).is_some() {
            done.push(format!("output {prev}"));
        }
    }
    let cur_source = pactl(&["get-default-source"]);
    if let (Some(prev), Some(ours)) = (&b.source, &b.our_source) {
        if cur_source.as_ref() == Some(ours) && prev != ours && pactl(&["set-default-source", prev]).is_some() {
            done.push(format!("input {prev}"));
        }
    }
    if done.is_empty() {
        "audio: nothing to give back".into()
    } else {
        format!("audio: back to {}", done.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_headset_nodes_by_prefix() {
        let sinks = "59\talsa_output.usb-Pimax_Corporation_Pimax_Crystal_Super_X-04.pro-output-1\tPipeWire\ts16le\n\
                     72\talsa_output.usb-Logitech_PRO_X_2-00.analog-stereo\tPipeWire\ts16le\n";
        assert_eq!(
            pick(sinks, SINK_PREFIX).as_deref(),
            Some("alsa_output.usb-Pimax_Corporation_Pimax_Crystal_Super_X-04.pro-output-1")
        );
        let sources = "1\talsa_output.usb-Pimax_Corporation_A.pro-output-1.monitor\tPipeWire\n\
                       60\talsa_input.usb-Pimax_Corporation_A-04.pro-input-0\tPipeWire\n";
        assert_eq!(pick(sources, SOURCE_PREFIX).as_deref(), Some("alsa_input.usb-Pimax_Corporation_A-04.pro-input-0"));
        assert_eq!(pick("72\talsa_output.usb-Logitech-00\tPipeWire\n", SINK_PREFIX), None);
    }
}
