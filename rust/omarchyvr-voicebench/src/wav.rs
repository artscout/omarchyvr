//! WAV (RIFF/WAVE) reading and writing, no dependencies.
//!
//! Reads PCM 8/16/24/32-bit, IEEE float 32/64 and `WAVE_FORMAT_EXTENSIBLE` wrappers of those, any channel count; walks chunks
//! with the RIFF pad byte rule; tolerates a streaming `data` chunk (size 0 or 0xFFFFFFFF) and a truncated final chunk.
//! Writes PCM16 or float32.

use std::fmt;
use std::path::Path;

/// Decoded audio: interleaved `f32` samples in -1..1.
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples (`frames * channels` values).
    pub samples: Vec<f32>,
}

/// Why a WAV could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    NotRiff,
    NoFmt,
    NoData,
    Truncated(&'static str),
    /// (format tag, bits per sample)
    Unsupported(u16, u16),
    BadHeader(&'static str),
    Io(String),
}

impl fmt::Display for WavError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WavError::NotRiff => write!(f, "not a RIFF/WAVE file"),
            WavError::NoFmt => write!(f, "no fmt chunk before data"),
            WavError::NoData => write!(f, "no data chunk"),
            WavError::Truncated(w) => write!(f, "truncated {w}"),
            WavError::Unsupported(t, b) => write!(f, "unsupported format tag {t} with {b} bits per sample"),
            WavError::BadHeader(w) => write!(f, "bad header: {w}"),
            WavError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WavError {}

impl Wav {
    /// Number of sample frames.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// Duration in seconds.
    pub fn duration_s(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate.max(1))
    }

    /// Averages all channels into one.
    pub fn to_mono(&self) -> Vec<f32> {
        let c = usize::from(self.channels.max(1));
        if c == 1 {
            return self.samples.clone();
        }
        self.samples.chunks_exact(c).map(|f| f.iter().sum::<f32>() / c as f32).collect()
    }
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Parses a WAV file image.
pub fn parse(bytes: &[u8]) -> Result<Wav, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotRiff);
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None; // tag, channels, rate, bits
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32le(bytes, pos + 4) as usize;
        let body_start = pos + 8;
        match id {
            b"fmt " => {
                if size < 16 || body_start + 16 > bytes.len() {
                    return Err(WavError::Truncated("fmt chunk"));
                }
                let mut tag = u16le(bytes, body_start);
                let channels = u16le(bytes, body_start + 2);
                let rate = u32le(bytes, body_start + 4);
                let bits = u16le(bytes, body_start + 14);
                if tag == 0xFFFE {
                    // WAVE_FORMAT_EXTENSIBLE: cbSize(2) validBits(2) channelMask(4) subformat GUID (first 2 bytes = tag)
                    if size < 40 || body_start + 26 > bytes.len() {
                        return Err(WavError::Truncated("extensible fmt chunk"));
                    }
                    tag = u16le(bytes, body_start + 24);
                }
                if channels == 0 {
                    return Err(WavError::BadHeader("zero channels"));
                }
                if rate == 0 {
                    return Err(WavError::BadHeader("zero sample rate"));
                }
                fmt = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = fmt.ok_or(WavError::NoFmt)?;
                let avail = bytes.len() - body_start;
                // streaming writers leave 0 or 0xFFFFFFFF; truncated files have less than announced
                let len = if size == 0 || size == 0xFFFF_FFFF || size > avail { avail } else { size };
                let samples = decode_samples(&bytes[body_start..body_start + len], tag, bits)?;
                let whole = samples.len() - samples.len() % usize::from(channels);
                let mut samples = samples;
                samples.truncate(whole);
                return Ok(Wav { sample_rate: rate, channels, samples });
            }
            _ => {}
        }
        // chunks are word aligned
        pos = body_start.saturating_add(size).saturating_add(size & 1);
    }
    if fmt.is_none() {
        Err(WavError::NoFmt)
    } else {
        Err(WavError::NoData)
    }
}

/// Splits `d` into `N`-byte groups (a trailing partial group is dropped).
fn groups<const N: usize>(d: &[u8]) -> Vec<[u8; N]> {
    let mut out = Vec::with_capacity(d.len() / N);
    let mut i = 0;
    while i + N <= d.len() {
        let mut a = [0u8; N];
        a.copy_from_slice(&d[i..i + N]);
        out.push(a);
        i += N;
    }
    out
}

