//! Reading the monado-service log: the lines `vr start` waits for and the numbers `vr status` reports.
//!
//! Monado prints ` INFO [function] text` / `DEBUG [function] text` / ` WARN ...` / `ERROR ...` and indents the
//! continuation lines of one message with a tab. Only the handful of messages that matter here are recognised; the
//! texts come from the real logs (`analysis/overnight/polish.md` lists the sources).
use std::collections::BTreeMap;

/// One recognised log message.
#[derive(Debug, Clone, PartialEq)]
pub enum LogEvent {
    /// `gyro bias done: gx gy gz dps over N samples (R restarts); accel ...` (pimax_hmd.c): head tracking is ready.
    GyroBiasDone {
        /// Mean gyro in deg/s.
        gyro_dps: [f32; 3],
        /// Samples averaged.
        samples: u32,
        /// How often the averaging window restarted because the headset moved.
        restarts: u32,
    },
    /// `headset not still for 3000 ms (13 restarts): tracking with the cached gyro bias gx gy gz dps; ...`
    /// (pimax_hmd.c, PIMAX_BIAS_TIMEOUT_MS): started while worn, head tracking is ready with a stored bias.
    GyroBiasFallback {
        /// Bias in deg/s that tracking started with.
        gyro_dps: [f32; 3],
        /// False when no cache existed and the bias is zero.
        cached: bool,
        /// How often the averaging window restarted before giving up.
        restarts: u32,
    },
    /// `gyro bias provisional: gx gy gz dps (...: not a real rest, not cached); put the headset still ...`
    /// (pimax_hmd.c, no bias cached yet): a quiet-enough window is used until the headset really rests.
    GyroBiasProvisional {
        /// Bias in deg/s in use now.
        gyro_dps: [f32; 3],
    },
    /// `gyro bias refined at rest: gx gy gz dps (moved ...)` (pimax_hmd.c): a real rest later in the session
    /// (measured and cached).
    GyroBiasRefined {
        /// Bias in deg/s in use now.
        gyro_dps: [f32; 3],
    },
    /// `pimax_video_prepare(90) -> 0`: the rate announced to the headset.
    VideoPrepare {
        /// Announced frames per second.
        fps: u32,
        /// Return code (0 = ok).
        rc: i32,
    },
    /// `found display mode 7104x3840@90.00` (debug level of the compositor).
    DisplayMode {
        /// Width in pixels.
        w: u32,
        /// Height in pixels.
        h: u32,
        /// Refresh rate in Hz.
        hz: f64,
    },
    /// `estimated_frame_period: 11.111111ms` printed when the compositor creates its pacer.
    FramePeriod {
        /// Milliseconds.
        ms: f64,
    },
    /// ` INFO [client_connected] Client 1 connected`.
    ClientConnected(u32),
    /// `application_name: 'xr_eyetest'` of the client info block.
    ClientApp(String),
    /// ` INFO [client_disconnected] Client 1 disconnected`.
    ClientDisconnected(u32),
    /// ` INFO [ipc_server_main_common] Server exiting`.
    ServerExiting,
    /// `dumped /path/frame100_7104x3840_fmt64.raw` (XRT_COMPOSITOR_DUMP_DIR).
    Dumped(String),
    /// Compositor pacer warning `Frame 123 missed by 3.40!` (only the presentation-feedback pacer prints it).
    MissedFrame {
        /// Frame id.
        frame: u64,
        /// Milliseconds late.
        ms: f64,
    },
    /// App pacing debug line `Delivered frame 0.30ms early.` (U_PACING_APP_LOG=debug).
    AppDelivered {
        /// Milliseconds the GPU work finished away from the prediction.
        ms: f64,
        /// True if later than predicted.
        late: bool,
    },
    /// App pacing debug line `period: 22.22` that follows `Delivered frame`.
    AppPeriod {
        /// Milliseconds the app was given for this frame.
        ms: f64,
    },
    /// An error line (`ERROR [...]`).
    Error(String),
    /// A warning line.
    Warning(String),
}

