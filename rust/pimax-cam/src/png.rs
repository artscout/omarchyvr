//! Tiny PNG encoder (8-bit grey or RGB, "Sub" filter, zlib via miniz_oxide) and binary PGM/PPM writers.

use std::io;
use std::path::Path;

fn crc32(data: &[u8], mut crc: u32) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
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
    });
    crc = !crc;
    for &b in data {
        crc = t[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(data, crc32(kind, 0));
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode `pixels` (rows of `width * channels` bytes, `channels` 1 = grey or 3 = RGB) as PNG.
pub fn encode(width: usize, height: usize, channels: usize, pixels: &[u8]) -> Vec<u8> {
    assert!(channels == 1 || channels == 3);
    let rowlen = width * channels;
    assert!(pixels.len() >= rowlen * height);
    let mut filtered = Vec::with_capacity((rowlen + 1) * height);
    for y in 0..height {
        let row = &pixels[y * rowlen..(y + 1) * rowlen];
        filtered.push(1u8); // Sub
        for x in 0..rowlen {
            let left = if x >= channels { row[x - channels] } else { 0 };
            filtered.push(row[x].wrapping_sub(left));
        }
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if channels == 1 { 0 } else { 2 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(
        &mut out,
        b"IDAT",
        &miniz_oxide::deflate::compress_to_vec_zlib(&filtered, 6),
    );
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Write a grey PNG.
pub fn write_grey(path: &Path, width: usize, height: usize, pixels: &[u8]) -> io::Result<()> {
    std::fs::write(path, encode(width, height, 1, pixels))
}

/// Write an RGB PNG.
pub fn write_rgb(path: &Path, width: usize, height: usize, pixels: &[u8]) -> io::Result<()> {
    std::fs::write(path, encode(width, height, 3, pixels))
}

/// Write a binary PGM (P5).
pub fn write_pgm(path: &Path, width: usize, height: usize, pixels: &[u8]) -> io::Result<()> {
    let mut v = format!("P5\n{width} {height}\n255\n").into_bytes();
    v.extend_from_slice(&pixels[..width * height]);
    std::fs::write(path, v)
}

/// Linear contrast stretch so that the 1st..99th percentile maps to 0..255 (for viewing dark frames).
pub fn stretch(pixels: &[u8]) -> Vec<u8> {
    let mut hist = [0u64; 256];
    for &p in pixels {
        hist[p as usize] += 1;
    }
    let n = pixels.len() as u64;
    let pct = |q: f64| {
        let target = (n as f64 * q) as u64;
        let mut acc = 0;
        for (v, &h) in hist.iter().enumerate() {
            acc += h;
            if acc > target {
                return v as f64;
            }
        }
        255.0
    };
    let (lo, hi) = (pct(0.01), pct(0.99).max(pct(0.01) + 1.0));
    pixels
        .iter()
        .map(|&p| (((p as f64 - lo) * 255.0 / (hi - lo)).clamp(0.0, 255.0)) as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"123456789", 0), 0xcbf4_3926);
        assert_eq!(crc32(b"IEND", 0), 0xae42_6082);
    }

    #[test]
    fn png_structure_and_roundtrip() {
        let (w, h) = (17usize, 5usize);
        let px: Vec<u8> = (0..w * h).map(|i| (i * 7) as u8).collect();
        let png = encode(w, h, 1, &px);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(
            u32::from_be_bytes(png[16..20].try_into().unwrap()),
            w as u32
        );
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // decode IDAT and undo the Sub filter
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&png[41..41 + idat_len]).unwrap();
        assert_eq!(raw.len(), (w + 1) * h);
        for y in 0..h {
            let row = &raw[y * (w + 1)..(y + 1) * (w + 1)];
            assert_eq!(row[0], 1);
            let mut prev = 0u8;
            for x in 0..w {
                let v = row[1 + x].wrapping_add(prev);
                assert_eq!(v, px[y * w + x]);
                prev = v;
            }
        }
        // chunk CRCs are valid
        let ihdr_crc = u32::from_be_bytes(png[29..33].try_into().unwrap());
        assert_eq!(ihdr_crc, crc32(&png[16..29], crc32(b"IHDR", 0)));
    }

    #[test]
    fn stretch_expands_range() {
        let px: Vec<u8> = (0..1000).map(|i| 10 + (i % 20) as u8).collect();
        let s = stretch(&px);
        assert_eq!(*s.iter().min().unwrap(), 0);
        assert!(*s.iter().max().unwrap() >= 250);
    }
}