fn decode_samples(d: &[u8], tag: u16, bits: u16) -> Result<Vec<f32>, WavError> {
    Ok(match (tag, bits) {
        (1, 8) => d.iter().map(|&b| (f32::from(b) - 128.0) / 128.0).collect(),
        (1, 16) => groups::<2>(d).into_iter().map(|c| f32::from(i16::from_le_bytes(c)) / 32768.0).collect(),
        (1, 24) => groups::<3>(d).into_iter().map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0).collect(),
        (1, 32) => groups::<4>(d).into_iter().map(|c| i32::from_le_bytes(c) as f32 / 2_147_483_648.0).collect(),
        (3, 32) => groups::<4>(d).into_iter().map(f32::from_le_bytes).collect(),
        (3, 64) => groups::<8>(d).into_iter().map(|c| f64::from_le_bytes(c) as f32).collect(),
        (t, b) => return Err(WavError::Unsupported(t, b)),
    })
}

/// Reads a WAV file.
pub fn read(path: &Path) -> Result<Wav, WavError> {
    parse(&std::fs::read(path).map_err(|e| WavError::Io(format!("{}: {e}", path.display())))?)
}

fn header(out: &mut Vec<u8>, tag: u16, channels: u16, rate: u32, bits: u16, data_len: u32) {
    let block = channels * bits / 8;
    let fmt_len = if tag == 3 { 18u32 } else { 16 };
    let riff_len = 4 + (8 + fmt_len) + (if tag == 3 { 12 } else { 0 }) + 8 + data_len + (data_len & 1);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&fmt_len.to_le_bytes());
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    if tag == 3 {
        out.extend_from_slice(&0u16.to_le_bytes()); // cbSize
        // float WAVs need a fact chunk
        out.extend_from_slice(b"fact");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&(data_len / u32::from(block)).to_le_bytes());
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
}

