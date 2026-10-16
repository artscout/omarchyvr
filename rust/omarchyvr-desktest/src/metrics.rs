//! Decoder and summaries for Monado's `XRT_METRICS_FILE`.
//!
//! The file is a stream of `varint length | Record` (nanopb `pb_encode_submessage`), `Record` being a protobuf `oneof` of
//! `Version(1) SessionFrame(2) Used(3) SystemFrame(4) SystemGpuInfo(5) SystemPresentInfo(6)`
//! (`src/external/metrics/monado_metrics.pb.h`). Only the fields needed for timing analysis are kept.

use std::collections::BTreeMap;
use std::fmt;

/// Wire-level decode error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// A varint ran past the end of the buffer or exceeded 10 bytes.
    BadVarint,
    /// A length-delimited field claims more bytes than are left.
    Truncated,
    /// Group wire types (3, 4) and unknown types are not used by this schema.
    BadWireType(u8),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::BadVarint => write!(f, "malformed varint"),
            DecodeError::Truncated => write!(f, "truncated length-delimited field"),
            DecodeError::BadWireType(t) => write!(f, "unsupported wire type {t}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Reads a base-128 varint at `*pos`, advancing it.
pub fn read_varint(buf: &[u8], pos: &mut usize) -> Result<u64, DecodeError> {
    let mut v: u64 = 0;
    for i in 0..10 {
        let b = *buf.get(*pos).ok_or(DecodeError::BadVarint)?;
        *pos += 1;
        v |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(DecodeError::BadVarint)
}

/// One decoded protobuf field value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    Fixed32(u32),
}

/// Iterator over the `(field number, value)` pairs of one message.
pub struct Fields<'a> {
    buf: &'a [u8],
    pos: usize,
    failed: bool,
}

/// Iterates the fields of `msg`.
pub fn fields(msg: &[u8]) -> Fields<'_> {
    Fields { buf: msg, pos: 0, failed: false }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<(u32, Wire<'a>), DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pos >= self.buf.len() {
            return None;
        }
        let res = (|| {
            let key = read_varint(self.buf, &mut self.pos)?;
            let num = (key >> 3) as u32;
            let val = match (key & 7) as u8 {
                0 => Wire::Varint(read_varint(self.buf, &mut self.pos)?),
                1 => {
                    let b = self.buf.get(self.pos..self.pos + 8).ok_or(DecodeError::Truncated)?;
                    self.pos += 8;
                    Wire::Fixed64(u64::from_le_bytes(b.try_into().unwrap()))
                }
                2 => {
                    let len = read_varint(self.buf, &mut self.pos)? as usize;
                    let end = self.pos.checked_add(len).ok_or(DecodeError::Truncated)?;
                    let b = self.buf.get(self.pos..end).ok_or(DecodeError::Truncated)?;
                    self.pos = end;
                    Wire::Len(b)
                }
                5 => {
                    let b = self.buf.get(self.pos..self.pos + 4).ok_or(DecodeError::Truncated)?;
                    self.pos += 4;
                    Wire::Fixed32(u32::from_le_bytes(b.try_into().unwrap()))
                }
                t => return Err(DecodeError::BadWireType(t)),
            };
            Ok((num, val))
        })();
        if res.is_err() {
            self.failed = true;
        }
        Some(res)
    }
}

/// `SessionFrame`: one frame of one client session (xrWaitFrame .. xrEndFrame .. GPU done).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFrame {
    pub session_id: i64,
    pub frame_id: i64,
    pub predicted_display_period_ns: u64,
    pub when_wait_woke_ns: u64,
    pub when_begin_ns: u64,
    pub when_delivered_ns: u64,
    pub when_gpu_done_ns: u64,
    pub discarded: bool,
}

/// `Used`: the compositor used `session_frame_id` of `session_id` for system frame `system_frame_id`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Used {
    pub session_id: i64,
    pub session_frame_id: i64,
    pub system_frame_id: i64,
    pub when_ns: u64,
}

