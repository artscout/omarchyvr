//! Minimal PNG encoder (8-bit RGB, filter type 0) on top of `miniz_oxide`, no other dependencies.

/// CRC-32 (IEEE 802.3) as used by PNG chunks.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = Vec::with_capacity(4 + data.len());
    body.extend_from_slice(kind);
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// Encodes `rgb` (`w*h*3` bytes, row-major) as a PNG file image.
///
/// # Panics
/// Panics if `rgb.len() != w * h * 3` or a dimension is zero.
pub fn encode_rgb8(w: u32, h: u32, rgb: &[u8]) -> Vec<u8> {
    assert!(w > 0 && h > 0, "empty image");
    assert_eq!(rgb.len(), w as usize * h as usize * 3, "pixel buffer size");
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for row in rgb.chunks_exact(w as usize * 3) {
        raw.push(0); // filter type: none
        raw.extend_from_slice(row);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = Vec::with_capacity(z.len() + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolor, deflate, no filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Writes an RGB8 PNG to `path`.
pub fn write_rgb8(path: &std::path::Path, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, encode_rgb8(w, h, rgb))
}

/// Decodes a PNG produced by [`encode_rgb8`] (8-bit RGB, filter 0, one IDAT) back to `(w, h, rgb)`.
/// Used by the tests; not a general PNG reader.
pub fn decode_rgb8(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if png.len() < 8 || png[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return Err("bad signature".into());
    }
    let mut pos = 8;
    let (mut w, mut h) = (0u32, 0u32);
    let mut idat = Vec::new();
    while pos + 8 <= png.len() {
        let len = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = png[pos + 4..pos + 8].try_into().unwrap();
        let body = png.get(pos + 8..pos + 8 + len).ok_or("truncated chunk")?;
        let crc = u32::from_be_bytes(png.get(pos + 8 + len..pos + 12 + len).ok_or("no crc")?.try_into().unwrap());
        let mut crc_in = kind.to_vec();
        crc_in.extend_from_slice(body);
        if crc32(&crc_in) != crc {
            return Err(format!("crc mismatch in {}", String::from_utf8_lossy(&kind)));
        }
        match &kind {
            b"IHDR" => {
                w = u32::from_be_bytes(body[0..4].try_into().unwrap());
                h = u32::from_be_bytes(body[4..8].try_into().unwrap());
                if body[8..13] != [8, 2, 0, 0, 0] {
                    return Err("unsupported IHDR".into());
                }
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat).map_err(|e| format!("inflate: {e:?}"))?;
    let stride = w as usize * 3 + 1;
    if raw.len() != stride * h as usize {
        return Err("size mismatch".into());
    }
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for row in raw.chunks_exact(stride) {
        if row[0] != 0 {
            return Err("filter type not 0".into());
        }
        rgb.extend_from_slice(&row[1..]);
    }
    Ok((w, h, rgb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b"IEND"), 0xae42_6082); // the constant CRC of an empty IEND chunk
    }

    #[test]
    fn roundtrip_gradient() {
        let (w, h) = (37u32, 19u32);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(&[(x * 7) as u8, (y * 13) as u8, (x ^ y) as u8]);
            }
        }
        let png = encode_rgb8(w, h, &rgb);
        let (w2, h2, back) = decode_rgb8(&png).unwrap();
        assert_eq!((w2, h2), (w, h));
        assert_eq!(back, rgb);
    }

    #[test]
    fn corrupt_crc_is_detected() {
        let mut png = encode_rgb8(2, 2, &[0u8; 12]);
        let n = png.len();
        png[n - 20] ^= 0xff; // inside IDAT data or its CRC
        assert!(decode_rgb8(&png).is_err());
        assert!(decode_rgb8(&[1, 2, 3]).is_err());
    }

    #[test]
    #[should_panic(expected = "pixel buffer size")]
    fn wrong_buffer_size_panics() {
        let _ = encode_rgb8(2, 2, &[0u8; 11]);
    }
}