fn num<T: std::str::FromStr>(s: &str) -> Option<T> {
    s.trim_matches(|c: char| !(c.is_ascii_digit() || c == '-' || c == '.' || c == '+' || c == 'e'))
        .parse()
        .ok()
}

/// Text after `key`, if the line contains it.
fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.find(key).map(|i| &line[i + key.len()..])
}

/// Warnings that appear on every start (or with every first client) and mean nothing.
const BENIGN_WARNINGS: [&str; 3] = [
    "Could not raise priority for thread",
    "Duplicate extension",
    "Session is visible but not active",
];

/// Recognise one log line (without the trailing newline).
pub fn parse_line(line: &str) -> Option<LogEvent> {
    let t = line.trim();
    if let Some(rest) = after(t, "gyro bias done:") {
        // "0.267 0.687 0.453 dps over 2000 samples (0 restarts); accel ..."
        let mut it = rest.split_whitespace();
        let g = [
            num::<f32>(it.next()?)?,
            num::<f32>(it.next()?)?,
            num::<f32>(it.next()?)?,
        ];
        let samples = num::<u32>(after(rest, "over ")?.split_whitespace().next()?)?;
        let restarts = num::<u32>(after(rest, "(")?.split_whitespace().next()?).unwrap_or(0);
        return Some(LogEvent::GyroBiasDone {
            gyro_dps: g,
            samples,
            restarts,
        });
    }
    for (key, provisional) in [("gyro bias provisional:", true), ("gyro bias refined at rest:", false)] {
        if let Some(rest) = after(t, key) {
            let mut it = rest.split_whitespace();
            let g = [num::<f32>(it.next()?)?, num::<f32>(it.next()?)?, num::<f32>(it.next()?)?];
            return Some(if provisional { LogEvent::GyroBiasProvisional { gyro_dps: g } } else { LogEvent::GyroBiasRefined { gyro_dps: g } });
        }
    }
    if let Some(rest) = after(t, "headset not still for ") {
        // "3000 ms (13 restarts): tracking with the cached gyro bias 0.087 0.684 0.474 dps; ..."
        let restarts = num::<u32>(after(rest, "(")?.split_whitespace().next()?).unwrap_or(0);
        let cached = !rest.contains("zero (no cache)");
        let mut it = after(rest, "gyro bias ")?.split_whitespace();
        let g = [
            num::<f32>(it.next()?)?,
            num::<f32>(it.next()?)?,
            num::<f32>(it.next()?)?,
        ];
        return Some(LogEvent::GyroBiasFallback {
            gyro_dps: g,
            cached,
            restarts,
        });
    }
    if let Some(rest) = after(t, "pimax_video_prepare(") {
        let (fps, tail) = rest.split_once(')')?;
        let rc = num::<i32>(after(tail, "->")?)?;
        return Some(LogEvent::VideoPrepare { fps: num(fps)?, rc });
    }
    if let Some(rest) = after(t, "found display mode ") {
        let (res, hz) = rest.split_once('@')?;
        let (w, h) = res.split_once('x')?;
        return Some(LogEvent::DisplayMode {
            w: num(w)?,
            h: num(h)?,
            hz: num(hz)?,
        });
    }
    if let Some(rest) = t.strip_prefix("estimated_frame_period:") {
        return Some(LogEvent::FramePeriod { ms: num(rest)? });
    }
    if let Some(rest) = after(t, "Client ") {
        if t.contains("[client_connected]") {
            return Some(LogEvent::ClientConnected(num(rest
                .split_whitespace()
                .next()?)?));
        }
        if t.contains("[client_disconnected]") {
            return Some(LogEvent::ClientDisconnected(num(rest
                .split_whitespace()
                .next()?)?));
        }
    }
    if let Some(rest) = t.strip_prefix("application_name:") {
        return Some(LogEvent::ClientApp(
            rest.trim().trim_matches('\'').to_string(),
        ));
    }
    if t.contains("[ipc_server_main_common] Server exiting") {
        return Some(LogEvent::ServerExiting);
    }
    if let Some(rest) = after(t, "] dumped ") {
        return Some(LogEvent::Dumped(rest.trim().to_string()));
    }
    if let Some(rest) = after(t, "Frame ") {
        if let Some((id, tail)) = rest.split_once(" missed by ") {
            return Some(LogEvent::MissedFrame {
                frame: num(id)?,
                ms: num(tail)?,
            });
        }
    }
    if let Some(rest) = after(t, "Delivered frame ") {
        let (v, tail) = rest.split_once("ms ")?;
        return Some(LogEvent::AppDelivered {
            ms: num(v)?,
            late: tail.starts_with("late"),
        });
    }
    if let Some(rest) = t.strip_prefix("period:") {
        return Some(LogEvent::AppPeriod { ms: num(rest)? });
    }
    if t.starts_with("ERROR ") || t.starts_with("ERROR[") {
        return Some(LogEvent::Error(t.to_string()));
    }
    if t.starts_with("WARN ") && !BENIGN_WARNINGS.iter().any(|b| t.contains(b)) {
        return Some(LogEvent::Warning(t.to_string()));
    }
    None
}