/// `SystemFrame`: compositor frame prediction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemFrame {
    pub frame_id: i64,
    pub predicted_display_period_ns: u64,
    pub desired_present_time_ns: u64,
    pub wake_up_time_ns: u64,
}

/// `SystemGpuInfo`: GPU timestamps of the compositor's own work for one frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuInfo {
    pub frame_id: i64,
    pub gpu_start_ns: u64,
    pub gpu_end_ns: u64,
    pub when_ns: u64,
}

/// `SystemPresentInfo`: timing of the present of one compositor frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresentInfo {
    pub frame_id: i64,
    pub when_woke_ns: u64,
    pub when_began_ns: u64,
    pub when_submitted_ns: u64,
    pub when_infoed_ns: u64,
    pub desired_present_time_ns: u64,
    pub present_margin_ns: u64,
    pub actual_present_time_ns: u64,
    pub earliest_present_time_ns: u64,
}

/// One decoded `Record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Version { major: u32, minor: u32 },
    SessionFrame(SessionFrame),
    Used(Used),
    SystemFrame(SystemFrame),
    GpuInfo(GpuInfo),
    PresentInfo(PresentInfo),
    /// A oneof arm this decoder does not know (field number).
    Other(u32),
}

fn varint_of(w: Wire<'_>) -> u64 {
    match w {
        Wire::Varint(v) | Wire::Fixed64(v) => v,
        Wire::Fixed32(v) => u64::from(v),
        Wire::Len(_) => 0,
    }
}

fn decode_msg<F: FnMut(u32, u64)>(msg: &[u8], mut f: F) -> Result<(), DecodeError> {
    for item in fields(msg) {
        let (n, w) = item?;
        f(n, varint_of(w));
    }
    Ok(())
}

/// Decodes one `Record` message (without its length prefix).
pub fn decode_record(msg: &[u8]) -> Result<Record, DecodeError> {
    for item in fields(msg) {
        let (num, w) = item?;
        let Wire::Len(body) = w else { continue };
        return Ok(match num {
            1 => {
                let (mut major, mut minor) = (0u32, 0u32);
                decode_msg(body, |n, v| match n {
                    1 => major = v as u32,
                    2 => minor = v as u32,
                    _ => {}
                })?;
                Record::Version { major, minor }
            }
            2 => {
                let mut r = SessionFrame::default();
                decode_msg(body, |n, v| match n {
                    1 => r.session_id = v as i64,
                    2 => r.frame_id = v as i64,
                    7 => r.predicted_display_period_ns = v,
                    10 => r.when_wait_woke_ns = v,
                    11 => r.when_begin_ns = v,
                    12 => r.when_delivered_ns = v,
                    13 => r.when_gpu_done_ns = v,
                    14 => r.discarded = v != 0,
                    _ => {}
                })?;
                Record::SessionFrame(r)
            }
            3 => {
                let mut r = Used::default();
                decode_msg(body, |n, v| match n {
                    1 => r.session_id = v as i64,
                    2 => r.session_frame_id = v as i64,
                    3 => r.system_frame_id = v as i64,
                    4 => r.when_ns = v,
                    _ => {}
                })?;
                Record::Used(r)
            }
            4 => {
                let mut r = SystemFrame::default();
                decode_msg(body, |n, v| match n {
                    1 => r.frame_id = v as i64,
                    3 => r.predicted_display_period_ns = v,
                    4 => r.desired_present_time_ns = v,
                    5 => r.wake_up_time_ns = v,
                    _ => {}
                })?;
                Record::SystemFrame(r)
            }
            5 => {
                let mut r = GpuInfo::default();
                decode_msg(body, |n, v| match n {
                    1 => r.frame_id = v as i64,
                    2 => r.gpu_start_ns = v,
                    3 => r.gpu_end_ns = v,
                    4 => r.when_ns = v,
                    _ => {}
                })?;
                Record::GpuInfo(r)
            }
            6 => {
                let mut r = PresentInfo::default();
                decode_msg(body, |n, v| match n {
                    1 => r.frame_id = v as i64,
                    7 => r.when_woke_ns = v,
                    8 => r.when_began_ns = v,
                    9 => r.when_submitted_ns = v,
                    10 => r.when_infoed_ns = v,
                    11 => r.desired_present_time_ns = v,
                    13 => r.present_margin_ns = v,
                    14 => r.actual_present_time_ns = v,
                    15 => r.earliest_present_time_ns = v,
                    _ => {}
                })?;
                Record::PresentInfo(r)
            }
            other => Record::Other(other),
        });
    }
    Ok(Record::Other(0))
}

