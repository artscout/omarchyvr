//! Minimal PNG decoder for 8-bit, non-interlaced grey / grey+alpha / RGB / RGBA files (the tiles and golden
//! images this crate reads). Colour is converted to grey (Rec. 601 luma). Encoding uses `pimax_cam::png`.

use std::path::Path;

/// A decoded grey image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grey {
    /// Width.
    pub width: usize,
    /// Height.
    pub height: usize,
    /// Row-major pixels.
    pub pixels: Vec<u8>,
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a16, b16, c16) = (a as i16, b as i16, c as i16);
    let p = a16 + b16 - c16;
    let (pa, pb, pc) = ((p - a16).abs(), (p - b16).abs(), (p - c16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Decode PNG bytes to grey.
pub fn decode(data: &[u8]) -> Result<Grey, String> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("not a PNG".into());
    }
    let mut pos = 8;
    let (mut w, mut h, mut ch) = (0usize, 0usize, 0usize);
    let mut idat = Vec::new();
    while pos + 8 <= data.len() {
        let len = be32(&data[pos..]) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data
            .get(pos + 8..pos + 8 + len)
            .ok_or("truncated PNG chunk")?;
        match kind {
            b"IHDR" => {
                if body.len() < 13 {
                    return Err("short IHDR".into());
                }
                w = be32(body) as usize;
                h = be32(&body[4..]) as usize;
                let (depth, ctype, interlace) = (body[8], body[9], body[12]);
                ch = match ctype {
                    0 => 1,
                    4 => 2,
                    2 => 3,
                    6 => 4,
                    _ => return Err(format!("PNG colour type {ctype} not supported")),
                };
                if depth != 8 || interlace != 0 {
                    return Err(format!(
                        "PNG depth {depth} / interlace {interlace} not supported"
                    ));
                }
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    if w == 0 || h == 0 || w > 1 << 15 || h > 1 << 15 {
        return Err("PNG without a sane IHDR".into());
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat)
        .map_err(|e| format!("PNG inflate: {e:?}"))?;
    let stride = w * ch;
    if raw.len() < (stride + 1) * h {
        return Err("PNG data too short".into());
    }
    let mut img = vec![0u8; stride * h];
    for y in 0..h {
        let f = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (done, rest) = img.split_at_mut(y * stride);
        let prev: &[u8] = if y > 0 {
            &done[(y - 1) * stride..]
        } else {
            &[]
        };
        let cur = &mut rest[..stride];
        for x in 0..stride {
            let a = if x >= ch { cur[x - ch] } else { 0 };
            let b = if y > 0 { prev[x] } else { 0 };
            let c = if y > 0 && x >= ch { prev[x - ch] } else { 0 };
            let v = src[x];
            cur[x] = match f {
                0 => v,
                1 => v.wrapping_add(a),
                2 => v.wrapping_add(b),
                3 => v.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => v.wrapping_add(paeth(a, b, c)),
                _ => return Err(format!("PNG filter {f}")),
            };
        }
    }
    let pixels = match ch {
        1 => img,
        2 => img.iter().step_by(2).copied().collect(),
        _ => img
            .chunks_exact(ch)
            .map(|p| {
                ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114 + 500) / 1000) as u8
            })
            .collect(),
    };
    Ok(Grey {
        width: w,
        height: h,
        pixels,
    })
}

/// Read and decode a PNG file.
pub fn read(path: &Path) -> Result<Grey, String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode(&d).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_the_encoder() {
        let w = 37;
        let h = 11;
        let g: Vec<u8> = (0..w * h).map(|i| (i * 7 % 251) as u8).collect();
        let d = decode(&pimax_cam::png::encode(w, h, 1, &g)).unwrap();
        assert_eq!((d.width, d.height), (w, h));
        assert_eq!(d.pixels, g);
        let rgb: Vec<u8> = g.iter().flat_map(|&v| [v, v, v]).collect();
        let d = decode(&pimax_cam::png::encode(w, h, 3, &rgb)).unwrap();
        assert_eq!(d.pixels, g);
        assert!(decode(b"nope").is_err());
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn decodes_the_real_tiles() {
        // written by another encoder (all filter types may occur)
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/morning2_tile1.png");
        let g = read(&p).unwrap();
        assert_eq!((g.width, g.height), (640, 480));
        let mean = g.pixels.iter().map(|&v| v as f64).sum::<f64>() / g.pixels.len() as f64;
        assert!(mean > 60.0 && mean < 250.0, "{mean}");
    }
}