/// Encodes interleaved samples as 16-bit PCM (clipped, rounded to nearest).
pub fn encode_pcm16(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    header(&mut out, 1, channels, sample_rate, 16, data_len);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Encodes interleaved samples as 32-bit float.
pub fn encode_f32(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    let data_len = (samples.len() * 4) as u32;
    let mut out = Vec::with_capacity(58 + samples.len() * 4);
    header(&mut out, 3, channels, sample_rate, 32, data_len);
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Writes a PCM16 WAV file.
pub fn write_pcm16(path: &Path, samples: &[f32], sample_rate: u32, channels: u16) -> Result<(), WavError> {
    std::fs::write(path, encode_pcm16(samples, sample_rate, channels)).map_err(|e| WavError::Io(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| ((i as f32) / n as f32) * 1.8 - 0.9).collect()
    }

    #[test]
    fn pcm16_roundtrip_within_quantisation() {
        let s = ramp(1000);
        let w = parse(&encode_pcm16(&s, 16000, 1)).unwrap();
        assert_eq!((w.sample_rate, w.channels), (16000, 1));
        assert_eq!(w.samples.len(), 1000);
        for (a, b) in s.iter().zip(&w.samples) {
            assert!((a - b).abs() <= 2.0 / 32768.0, "{a} vs {b}");
        }
        assert!((w.duration_s() - 1000.0 / 16000.0).abs() < 1e-12);
    }

    #[test]
    fn float32_roundtrip_is_exact() {
        let s = ramp(333);
        let w = parse(&encode_f32(&s, 24000, 1)).unwrap();
        assert_eq!(w.samples, s);
        assert_eq!(w.sample_rate, 24000);
    }

    #[test]
    fn clipping_and_rounding() {
        let w = parse(&encode_pcm16(&[2.0, -2.0, 0.0], 8000, 1)).unwrap();
        assert!((w.samples[0] - 32767.0 / 32768.0).abs() < 1e-6);
        assert!((w.samples[1] + 32767.0 / 32768.0).abs() < 1e-6);
        assert_eq!(w.samples[2], 0.0);
    }

    #[test]
    fn stereo_to_mono_and_frames() {
        let inter = [0.5f32, -0.5, 1.0, 0.0, 0.25, 0.25];
        let w = parse(&encode_f32(&inter, 48000, 2)).unwrap();
        assert_eq!(w.channels, 2);
        assert_eq!(w.frames(), 3);
        assert_eq!(w.to_mono(), vec![0.0, 0.5, 0.25]);
    }

    #[test]
    fn extra_chunks_and_odd_padding_are_skipped() {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        v.extend_from_slice(b"LIST");
        v.extend_from_slice(&3u32.to_le_bytes());
        v.extend_from_slice(&[1, 2, 3, 0]); // 3 bytes + pad
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&[1, 0, 1, 0]);
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&[2, 0, 16, 0]);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&[0, 64, 0, 192]);
        let w = parse(&v).unwrap();
        assert_eq!(w.samples, vec![0.5, -0.5]);
    }

    #[test]
    fn extensible_float_header() {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&0xFFFEu16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&44100u32.to_le_bytes());
        v.extend_from_slice(&(44100u32 * 4).to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        v.extend_from_slice(&32u16.to_le_bytes()); // valid bits
        v.extend_from_slice(&4u32.to_le_bytes()); // channel mask
        v.extend_from_slice(&3u16.to_le_bytes()); // subformat: IEEE float
        v.extend_from_slice(&[0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&8u32.to_le_bytes());
        v.extend_from_slice(&0.25f32.to_le_bytes());
        v.extend_from_slice(&(-1.0f32).to_le_bytes());
        let w = parse(&v).unwrap();
        assert_eq!((w.sample_rate, w.channels), (44100, 1));
        assert_eq!(w.samples, vec![0.25, -1.0]);
    }

    #[test]
    fn pcm24_and_pcm32_and_pcm8() {
        let mk = |bits: u16, data: &[u8]| {
            let mut v = Vec::new();
            v.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
            v.extend_from_slice(&16u32.to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&1u16.to_le_bytes());
            v.extend_from_slice(&8000u32.to_le_bytes());
            v.extend_from_slice(&(8000 * u32::from(bits) / 8).to_le_bytes());
            v.extend_from_slice(&(bits / 8).to_le_bytes());
            v.extend_from_slice(&bits.to_le_bytes());
            v.extend_from_slice(b"data");
            v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            v.extend_from_slice(data);
            v
        };
        let w24 = parse(&mk(24, &[0, 0, 64, 0, 0, 192])).unwrap();
        assert_eq!(w24.samples, vec![0.5, -0.5]);
        let w32 = parse(&mk(32, &[0, 0, 0, 64])).unwrap();
        assert_eq!(w32.samples, vec![0.5]);
        let w8 = parse(&mk(8, &[128, 192, 0])).unwrap();
        assert_eq!(w8.samples, vec![0.0, 0.5, -1.0]);
    }

    #[test]
    fn streaming_and_truncated_data_chunk() {
        let mut v = encode_pcm16(&[0.5, -0.5, 0.25, 0.0], 16000, 1);
        // announce more data than present (a file cut short) and a half sample at the end
        v[40..44].copy_from_slice(&1000u32.to_le_bytes());
        v.push(1);
        let w = parse(&v).unwrap();
        assert_eq!(w.samples.len(), 4);
        // streaming writers put 0xFFFFFFFF
        let mut s = encode_pcm16(&[0.5, -0.5], 16000, 1);
        s[40..44].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(parse(&s).unwrap().samples.len(), 2);
    }

    #[test]
    fn errors() {
        assert_eq!(parse(b"nope"), Err(WavError::NotRiff));
        assert_eq!(parse(b"RIFF\0\0\0\0WAVE"), Err(WavError::NoFmt));
        let mut adpcm = encode_pcm16(&[0.0], 8000, 1);
        adpcm[20] = 2;
        assert_eq!(parse(&adpcm), Err(WavError::Unsupported(2, 16)));
        let mut zero_rate = encode_pcm16(&[0.0], 8000, 1);
        zero_rate[24..28].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(parse(&zero_rate), Err(WavError::BadHeader(_))));
        let mut nodata = encode_pcm16(&[], 8000, 1);
        nodata.truncate(36);
        assert_eq!(parse(&nodata), Err(WavError::NoData));
        assert!(read(Path::new("/nonexistent/x.wav")).is_err());
    }

    #[test]
    fn file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pimax-vb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        write_pcm16(&p, &[0.1, 0.2, -0.3], 22050, 1).unwrap();
        let w = read(&p).unwrap();
        assert_eq!(w.sample_rate, 22050);
        assert_eq!(w.samples.len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn header_sizes_are_consistent() {
        let b = encode_pcm16(&[0.0; 10], 8000, 1);
        assert_eq!(b.len(), 44 + 20);
        assert_eq!(u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize, b.len() - 8);
        let f = encode_f32(&[0.0; 10], 8000, 1);
        assert_eq!(u32::from_le_bytes([f[4], f[5], f[6], f[7]]) as usize, f.len() - 8);
    }
}