/// Result of [`parse_stream`].
#[derive(Debug, Default)]
pub struct Parsed {
    pub records: Vec<Record>,
    /// Bytes at the end that could not be decoded (a file cut short by SIGKILL is normal).
    pub trailing_bytes: usize,
}

/// Decodes a whole metrics file. A truncated or corrupt tail ends parsing and is reported, not an error.
pub fn parse_stream(data: &[u8]) -> Parsed {
    let mut out = Parsed::default();
    let mut pos = 0usize;
    while pos < data.len() {
        let start = pos;
        let Ok(len) = read_varint(data, &mut pos) else {
            out.trailing_bytes = data.len() - start;
            break;
        };
        let end = pos.saturating_add(len as usize);
        let Some(msg) = data.get(pos..end) else {
            out.trailing_bytes = data.len() - start;
            break;
        };
        match decode_record(msg) {
            Ok(r) => out.records.push(r),
            Err(_) => {
                out.trailing_bytes = data.len() - start;
                break;
            }
        }
        pos = end;
    }
    out
}

/// Distribution summary in milliseconds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

/// Linear-interpolated percentile of an ascending-sorted slice (`q` in 0..=1).
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// Summarises `values` (any unit).
pub fn stats(values: &[f64]) -> Stats {
    if values.is_empty() {
        return Stats::default();
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    Stats {
        n: v.len(),
        mean: v.iter().sum::<f64>() / v.len() as f64,
        p50: percentile(&v, 0.5),
        p95: percentile(&v, 0.95),
        p99: percentile(&v, 0.99),
        max: *v.last().unwrap(),
    }
}

/// Statistics of the compositor frames that used one particular set of client sessions.
#[derive(Debug, Clone, Default)]
pub struct ClassReport {
    /// Session ids that contributed layers, ascending.
    pub sessions: Vec<i64>,
    /// Compositor frames in this class after trimming.
    pub frames: usize,
    /// Wall time covered (first to last frame, trimmed), seconds.
    pub seconds: f64,
    /// Compositor GPU time per frame (ms).
    pub gpu_ms: Stats,
    /// Distance between consecutive frame starts (ms), the cadence.
    pub period_ms: Stats,
    /// Frames whose period exceeded 1.5x the nominal period.
    pub late_frames: usize,
}

/// Per-session (client) summary.
#[derive(Debug, Clone, Default)]
pub struct SessionReport {
    pub session_id: i64,
    pub frames: usize,
    pub discarded: usize,
    /// First and last `when_begin_ns` (monotonic ns).
    pub first_ns: u64,
    pub last_ns: u64,
    /// xrBeginFrame .. xrEndFrame (ms).
    pub begin_to_delivered_ms: Stats,
    /// xrEndFrame .. client GPU work done (ms).
    pub delivered_to_gpu_done_ms: Stats,
}

/// Whole-file report.
#[derive(Debug, Default)]
pub struct Report {
    pub nominal_period_ms: f64,
    pub classes: Vec<ClassReport>,
    pub sessions: Vec<SessionReport>,
}

/// Summarises a decoded file. `trim_s` seconds are dropped at the start and end of each class so that
/// transitions (a client appearing or exiting) do not pollute the numbers.
pub fn analyze(records: &[Record], trim_s: f64) -> Report {
    analyze_with(records, trim_s, &|_| true)
}

/// Like [`analyze`] but only frames whose start time (monotonic ns, the clock Monado stamps metrics with) satisfies `keep`
/// are used: compare the compositor inside and outside a window, e.g. while a voice benchmark runs.
pub fn analyze_with(records: &[Record], trim_s: f64, keep: &dyn Fn(u64) -> bool) -> Report {
    let mut report = Report::default();
    let mut used: BTreeMap<i64, Vec<i64>> = BTreeMap::new(); // system frame -> sessions
    let mut gpu: BTreeMap<i64, (u64, u64)> = BTreeMap::new();
    let mut start: BTreeMap<i64, u64> = BTreeMap::new(); // system frame -> when_began_ns
    let mut sess: BTreeMap<i64, Vec<&SessionFrame>> = BTreeMap::new();
    let mut period_hint = 0u64;
    for r in records {
        match r {
            Record::Used(u) => used.entry(u.system_frame_id).or_default().push(u.session_id),
            Record::GpuInfo(g) => {
                gpu.insert(g.frame_id, (g.gpu_start_ns, g.gpu_end_ns));
            }
            Record::PresentInfo(p) => {
                let t = if p.when_began_ns != 0 { p.when_began_ns } else { p.when_woke_ns };
                start.insert(p.frame_id, t);
            }
            Record::SystemFrame(s) => {
                if s.predicted_display_period_ns != 0 {
                    period_hint = s.predicted_display_period_ns;
                }
            }
            Record::SessionFrame(f) => sess.entry(f.session_id).or_default().push(f),
            _ => {}
        }
    }
    report.nominal_period_ms = period_hint as f64 / 1e6;

    // Group compositor frames by the set of sessions used.
    let mut classes: BTreeMap<Vec<i64>, Vec<i64>> = BTreeMap::new();
    for (&frame, sessions) in &used {
        let mut key = sessions.clone();
        key.sort_unstable();
        key.dedup();
        classes.entry(key).or_default().push(frame);
    }
    // Frames with no session at all are not in `used`; they are not interesting here.
    for (key, mut frames) in classes {
        frames.sort_unstable();
        let t_of = |f: &i64| start.get(f).copied().or_else(|| gpu.get(f).map(|g| g.0));
        let times: Vec<(i64, u64)> = frames.iter().filter_map(|f| t_of(f).map(|t| (*f, t))).filter(|(_, t)| keep(*t)).collect();
        if times.len() < 3 {
            continue;
        }
        let t0 = times.first().unwrap().1;
        let t1 = times.last().unwrap().1;
        let trim = (trim_s * 1e9) as u64;
        let kept: Vec<&(i64, u64)> = times
            .iter()
            .filter(|(_, t)| *t >= t0.saturating_add(trim) && *t + trim <= t1)
            .collect();
        if kept.len() < 2 {
            continue;
        }
        let gpu_ms: Vec<f64> = kept
            .iter()
            .filter_map(|(f, _)| gpu.get(f))
            .filter(|g| g.1 >= g.0)
            .map(|g| (g.1 - g.0) as f64 / 1e6)
            .collect();
        let periods: Vec<f64> = kept
            .windows(2)
            .filter(|w| w[1].0 == w[0].0 + 1)
            .map(|w| (w[1].1.saturating_sub(w[0].1)) as f64 / 1e6)
            .collect();
        let nominal = report.nominal_period_ms;
        let late = if nominal > 0.0 { periods.iter().filter(|p| **p > 1.5 * nominal).count() } else { 0 };
        report.classes.push(ClassReport {
            sessions: key,
            frames: kept.len(),
            seconds: (kept.last().unwrap().1 - kept.first().unwrap().1) as f64 / 1e9,
            gpu_ms: stats(&gpu_ms),
            period_ms: stats(&periods),
            late_frames: late,
        });
    }

    for (id, frames) in sess {
        let frames: Vec<&SessionFrame> = frames.into_iter().filter(|f| f.when_begin_ns == 0 || keep(f.when_begin_ns)).collect();
        if frames.is_empty() {
            continue;
        }
        let b2d: Vec<f64> = frames
            .iter()
            .filter(|f| f.when_delivered_ns >= f.when_begin_ns && f.when_begin_ns != 0)
            .map(|f| (f.when_delivered_ns - f.when_begin_ns) as f64 / 1e6)
            .collect();
        let d2g: Vec<f64> = frames
            .iter()
            .filter(|f| f.when_gpu_done_ns >= f.when_delivered_ns && f.when_delivered_ns != 0)
            .map(|f| (f.when_gpu_done_ns - f.when_delivered_ns) as f64 / 1e6)
            .collect();
        let begins: Vec<u64> = frames.iter().map(|f| f.when_begin_ns).filter(|t| *t != 0 && keep(*t)).collect();
        report.sessions.push(SessionReport {
            session_id: id,
            frames: frames.len(),
            discarded: frames.iter().filter(|f| f.discarded).count(),
            first_ns: begins.iter().copied().min().unwrap_or(0),
            last_ns: begins.iter().copied().max().unwrap_or(0),
            begin_to_delivered_ms: stats(&b2d),
            delivered_to_gpu_done_ms: stats(&d2g),
        });
    }
    report
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "nominal frame period {:.3} ms", self.nominal_period_ms)?;
        writeln!(f, "compositor frames by set of contributing sessions (GPU ms = compositor's own render+distortion):")?;
        for c in &self.classes {
            writeln!(
                f,
                "  sessions {:?}: {} frames over {:.1} s | gpu mean {:.3} p50 {:.3} p95 {:.3} p99 {:.3} max {:.3} ms | period mean {:.3} p99 {:.3} max {:.3} ms, late(>1.5x) {}",
                c.sessions, c.frames, c.seconds, c.gpu_ms.mean, c.gpu_ms.p50, c.gpu_ms.p95, c.gpu_ms.p99, c.gpu_ms.max,
                c.period_ms.mean, c.period_ms.p99, c.period_ms.max, c.late_frames
            )?;
        }
        writeln!(f, "client sessions:")?;
        for s in &self.sessions {
            let secs = (s.last_ns.saturating_sub(s.first_ns)) as f64 / 1e9;
            let fps = if secs > 0.0 { s.frames as f64 / secs } else { 0.0 };
            writeln!(
                f,
                "  session {}: {} frames ({} discarded) over {:.1} s = {:.1} fps | begin->end {:.3} ms mean (p99 {:.3}) | end->gpu done {:.3} ms mean (p99 {:.3})",
                s.session_id, s.frames, s.discarded, secs, fps, s.begin_to_delivered_ms.mean, s.begin_to_delivered_ms.p99,
                s.delivered_to_gpu_done_ms.mean, s.delivered_to_gpu_done_ms.p99
            )?;
        }
        Ok(())
    }
}