/// Splits an arbitrary byte stream into lines (a line is complete once its newline arrived). The bytes are decoded per
/// complete line, so a multi-byte character that arrives in two chunks is not damaged.
#[derive(Debug, Default)]
pub struct LineBuffer {
    partial: Vec<u8>,
}

impl LineBuffer {
    /// Feed bytes; returns the completed lines (lossy UTF-8, without `\n` / `\r\n`).
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<String> {
        self.partial.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.partial.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=i).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]);
            out.push(text.trim_end_matches('\r').to_string());
        }
        out
    }

    /// Feed text; returns the completed lines.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.push_bytes(text.as_bytes())
    }
}

/// What the log says about the session so far.
#[derive(Debug, Default, Clone)]
pub struct Summary {
    /// Set once the gyro calibration finished.
    pub bias: Option<LogEvent>,
    /// Rate announced to the headset.
    pub video_prepare: Option<(u32, i32)>,
    /// Display mode the compositor chose (needs `XRT_COMPOSITOR_LOG=debug`).
    pub display_mode: Option<(u32, u32, f64)>,
    /// Frame interval of the compositor pacer in ms.
    pub frame_period_ms: Option<f64>,
    /// Client ids that connected.
    pub clients_connected: Vec<u32>,
    /// Number of disconnects.
    pub clients_disconnected: u32,
    /// Application names reported by clients.
    pub apps: Vec<String>,
    /// The service printed its exit line.
    pub server_exited: bool,
    /// Paths of frame dumps.
    pub dumps: Vec<String>,
    /// Compositor frames reported as missed.
    pub missed: Vec<(u64, f64)>,
    /// App pacing: frames delivered (needs `U_PACING_APP_LOG=debug`).
    pub app_frames: u32,
    /// App pacing: frames whose GPU work ended more than 2 ms after the prediction.
    pub app_late_gt2ms: u32,
    /// App pacing: how many frames were given each period (key = period in 0.01 ms).
    pub app_periods: BTreeMap<u32, u32>,
    /// Error lines.
    pub errors: Vec<String>,
    /// Warning lines (the benign start-up ones excluded).
    pub warnings: Vec<String>,
}

