//! Minimal PNG reader for the 8-bit greyscale frames of EuRoC recordings (non-interlaced; colour type 0, or 2/6
//! converted to grey). Enough for the datasets Monado's recorder and `pimax-slam smoke-euroc` write.

use std::path::Path;

/// A decoded grey image.
#[derive(Clone, Debug, PartialEq)]
pub struct Grey {
    /// Width.
    pub w: usize,
    /// Height.
    pub h: usize,
    /// Pixels, row-major, `w` bytes per row.
    pub data: Vec<u8>,
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (a as i16, b as i16, c as i16);
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Decode PNG bytes.
pub fn decode(b: &[u8]) -> Result<Grey, String> {
    if b.len() < 8 || &b[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("not a PNG".into());
    }
    let (mut i, mut idat) = (8usize, Vec::new());
    let (mut w, mut h, mut depth, mut ctype, mut interlace) = (0usize, 0usize, 0u8, 0u8, 0u8);
    while i + 8 <= b.len() {
        let len = u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) as usize;
        let kind = &b[i + 4..i + 8];
        let body = b.get(i + 8..i + 8 + len).ok_or("truncated PNG chunk")?;
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(body[0..4].try_into().unwrap()) as usize;
                h = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                depth = body[8];
                ctype = body[9];
                interlace = body[12];
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        i += 12 + len;
    }
    if depth != 8 || interlace != 0 || !matches!(ctype, 0 | 2 | 6) || w == 0 || h == 0 {
        return Err(format!(
            "unsupported PNG (depth {depth}, colour type {ctype}, interlace {interlace})"
        ));
    }
    let bpp = match ctype {
        0 => 1,
        2 => 3,
        _ => 4,
    };
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat)
        .map_err(|e| format!("PNG inflate: {e:?}"))?;
    let stride = w * bpp;
    if raw.len() < h * (stride + 1) {
        return Err("PNG data too short".into());
    }
    let mut prev = vec![0u8; stride];
    let mut cur = vec![0u8; stride];
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        let f = row[0];
        for x in 0..stride {
            let a = if x >= bpp { cur[x - bpp] } else { 0 };
            let up = prev[x];
            let c = if x >= bpp { prev[x - bpp] } else { 0 };
            let v = row[1 + x];
            cur[x] = match f {
                0 => v,
                1 => v.wrapping_add(a),
                2 => v.wrapping_add(up),
                3 => v.wrapping_add(((a as u16 + up as u16) / 2) as u8),
                4 => v.wrapping_add(paeth(a, up, c)),
                _ => return Err(format!("bad PNG filter {f}")),
            };
        }
        if bpp == 1 {
            out.extend_from_slice(&cur);
        } else {
            out.extend(
                cur.chunks(bpp).map(|p| {
                    ((p[0] as u16 * 77 + p[1] as u16 * 150 + p[2] as u16 * 29) >> 8) as u8
                }),
            );
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    Ok(Grey { w, h, data: out })
}

/// Read a PNG file.
pub fn read(path: &Path) -> Result<Grey, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode(&b).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_the_repo_encoder() {
        let (w, h) = (37, 11);
        let px: Vec<u8> = (0..w * h).map(|i| ((i * 7) % 251) as u8).collect();
        let png = pimax_cam::png::encode(w, h, 1, &px);
        let g = decode(&png).unwrap();
        assert_eq!((g.w, g.h), (w, h));
        assert_eq!(g.data, px);
    }
}