/// Minimal protobuf writer used by the tests (and handy for synthetic fixtures).
pub mod enc {
    /// Appends a varint.
    pub fn varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }
    /// Appends a varint field.
    pub fn field(out: &mut Vec<u8>, num: u32, v: u64) {
        varint(out, u64::from(num) << 3);
        varint(out, v);
    }
    /// Appends a length-delimited field.
    pub fn bytes(out: &mut Vec<u8>, num: u32, b: &[u8]) {
        varint(out, (u64::from(num) << 3) | 2);
        varint(out, b.len() as u64);
        out.extend_from_slice(b);
    }
    /// Appends `varint(len) | Record{ oneof arm num = body }` as nanopb would.
    pub fn record(out: &mut Vec<u8>, arm: u32, body: &[u8]) {
        let mut rec = Vec::new();
        bytes(&mut rec, arm, body);
        varint(out, rec.len() as u64);
        out.extend_from_slice(&rec);
    }
}

#[cfg(test)]
mod tests {
    use super::enc;
    use super::*;

    #[test]
    fn varint_roundtrip_and_errors() {
        for v in [0u64, 1, 127, 128, 300, u32::MAX as u64, u64::MAX] {
            let mut b = Vec::new();
            enc::varint(&mut b, v);
            let mut pos = 0;
            assert_eq!(read_varint(&b, &mut pos), Ok(v));
            assert_eq!(pos, b.len());
        }
        assert_eq!(read_varint(&[0x80], &mut 0), Err(DecodeError::BadVarint));
        assert_eq!(read_varint(&[0xff; 11], &mut 0), Err(DecodeError::BadVarint));
        assert_eq!(read_varint(&[], &mut 0), Err(DecodeError::BadVarint));
    }