impl Summary {
    /// Account one line.
    pub fn feed(&mut self, line: &str) {
        let Some(ev) = parse_line(line) else { return };
        match ev {
            LogEvent::GyroBiasDone { .. }
            | LogEvent::GyroBiasFallback { .. }
            | LogEvent::GyroBiasProvisional { .. }
            | LogEvent::GyroBiasRefined { .. } => self.bias = Some(ev),
            LogEvent::VideoPrepare { fps, rc } => self.video_prepare = Some((fps, rc)),
            LogEvent::DisplayMode { w, h, hz } => self.display_mode = Some((w, h, hz)),
            LogEvent::FramePeriod { ms } => self.frame_period_ms = Some(ms),
            LogEvent::ClientConnected(id) => self.clients_connected.push(id),
            LogEvent::ClientDisconnected(_) => self.clients_disconnected += 1,
            LogEvent::ClientApp(n) => self.apps.push(n),
            LogEvent::ServerExiting => self.server_exited = true,
            LogEvent::Dumped(p) => self.dumps.push(p),
            LogEvent::MissedFrame { frame, ms } => self.missed.push((frame, ms)),
            LogEvent::AppDelivered { ms, late } => {
                self.app_frames += 1;
                if late && ms > 2.0 {
                    self.app_late_gt2ms += 1;
                }
            }
            LogEvent::AppPeriod { ms } => {
                *self
                    .app_periods
                    .entry((ms * 100.0).round() as u32)
                    .or_default() += 1;
            }
            LogEvent::Error(l) => self.errors.push(l),
            LogEvent::Warning(l) => self.warnings.push(l),
        }
    }

    /// Summary of a whole log text.
    pub fn from_text(text: &str) -> Self {
        let mut s = Summary::default();
        for l in text.lines() {
            s.feed(l);
        }
        s
    }

    /// Summary of a log read line by line (constant memory: a `--debug` log grows by about 150 MB per hour; invalid
    /// UTF-8 does not stop the reading).
    pub fn from_reader<R: std::io::BufRead>(r: R) -> Self {
        let mut s = Summary::default();
        for line in r.split(b'\n').map_while(Result::ok) {
            s.feed(&String::from_utf8_lossy(&line));
        }
        s
    }

    /// True once `gyro bias done` was seen.
    pub fn ready(&self) -> bool {
        self.bias.is_some()
    }

    /// Frame interval implied by the announced rate (ms).
    pub fn nominal_period_ms(&self) -> Option<f64> {
        self.frame_period_ms
            .or_else(|| self.video_prepare.map(|(fps, _)| 1000.0 / f64::from(fps)))
    }

    /// Contradictions between what was asked (`want_hz`) and what the log says the headset and the compositor do.
    /// A compositor that scans out at another rate than the headset was told gives dark panels (the firmware waits
    /// for the announced rate), so these deserve a loud warning.
    pub fn rate_warnings(&self, want_hz: u32) -> Vec<String> {
        let want = f64::from(want_hz);
        let mut v = Vec::new();
        match self.video_prepare {
            None => v.push(
                "the log has no pimax_video_prepare line: the headset was not told a refresh rate"
                    .into(),
            ),
            Some((fps, rc)) => {
                if rc != 0 {
                    v.push(format!(
                        "pimax_video_prepare({fps}) failed with {rc}: the panels will stay dark"
                    ));
                }
                if fps != want_hz {
                    v.push(format!(
                        "the headset was told {fps} Hz but {want_hz} Hz was requested"
                    ));
                }
            }
        }
        if let Some(ms) = self.frame_period_ms {
            let hz = 1000.0 / ms;
            if (hz - want).abs() > 0.5 {
                v.push(format!(
                    "the compositor frame interval is {ms:.3} ms ({hz:.2} Hz), not {want_hz} Hz: check the display mode \
                     index (--mode N, XRT_COMPOSITOR_PRINT_MODES=1)"
                ));
            }
        }
        if let Some((w, h, hz)) = self.display_mode {
            if (hz - want).abs() > 0.5 {
                v.push(format!(
                    "the compositor picked the display mode {w}x{h} @ {hz:.2} Hz, not {want_hz} Hz: the headset will \
                     not light up"
                ));
            }
        }
        v
    }

