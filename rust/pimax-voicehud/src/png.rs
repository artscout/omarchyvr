//! A tiny PNG writer (8-bit RGBA, no interlacing) for the offline `preview`, with a decoder for the tests.
//! Rows are filtered with None, Sub or Up, whichever has the smallest sum of absolute values.

use std::io;
use std::path::Path;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (n, e) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *e = c;
    }
    t
}

/// CRC-32 as used by PNG chunks.
pub fn crc32(data: &[u8]) -> u32 {
    let t = crc_table();
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c = t[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode `rgba` (`width * height * 4` bytes, straight alpha, rows top to bottom) as a PNG file image.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let channels = 4;
    assert_eq!(rgba.len(), w * h * channels, "pixel buffer size");
    let stride = w * channels;
    let zero = vec![0u8; stride];
    let mut raw = Vec::with_capacity((stride + 1) * h);
    let mut cand = [vec![0u8; stride], vec![0u8; stride], vec![0u8; stride]];
    for y in 0..h {
        let row = &rgba[y * stride..(y + 1) * stride];
        let prev = if y > 0 {
            &rgba[(y - 1) * stride..y * stride]
        } else {
            &zero[..]
        };
        for i in 0..stride {
            let left = if i >= channels { row[i - channels] } else { 0 };
            cand[0][i] = row[i];
            cand[1][i] = row[i].wrapping_sub(left);
            cand[2][i] = row[i].wrapping_sub(prev[i]);
        }
        let best = (0..3)
            .min_by_key(|&f| {
                cand[f]
                    .iter()
                    .map(|&b| u64::from((b as i8).unsigned_abs()))
                    .sum::<u64>()
            })
            .unwrap_or(0);
        raw.push(best as u8);
        raw.extend_from_slice(&cand[best]);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // 8 bits, colour type 6 (RGBA), deflate, adaptive filtering, no interlace
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn write_rgba(path: &Path, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    std::fs::write(path, encode_rgba(width, height, rgba))
}

/// Decoded RGBA image (only what this writer produces). For tests and tools.
#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode_rgba(png: &[u8]) -> Result<Decoded, String> {
    if png.len() < 8 || png[..8] != SIGNATURE {
        return Err("not a PNG".into());
    }
    let mut pos = 8;
    let (mut width, mut height) = (0u32, 0u32);
    let mut idat = Vec::new();
    while pos + 12 <= png.len() {
        let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = &png[pos + 4..pos + 8];
        let body = png.get(pos + 8..pos + 8 + len).ok_or("truncated chunk")?;
        let crc = u32::from_be_bytes(png[pos + 8 + len..pos + 12 + len].try_into().unwrap());
        if crc != crc32(&png[pos + 4..pos + 8 + len]) {
            return Err(format!("bad CRC in {}", String::from_utf8_lossy(kind)));
        }
        match kind {
            b"IHDR" => {
                width = u32::from_be_bytes(body[0..4].try_into().unwrap());
                height = u32::from_be_bytes(body[4..8].try_into().unwrap());
                if body[8] != 8 || body[9] != 6 || body[12] != 0 {
                    return Err("only 8-bit non-interlaced RGBA".into());
                }
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat)
        .map_err(|e| format!("inflate: {e:?}"))?;
    let stride = width as usize * 4;
    if raw.len() != (stride + 1) * height as usize {
        return Err("size mismatch".into());
    }
    let mut rgba = vec![0u8; stride * height as usize];
    for y in 0..height as usize {
        let f = raw[y * (stride + 1)];
        for i in 0..stride {
            let v = raw[y * (stride + 1) + 1 + i];
            let left = if i >= 4 { rgba[y * stride + i - 4] } else { 0 };
            let up = if y > 0 { rgba[(y - 1) * stride + i] } else { 0 };
            rgba[y * stride + i] = match f {
                0 => v,
                1 => v.wrapping_add(left),
                2 => v.wrapping_add(up),
                _ => return Err(format!("filter {f}")),
            };
        }
    }
    Ok(Decoded {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
    }

    #[test]
    fn roundtrip() {
        let (w, h) = (37u32, 19u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[
                    (x * 7) as u8,
                    (y * 13) as u8,
                    ((x ^ y) * 5) as u8,
                    if x > 20 { 255 } else { (x * 12) as u8 },
                ]);
            }
        }
        let png = encode_rgba(w, h, &px);
        assert_eq!(&png[..8], &SIGNATURE);
        let d = decode_rgba(&png).unwrap();
        assert_eq!((d.width, d.height), (w, h));
        assert_eq!(d.rgba, px);
    }

    #[test]
    fn transparent_image_is_small_and_decodes() {
        let png = encode_rgba(1600, 400, &vec![0u8; 1600 * 400 * 4]);
        assert!(png.len() < 20_000, "{}", png.len());
        assert_eq!(decode_rgba(&png).unwrap().rgba.len(), 1600 * 400 * 4);
    }

    #[test]
    fn decode_rejects_damage() {
        let mut png = encode_rgba(2, 2, &[7u8; 16]);
        assert!(decode_rgba(&png[..20]).is_err());
        let n = png.len();
        png[n - 20] ^= 0xff; // inside IDAT or its CRC
        assert!(decode_rgba(&png).is_err());
        assert!(decode_rgba(b"hello").is_err());
    }

    #[test]
    #[should_panic(expected = "pixel buffer size")]
    fn size_is_checked() {
        encode_rgba(2, 2, &[0; 15]);
    }
}