    #[test]
    fn negative_int64_is_ten_bytes_and_roundtrips() {
        let mut b = Vec::new();
        enc::field(&mut b, 1, (-5i64) as u64);
        assert_eq!(b.len(), 11);
        let mut r = Vec::new();
        enc::record(&mut r, 3, &b);
        let p = parse_stream(&r);
        assert_eq!(p.records, vec![Record::Used(Used { session_id: -5, ..Default::default() })]);
    }

    #[test]
    fn fields_reports_truncation_and_bad_wire_type() {
        let mut b = Vec::new();
        enc::varint(&mut b, (1 << 3) | 2);
        enc::varint(&mut b, 10);
        b.extend_from_slice(&[1, 2, 3]);
        assert_eq!(fields(&b).next().unwrap(), Err(DecodeError::Truncated));
        let g = [(1u8 << 3) | 3];
        assert_eq!(fields(&g).next().unwrap(), Err(DecodeError::BadWireType(3)));
        // fixed32/fixed64
        let mut f = Vec::new();
        enc::varint(&mut f, (2 << 3) | 5);
        f.extend_from_slice(&7u32.to_le_bytes());
        enc::varint(&mut f, (3 << 3) | 1);
        f.extend_from_slice(&9u64.to_le_bytes());
        let v: Vec<_> = fields(&f).map(|x| x.unwrap()).collect();
        assert_eq!(v, vec![(2, Wire::Fixed32(7)), (3, Wire::Fixed64(9))]);
    }