    /// Human readable lines for `vr status` and the test reports.
    pub fn report(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some((fps, rc)) = self.video_prepare {
            v.push(format!(
                "headset video prepare: {fps} Hz announced (rc {rc}{})",
                if rc == 0 { "" } else { ", FAILED" }
            ));
        }
        if let Some((w, h, hz)) = self.display_mode {
            v.push(format!("compositor display mode: {w}x{h} @ {hz:.2} Hz"));
        }
        if let Some(ms) = self.frame_period_ms {
            v.push(format!(
                "compositor frame interval: {ms:.3} ms ({:.2} Hz)",
                1000.0 / ms
            ));
        }
        match &self.bias {
            Some(LogEvent::GyroBiasDone {
                gyro_dps,
                samples,
                restarts,
            }) => v.push(format!(
                "gyro calibration done: {:.3} {:.3} {:.3} dps over {samples} samples ({restarts} restarts)",
                gyro_dps[0], gyro_dps[1], gyro_dps[2]
            )),
            // the launcher's default since 2026-10-17 (PIMAX_BIAS_TIMEOUT_MS=200): start at once with the saved bias and
            // refine it in the background the next time the headset rests
            Some(LogEvent::GyroBiasFallback {
                gyro_dps,
                cached,
                restarts,
            }) => v.push(format!(
                "gyro: started with the {} bias {:.3} {:.3} {:.3} dps without waiting for a still headset ({restarts} restarts); it is refined when the headset rests",
                if *cached { "saved" } else { "zero (none saved)" },
                gyro_dps[0], gyro_dps[1], gyro_dps[2]
            )),
            Some(LogEvent::GyroBiasProvisional { gyro_dps }) => v.push(format!(
                "gyro: provisional bias {:.3} {:.3} {:.3} dps (no real rest yet: put the headset still on the desk for 2 s to calibrate the gyro)",
                gyro_dps[0], gyro_dps[1], gyro_dps[2]
            )),
            Some(LogEvent::GyroBiasRefined { gyro_dps }) => v.push(format!(
                "gyro calibration refined at rest: {:.3} {:.3} {:.3} dps",
                gyro_dps[0], gyro_dps[1], gyro_dps[2]
            )),
            _ => v.push("gyro calibration: not finished".into()),
        }
        if !self.clients_connected.is_empty() {
            v.push(format!(
                "clients: {} connected, {} disconnected{}",
                self.clients_connected.len(),
                self.clients_disconnected,
                if self.apps.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", self.apps.join(", "))
                }
            ));
        }
        if self.app_frames > 0 {
            let mut p: Vec<String> = self
                .app_periods
                .iter()
                .map(|(k, n)| format!("{:.2} ms x{n}", f64::from(*k) / 100.0))
                .collect();
            p.truncate(6);
            v.push(format!(
                "app pacing: {} frames delivered, {} more than 2 ms late; periods given: {}",
                self.app_frames,
                self.app_late_gt2ms,
                p.join(", ")
            ));
        }
        v.push(format!(
            "compositor missed-frame warnings: {}",
            self.missed.len()
        ));
        if !self.dumps.is_empty() {
            v.push(format!("frame dumps written: {}", self.dumps.len()));
        }
        if self.server_exited {
            v.push("service has exited".into());
        }
        v.push(format!(
            "log: {} error lines, {} warning lines",
            self.errors.len(),
            self.warnings.len()
        ));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lines copied from real monado-service logs (2026-10-13/14 sessions).
    const GYRO: &str = " INFO [bias_finish] gyro bias done: 0.267 0.687 0.453 dps over 2000 samples (0 restarts); accel -0.035 0.968 0.241 g";
    const PREP: &str = " INFO [pimax_hmd_create] pimax_video_prepare(90) -> 0";
    const MODE: &str = "DEBUG [get_primary_display_mode] found display mode 7104x3840@90.00";

    #[test]
    fn gyro_provisional_and_refined_lines() {
        let p = " WARN [bias_provisional] gyro bias provisional: 0.120 0.781 0.366 dps (gyro sd 0.210 0.190 0.230 dps: not a real rest, not cached); put the headset still on the desk for 2 s to calibrate the gyro";
        assert_eq!(parse_line(p), Some(LogEvent::GyroBiasProvisional { gyro_dps: [0.120, 0.781, 0.366] }));
        let r = " INFO [bias_finish] gyro bias refined at rest: 0.118 0.775 0.370 dps (moved 0.006; 3 rests this session, 9 windows rejected as not a real rest)";
        assert_eq!(parse_line(r), Some(LogEvent::GyroBiasRefined { gyro_dps: [0.118, 0.775, 0.370] }));
    }

    #[test]
    fn gyro_fallback_line_counts_as_ready() {
        let l = " WARN [bias_timeout] headset not still for 3000 ms (13 restarts): tracking with the cached gyro bias 0.087 0.684 0.474 dps; it is refined the next time the headset rests for 2000 ms";
        assert_eq!(
            parse_line(l),
            Some(LogEvent::GyroBiasFallback {
                gyro_dps: [0.087, 0.684, 0.474],
                cached: true,
                restarts: 13
            })
        );
        let z = l.replace(
            "cached gyro bias 0.087 0.684 0.474",
            "zero (no cache) gyro bias 0.000 0.000 0.000",
        );
        assert!(matches!(
            parse_line(&z),
            Some(LogEvent::GyroBiasFallback { cached: false, .. })
        ));
        let mut s = Summary::default();
        s.feed(l);
        assert!(s.ready());
    }

    #[test]
    fn gyro_line() {
        assert_eq!(
            parse_line(GYRO),
            Some(LogEvent::GyroBiasDone {
                gyro_dps: [0.267, 0.687, 0.453],
                samples: 2000,
                restarts: 0
            })
        );
        let moved = GYRO.replace("(0 restarts)", "(3 restarts)");
        assert!(matches!(
            parse_line(&moved),
            Some(LogEvent::GyroBiasDone { restarts: 3, .. })
        ));
        // the controller line is a different message and must not count as "ready"
        assert_eq!(
            parse_line(
                " INFO [pimax_ctrl_on_report] IMU bias done (gyro 0.1 0.2 0.3 rad/s, |acc| 1.00 g)"
            ),
            None
        );
        // truncated line
        assert_eq!(
            parse_line(" INFO [bias_finish] gyro bias done: 0.267 0.687"),
            None
        );
    }

    #[test]
    fn video_prepare_and_mode_lines() {
        assert_eq!(
            parse_line(PREP),
            Some(LogEvent::VideoPrepare { fps: 90, rc: 0 })
        );
        assert_eq!(
            parse_line(" INFO [pimax_hmd_create] pimax_video_prepare(72) -> -5"),
            Some(LogEvent::VideoPrepare { fps: 72, rc: -5 })
        );
        assert_eq!(
            parse_line(MODE),
            Some(LogEvent::DisplayMode {
                w: 7104,
                h: 3840,
                hz: 90.0
            })
        );
        assert_eq!(
            parse_line("\testimated_frame_period: 13.888888ms"),
            Some(LogEvent::FramePeriod { ms: 13.888888 })
        );
        assert_eq!(parse_line("\tpercentage: 20.000000%"), None);
        assert_eq!(
            parse_line("DEBUG [get_primary_display_mode] Found 4 modes"),
            None
        );
    }

    #[test]
    fn client_and_exit_lines() {
        assert_eq!(
            parse_line(" INFO [client_connected] Client 3 connected"),
            Some(LogEvent::ClientConnected(3))
        );
        assert_eq!(
            parse_line(" INFO [client_disconnected] Client 3 disconnected"),
            Some(LogEvent::ClientDisconnected(3))
        );
        assert_eq!(
            parse_line("\tapplication_name: 'Unity Application'"),
            Some(LogEvent::ClientApp("Unity Application".into()))
        );
        assert_eq!(
            parse_line(" INFO [ipc_server_main_common] Server exiting: '0'"),
            Some(LogEvent::ServerExiting)
        );
        assert_eq!(
            parse_line(" INFO [debug_dump_target] dumped /tmp/d/frame100_7104x3840_fmt64.raw"),
            Some(LogEvent::Dumped(
                "/tmp/d/frame100_7104x3840_fmt64.raw".into()
            ))
        );
    }