    fn sample_stream() -> Vec<u8> {
        let mut out = Vec::new();
        let mut v = Vec::new();
        enc::field(&mut v, 1, 1);
        enc::field(&mut v, 2, 1);
        enc::record(&mut out, 1, &v);
        // 120 compositor frames at 10 ms; session 1 for all, session 2 joins for frames 60..120
        for i in 0..120i64 {
            let t = 1_000_000_000u64 + (i as u64) * 10_000_000;
            let mut sf = Vec::new();
            enc::field(&mut sf, 1, i as u64);
            enc::field(&mut sf, 3, 10_000_000);
            enc::record(&mut out, 4, &sf);
            let mut u = Vec::new();
            enc::field(&mut u, 1, 1);
            enc::field(&mut u, 2, i as u64);
            enc::field(&mut u, 3, i as u64);
            enc::record(&mut out, 3, &u);
            if i >= 60 {
                let mut u2 = Vec::new();
                enc::field(&mut u2, 1, 2);
                enc::field(&mut u2, 2, i as u64);
                enc::field(&mut u2, 3, i as u64);
                enc::record(&mut out, 3, &u2);
            }
            let gpu_ms = if i >= 60 { 3 } else { 2 };
            let mut g = Vec::new();
            enc::field(&mut g, 1, i as u64);
            enc::field(&mut g, 2, t);
            enc::field(&mut g, 3, t + gpu_ms * 1_000_000);
            enc::record(&mut out, 5, &g);
            let mut p = Vec::new();
            enc::field(&mut p, 1, i as u64);
            enc::field(&mut p, 8, t);
            enc::record(&mut out, 6, &p);
            let mut s = Vec::new();
            enc::field(&mut s, 1, 1);
            enc::field(&mut s, 2, i as u64);
            enc::field(&mut s, 11, t);
            enc::field(&mut s, 12, t + 1_000_000);
            enc::field(&mut s, 13, t + 3_000_000);
            enc::record(&mut out, 2, &s);
        }
        out
    }