    #[test]
    fn pacing_lines() {
        assert_eq!(
            parse_line(" WARN [adjust_comp_time] Frame 4711 missed by 3.40!"),
            Some(LogEvent::MissedFrame {
                frame: 4711,
                ms: 3.40
            })
        );
        assert_eq!(
            parse_line("DEBUG [pa_mark_gpu_done] Delivered frame 4.75ms late."),
            Some(LogEvent::AppDelivered {
                ms: 4.75,
                late: true
            })
        );
        assert_eq!(
            parse_line("DEBUG [pa_mark_gpu_done] Delivered frame 0.30ms early."),
            Some(LogEvent::AppDelivered {
                ms: 0.30,
                late: false
            })
        );
        assert_eq!(
            parse_line("\tperiod: 22.22"),
            Some(LogEvent::AppPeriod { ms: 22.22 })
        );
    }

    #[test]
    fn warnings_and_errors() {
        assert_eq!(
            parse_line(" WARN [u_linux_try_to_set_realtime_priority_on_thread] Could not raise priority for thread 'VBlank Events'"),
            None
        );
        assert_eq!(
            parse_line(" WARN [build_extension_list] Duplicate extension VK_KHR_get_surface_capabilities2 not added twice"),
            None
        );
        assert_eq!(
            parse_line(" WARN [transfer_layers_locked] Session is visible but not active."),
            None
        );
        assert!(matches!(
            parse_line(" WARN [x] something odd"),
            Some(LogEvent::Warning(_))
        ));
        assert!(matches!(
            parse_line("ERROR [render_resources_init] vkCreateFoo failed"),
            Some(LogEvent::Error(_))
        ));
        assert_eq!(parse_line(" INFO [x] all fine"), None);
        assert_eq!(parse_line(""), None);
    }

    #[test]
    fn line_buffer_joins_partial_lines() {
        let mut b = LineBuffer::default();
        assert!(b.push(" INFO [bias_fin").is_empty());
        let l = b.push("ish] gyro bias done: 1 2 3 dps over 4 samples (0 restarts)\r\nnext");
        assert_eq!(l.len(), 1);
        assert!(l[0].ends_with("(0 restarts)"));
        assert_eq!(
            b.push(" line\n\n"),
            vec!["next line".to_string(), String::new()]
        );
    }

    #[test]
    fn line_buffer_keeps_multibyte_characters_that_arrive_in_two_chunks() {
        let mut b = LineBuffer::default();
        let bytes = "Zoë: ok\n".as_bytes(); // ë is two bytes
        assert!(b.push_bytes(&bytes[..3]).is_empty());
        assert_eq!(b.push_bytes(&bytes[3..]), vec!["Zoë: ok".to_string()]);
        // invalid UTF-8 is replaced, the line still arrives
        assert_eq!(b.push_bytes(b"a\xffb\n"), vec!["a\u{fffd}b".to_string()]);
    }

    #[test]
    fn summary_from_reader_survives_bad_bytes_and_matches_from_text() {
        let text = format!("{PREP}\n garbage \u{fffd}\n{GYRO}\n");
        let mut raw = text.clone().into_bytes();
        raw.splice(0..0, b"\xff\xfe not utf8\n".iter().copied());
        let a = Summary::from_reader(std::io::Cursor::new(raw));
        let b = Summary::from_text(&text);
        assert_eq!(a.video_prepare, b.video_prepare);
        assert!(a.ready() && b.ready());
    }

    #[test]
    fn summary_of_a_realistic_log() {
        let text = format!(
            "{PREP}\n{MODE}\n\testimated_frame_period: 11.111111ms\n {w} \n{GYRO}\n\
             \x20INFO [client_connected] Client 1 connected\n\tapplication_name: 'xr_eyetest'\n\
             DEBUG [pa_mark_gpu_done] Delivered frame 5.30ms late.\n\tperiod: 11.11\n\
             DEBUG [pa_mark_gpu_done] Delivered frame 0.30ms early.\n\tperiod: 22.22\n\
             DEBUG [pa_mark_gpu_done] Delivered frame 0.50ms early.\n\tperiod: 22.22\n\
             \x20INFO [debug_dump_target] dumped /tmp/d/frame100_7104x3840_fmt64.raw\n\
             \x20INFO [client_disconnected] Client 1 disconnected\n\
             \x20INFO [ipc_server_main_common] Server exiting: '0'\n",
            w = " WARN [u_linux_try_to_set_realtime_priority_on_thread] Could not raise priority for thread 'x'"
        );
        let s = Summary::from_text(&text);
        assert!(s.ready());
        assert_eq!(s.video_prepare, Some((90, 0)));
        assert_eq!(s.display_mode, Some((7104, 3840, 90.0)));
        assert_eq!(s.nominal_period_ms(), Some(11.111111));
        assert_eq!(s.clients_connected, vec![1]);
        assert_eq!(s.apps, vec!["xr_eyetest".to_string()]);
        assert_eq!(s.app_frames, 3);
        assert_eq!(s.app_late_gt2ms, 1);
        assert_eq!(s.app_periods.get(&2222), Some(&2));
        assert_eq!(s.app_periods.get(&1111), Some(&1));
        assert!(s.server_exited);
        assert!(s.errors.is_empty() && s.warnings.is_empty());
        assert_eq!(s.dumps.len(), 1);
        let r = s.report().join("\n");
        assert!(r.contains("90 Hz announced (rc 0)"), "{r}");
        assert!(r.contains("7104x3840 @ 90.00 Hz"), "{r}");
        assert!(r.contains("11.111 ms (90.00 Hz)"), "{r}");
        assert!(r.contains("22.22 ms x2"), "{r}");
        assert!(r.contains("missed-frame warnings: 0"), "{r}");
        assert!(r.contains("service has exited"), "{r}");
    }

    #[test]
    fn rate_warnings_catch_a_compositor_that_disagrees_with_the_headset() {
        let ok = Summary::from_text(&format!(
            "{PREP}\n{MODE}\n\testimated_frame_period: 11.111111ms\n"
        ));
        assert!(
            ok.rate_warnings(90).is_empty(),
            "{:?}",
            ok.rate_warnings(90)
        );
        // asked for 72 but everything says 90
        let w = ok.rate_warnings(72);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].contains("told 90 Hz but 72 Hz was requested"), "{w:?}");
        assert!(w[1].contains("11.111 ms (90.00 Hz), not 72 Hz"), "{w:?}");
        assert!(w[2].contains("7104x3840 @ 90.00 Hz, not 72 Hz"), "{w:?}");
        // the run of 2026-10-13 that never lit the panels: firmware told 72, compositor auto-picked 90
        let bad = Summary::from_text(
            " INFO [pimax_hmd_create] pimax_video_prepare(72) -> 0\n\testimated_frame_period: 11.111111ms\n",
        );
        assert_eq!(bad.rate_warnings(72).len(), 1);
        // failed video prepare and a log without the line at all
        let failed = Summary::from_text(" INFO [pimax_hmd_create] pimax_video_prepare(90) -> -5");
        assert!(failed.rate_warnings(90)[0].contains("failed with -5"));
        assert!(Summary::default().rate_warnings(90)[0].contains("no pimax_video_prepare"));
    }

    #[test]
    fn nominal_period_falls_back_to_the_announced_rate() {
        let s = Summary::from_text(PREP);
        let p = s.nominal_period_ms().unwrap();
        assert!((p - 11.111).abs() < 0.001);
        assert_eq!(Summary::default().nominal_period_ms(), None);
        assert!(!Summary::default().ready());
        assert!(Summary::default()
            .report()
            .join("\n")
            .contains("gyro calibration: not finished"));
    }
}