    #[test]
    fn parses_stream_and_reports_classes() {
        let data = sample_stream();
        let p = parse_stream(&data);
        assert_eq!(p.trailing_bytes, 0);
        assert_eq!(p.records[0], Record::Version { major: 1, minor: 1 });
        let r = analyze(&p.records, 0.05);
        assert!((r.nominal_period_ms - 10.0).abs() < 1e-9);
        assert_eq!(r.classes.len(), 2);
        let alone = r.classes.iter().find(|c| c.sessions == vec![1]).unwrap();
        let both = r.classes.iter().find(|c| c.sessions == vec![1, 2]).unwrap();
        assert!((alone.gpu_ms.mean - 2.0).abs() < 1e-9, "{}", alone.gpu_ms.mean);
        assert!((both.gpu_ms.mean - 3.0).abs() < 1e-9);
        assert!((both.period_ms.mean - 10.0).abs() < 1e-9);
        assert_eq!(both.late_frames, 0);
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].frames, 120);
        assert!((r.sessions[0].begin_to_delivered_ms.mean - 1.0).abs() < 1e-9);
        assert!((r.sessions[0].delivered_to_gpu_done_ms.mean - 2.0).abs() < 1e-9);
        let text = r.to_string();
        assert!(text.contains("sessions [1, 2]"));
    }

    #[test]
    fn truncated_tail_is_reported_not_fatal() {
        let mut data = sample_stream();
        let full = parse_stream(&data).records.len();
        data.truncate(data.len() - 3);
        let p = parse_stream(&data);
        assert_eq!(p.records.len(), full - 1);
        assert!(p.trailing_bytes > 0);
    }

    #[test]
    fn late_frames_are_counted() {
        let mut out = Vec::new();
        let mut t = 1_000_000_000u64;
        for i in 0..100i64 {
            // frame 50 starts 25 ms after frame 49
            t += if i == 50 { 25_000_000 } else { 10_000_000 };
            let mut sf = Vec::new();
            enc::field(&mut sf, 1, i as u64);
            enc::field(&mut sf, 3, 10_000_000);
            enc::record(&mut out, 4, &sf);
            let mut u = Vec::new();
            enc::field(&mut u, 1, 1);
            enc::field(&mut u, 3, i as u64);
            enc::record(&mut out, 3, &u);
            let mut p = Vec::new();
            enc::field(&mut p, 1, i as u64);
            enc::field(&mut p, 8, t);
            enc::record(&mut out, 6, &p);
        }
        let r = analyze(&parse_stream(&out).records, 0.0);
        assert_eq!(r.classes[0].late_frames, 1);
        assert!((r.classes[0].period_ms.max - 25.0).abs() < 1e-9);
    }

    #[test]
    fn window_filter_splits_inside_and_outside() {
        let data = sample_stream(); // frames at 1.000 s + i * 10 ms, i in 0..120
        let recs = parse_stream(&data).records;
        let t = |i: u64| 1_000_000_000u64 + i * 10_000_000;
        let inside = analyze_with(&recs, 0.0, &|x| x >= t(30) && x < t(90));
        let outside = analyze_with(&recs, 0.0, &|x| !(x >= t(30) && x < t(90)));
        let n_in: usize = inside.classes.iter().map(|c| c.frames).sum();
        let n_out: usize = outside.classes.iter().map(|c| c.frames).sum();
        assert_eq!(n_in, 60);
        assert_eq!(n_out, 60);
        assert_eq!(inside.sessions[0].frames, 60);
        // inside the window both session sets occur (frames 30..60 alone, 60..90 with both)
        assert_eq!(inside.classes.len(), 2);
        assert!(analyze_with(&recs, 0.0, &|_| false).classes.is_empty());
    }

    #[test]
    fn percentile_and_stats() {
        let s = stats(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(s.n, 5);
        assert!((s.mean - 3.0).abs() < 1e-12);
        assert!((s.p50 - 3.0).abs() < 1e-12);
        assert!((s.p95 - 4.8).abs() < 1e-12);
        assert_eq!(s.max, 5.0);
        assert!(percentile(&[], 0.5).is_nan());
        assert_eq!(stats(&[]).n, 0);
    }

    #[test]
    fn unknown_record_arm_is_kept_as_other() {
        let mut out = Vec::new();
        enc::record(&mut out, 9, &[0x08, 0x01]);
        assert_eq!(parse_stream(&out).records, vec![Record::Other(9)]);
    }
}
