//! Decoding of the compositor's final-output dumps (`XRT_COMPOSITOR_DUMP_DIR`, files named
//! `frame<N>_<W>x<H>_fmt<vkformat>.raw`, tightly packed rows) into small PNGs: box-filter downscale in code space,
//! then each eye is turned back to the orientation the user sees (the compositor pre-rotates the eye images for the
//! 90 degree mounted panels, `PIMAX_ROT_LEFT` / `PIMAX_ROT_RIGHT`).

use crate::image::{default_threads, Image};
use std::path::Path;
use std::thread;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DumpInfo {
    pub frame: u32,
    pub w: u32,
    pub h: u32,
    pub vk_format: u32,
}

/// `frame300_7104x3840_fmt64.raw` -> frame 300, 7104 x 3840, VkFormat 64.
pub fn parse_dump_name(name: &str) -> Option<DumpInfo> {
    // also accepts a prefix: `01-grid_frame300_7104x3840_fmt64.raw` (a dump kept by `--keep-raw`)
    let stem = name[name.rfind("frame")? + "frame".len()..].strip_suffix(".raw")?;
    let (frame, rest) = stem.split_once('_')?;
    let (dims, fmt) = rest.split_once("_fmt")?;
    let (w, h) = dims.split_once('x')?;
    Some(DumpInfo {
        frame: frame.parse().ok()?,
        w: w.parse().ok()?,
        h: h.parse().ok()?,
        vk_format: fmt.parse().ok()?,
    })
}

const A2B10G10R10: u32 = 64;
const A2R10G10B10: u32 = 58;
const RGBA8: [u32; 2] = [37, 43];
const BGRA8: [u32; 2] = [44, 50];

/// All supported dump formats have 4 bytes per pixel.
pub fn supported(vk_format: u32) -> bool {
    vk_format == A2B10G10R10
        || vk_format == A2R10G10B10
        || RGBA8.contains(&vk_format)
        || BGRA8.contains(&vk_format)
}

/// One pixel (4 bytes) as 10-bit R, G, B.
pub fn unpack10(vk_format: u32, px: &[u8]) -> [u16; 3] {
    let v = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
    let up = |b: u8| (u16::from(b) << 2) | (u16::from(b) >> 6);
    match vk_format {
        A2B10G10R10 => [
            (v & 1023) as u16,
            ((v >> 10) & 1023) as u16,
            ((v >> 20) & 1023) as u16,
        ],
        A2R10G10B10 => [
            ((v >> 20) & 1023) as u16,
            ((v >> 10) & 1023) as u16,
            (v & 1023) as u16,
        ],
        f if BGRA8.contains(&f) => [up(px[2]), up(px[1]), up(px[0])],
        _ => [up(px[0]), up(px[1]), up(px[2])],
    }
}

/// Box-filter downscale of a dump by `factor` (output `w/factor` x `h/factor`, partial blocks dropped).
pub fn downscale(
    raw: &[u8],
    w: u32,
    h: u32,
    vk_format: u32,
    factor: u32,
    threads: usize,
) -> Result<Image, String> {
    if !supported(vk_format) {
        return Err(format!(
            "dump format {vk_format} is not supported (need 4 byte RGB formats)"
        ));
    }
    if factor == 0 || w / factor == 0 || h / factor == 0 {
        return Err(format!("bad downscale factor {factor} for {w}x{h}"));
    }
    if raw.len() < w as usize * h as usize * 4 {
        return Err(format!(
            "dump has {} bytes, {w}x{h} needs {}",
            raw.len(),
            w as usize * h as usize * 4
        ));
    }
    let (ow, oh) = (w / factor, h / factor);
    let mut out = vec![0u8; ow as usize * oh as usize * 3];
    let row_job = |oy: usize, row: &mut [u8]| {
        let mut sums = vec![[0u32; 3]; ow as usize];
        for sy in 0..factor as usize {
            let y = oy * factor as usize + sy;
            let line = &raw[y * w as usize * 4..(y + 1) * w as usize * 4];
            for (ox, s) in sums.iter_mut().enumerate() {
                for sx in 0..factor as usize {
                    let x = ox * factor as usize + sx;
                    let p = unpack10(vk_format, &line[x * 4..x * 4 + 4]);
                    s[0] += u32::from(p[0]);
                    s[1] += u32::from(p[1]);
                    s[2] += u32::from(p[2]);
                }
            }
        }
        let n = factor * factor;
        for (ox, s) in sums.iter().enumerate() {
            for c in 0..3 {
                row[ox * 3 + c] = (((s[c] + n / 2) / n + 2) >> 2).min(255) as u8;
            }
        }
    };
    let stride = ow as usize * 3;
    if threads <= 1 {
        for (oy, row) in out.chunks_mut(stride).enumerate() {
            row_job(oy, row);
        }
    } else {
        let rows_per = (oh as usize).div_ceil(threads);
        thread::scope(|s| {
            for (i, chunk) in out.chunks_mut(rows_per * stride).enumerate() {
                let row_job = &row_job;
                s.spawn(move || {
                    for (r, row) in chunk.chunks_mut(stride).enumerate() {
                        row_job(i * rows_per + r, row);
                    }
                });
            }
        });
    }
    Ok(Image {
        w: ow,
        h: oh,
        rgb: out,
    })
}

/// Rotate clockwise by `quarters` x 90 degrees.
pub fn rotate_cw(img: &Image, quarters: u32) -> Image {
    let mut cur = img.clone();
    for _ in 0..quarters % 4 {
        let (w, h) = (cur.w, cur.h);
        let mut out = Image {
            w: h,
            h: w,
            rgb: vec![0; cur.rgb.len()],
        };
        for y in 0..h {
            for x in 0..w {
                // (x, y) of the source goes to (h - 1 - y, x)
                let c = cur.get(x, y);
                out.set((h - 1 - y) as i32, x as i32, c);
            }
        }
        cur = out;
    }
    cur
}

/// Cut a (downscaled) dump into the two eye halves, undo the compositor's pre-rotation (`rot_left` / `rot_right` in
/// degrees clockwise, as `PIMAX_ROT_*`) and place the eyes side by side with `gap` pixels between them.
pub fn compose(img: &Image, rot_left: u32, rot_right: u32, gap: u32) -> Image {
    let half = img.w / 2;
    let cut = |x0: u32| {
        let mut e = Image::new(half, img.h, [0.0; 3]);
        for y in 0..img.h {
            for x in 0..half {
                e.set(x as i32, y as i32, img.get(x0 + x, y));
            }
        }
        e
    };
    let undo = |deg: u32| (4 - (deg / 90) % 4) % 4;
    let l = rotate_cw(&cut(0), undo(rot_left));
    let r = rotate_cw(&cut(half), undo(rot_right));
    let (w, h) = (l.w + gap + r.w, l.h.max(r.h));
    let mut out = Image::new(w, h, [0.12, 0.12, 0.12]);
    for (e, x0) in [(&l, 0), (&r, l.w + gap)] {
        for y in 0..e.h {
            for x in 0..e.w {
                out.set((x0 + x) as i32, y as i32, e.get(x, y));
            }
        }
    }
    out
}

/// FNV-1a 64 bit hash of the bytes: identical dumps have identical checksums.
pub fn checksum(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// What [`convert`] found out about a dump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Converted {
    pub info: DumpInfo,
    /// [`checksum`] of the raw file.
    pub checksum: u64,
}

/// Read a dump, downscale it, undo the pre-rotation and write a PNG. Returns what was in the file name and the
/// checksum of the raw bytes.
pub fn convert(
    raw: &Path,
    png: &Path,
    factor: u32,
    rot_left: u32,
    rot_right: u32,
) -> Result<Converted, String> {
    let name = raw.file_name().and_then(|n| n.to_str()).ok_or("bad path")?;
    let info = parse_dump_name(name)
        .ok_or_else(|| format!("{name}: not a frame<N>_<W>x<H>_fmt<F>.raw file name"))?;
    let data = std::fs::read(raw).map_err(|e| format!("{}: {e}", raw.display()))?;
    let sum = checksum(&data);
    let small = downscale(
        &data,
        info.w,
        info.h,
        info.vk_format,
        factor,
        default_threads(),
    )?;
    let img = compose(&small, rot_left, rot_right, (8 / factor).max(2));
    pimax_lens::png::write_rgb8(png, img.w, img.h, &img.rgb)
        .map_err(|e| format!("{}: {e}", png.display()))?;
    Ok(Converted {
        info,
        checksum: sum,
    })
}

/// Result of comparing two dumps pixel by pixel (differences in 10-bit code values).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffStats {
    pub w: u32,
    pub h: u32,
    pub total: u64,
    pub differing: u64,
    /// Largest difference of any channel, 10-bit units.
    pub max_diff: u16,
    pub sum_abs: u64,
    pub first: Option<(u32, u32)>,
    /// Bounding box of the differing pixels (x0, y0, x1, y1).
    pub bbox: Option<(u32, u32, u32, u32)>,
}

impl DiffStats {
    pub fn identical(&self) -> bool {
        self.differing == 0
    }
}

/// Compare two raw dumps of the same size and format.
pub fn diff(
    a: &[u8],
    b: &[u8],
    w: u32,
    h: u32,
    vk_format: u32,
) -> Result<(DiffStats, Vec<u16>), String> {
    if !supported(vk_format) {
        return Err(format!("dump format {vk_format} is not supported"));
    }
    let n = w as usize * h as usize * 4;
    if a.len() < n || b.len() < n {
        return Err(format!(
            "dumps are {} and {} bytes, {w}x{h} needs {n}",
            a.len(),
            b.len()
        ));
    }
    let mut st = DiffStats {
        w,
        h,
        total: u64::from(w) * u64::from(h),
        differing: 0,
        max_diff: 0,
        sum_abs: 0,
        first: None,
        bbox: None,
    };
    let mut map = vec![0u16; w as usize * h as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y as usize * w as usize + x as usize) * 4;
            if a[i..i + 4] == b[i..i + 4] {
                continue;
            }
            let (pa, pb) = (
                unpack10(vk_format, &a[i..i + 4]),
                unpack10(vk_format, &b[i..i + 4]),
            );
            let d = (0..3).map(|c| pa[c].abs_diff(pb[c])).max().unwrap_or(0);
            if d == 0 {
                continue; // only the alpha bits differ
            }
            st.differing += 1;
            st.sum_abs += (0..3)
                .map(|c| u64::from(pa[c].abs_diff(pb[c])))
                .sum::<u64>();
            st.max_diff = st.max_diff.max(d);
            st.first.get_or_insert((x, y));
            st.bbox = Some(match st.bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
            map[y as usize * w as usize + x as usize] = d;
        }
    }
    Ok((st, map))
}

/// A picture of a difference map: each output pixel is the largest difference in its `factor` x `factor` block,
/// amplified (x16, so a 1/1023 step is visible) and drawn white on black.
pub fn diff_image(map: &[u16], w: u32, h: u32, factor: u32) -> Image {
    let (ow, oh) = ((w / factor.max(1)).max(1), (h / factor.max(1)).max(1));
    let mut img = Image::new(ow, oh, [0.0; 3]);
    for oy in 0..oh {
        for ox in 0..ow {
            let mut m = 0u16;
            for sy in 0..factor {
                for sx in 0..factor {
                    let (x, y) = (ox * factor + sx, oy * factor + sy);
                    if x < w && y < h {
                        m = m.max(map[y as usize * w as usize + x as usize]);
                    }
                }
            }
            let v = (u32::from(m) * 16).min(255) as u8;
            img.set(ox as i32, oy as i32, [v, v, v]);
        }
    }
    img
}

/// Compare two dump files; optionally write the difference picture (orientation as stored, not rotated back).
pub fn diff_files(
    a: &Path,
    b: &Path,
    png: Option<&Path>,
    factor: u32,
) -> Result<DiffStats, String> {
    let name = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .ok_or_else(|| "bad path".to_string())
    };
    let (ia, ib) = (
        parse_dump_name(&name(a)?).ok_or("first file is not a frame*.raw dump")?,
        parse_dump_name(&name(b)?).ok_or("second file is not a frame*.raw dump")?,
    );
    if (ia.w, ia.h, ia.vk_format) != (ib.w, ib.h, ib.vk_format) {
        return Err(format!(
            "different dumps: {}x{} fmt{} vs {}x{} fmt{}",
            ia.w, ia.h, ia.vk_format, ib.w, ib.h, ib.vk_format
        ));
    }
    let da = std::fs::read(a).map_err(|e| format!("{}: {e}", a.display()))?;
    let db = std::fs::read(b).map_err(|e| format!("{}: {e}", b.display()))?;
    let (st, map) = diff(&da, &db, ia.w, ia.h, ia.vk_format)?;
    if let Some(p) = png {
        let img = diff_image(&map, ia.w, ia.h, factor.max(1));
        pimax_lens::png::write_rgb8(p, img.w, img.h, &img.rgb)
            .map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            parse_dump_name("frame300_7104x3840_fmt64.raw"),
            Some(DumpInfo {
                frame: 300,
                w: 7104,
                h: 3840,
                vk_format: 64
            })
        );
        assert_eq!(parse_dump_name("frame0_8x2_fmt44.raw").unwrap().w, 8);
        assert_eq!(
            parse_dump_name("01-grid_frame60_7104x3840_fmt64.raw"),
            Some(DumpInfo {
                frame: 60,
                w: 7104,
                h: 3840,
                vk_format: 64
            })
        );
        for bad in [
            "frame_7104x3840_fmt64.raw",
            "frame3_7104x3840.raw",
            "x.raw",
            "frame3_ax4_fmt1.raw",
            "frame3_8x4_fmt1.png",
            "",
        ] {
            assert_eq!(parse_dump_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn unpack_formats() {
        // A2B10G10R10: R=1023, G=512, B=0, A=3
        let v: u32 = 1023 | (512 << 10) | (3 << 30);
        assert_eq!(unpack10(64, &v.to_le_bytes()), [1023, 512, 0]);
        // A2R10G10B10 with the same bits: B=1023 R=0
        assert_eq!(unpack10(58, &v.to_le_bytes()), [0, 512, 1023]);
        // 8 bit: 255 -> 1023, 0 -> 0, 128 -> 514
        assert_eq!(unpack10(37, &[255, 128, 0, 255]), [1023, 514, 0]);
        assert_eq!(unpack10(43, &[255, 128, 0, 255]), [1023, 514, 0]);
        assert_eq!(unpack10(44, &[0, 128, 255, 255]), [1023, 514, 0]);
        assert_eq!(unpack10(50, &[0, 128, 255, 255]), [1023, 514, 0]);
        assert!(supported(64) && supported(44) && !supported(97));
    }

    #[test]
    fn downscale_averages_blocks() {
        // 4 x 2 BGRA: left block (2x2) has values 0,100,200,100 in R ; right block all 255
        let mut raw = Vec::new();
        for (y, row) in [[0u8, 100, 255, 255], [200, 100, 255, 255]]
            .iter()
            .enumerate()
        {
            let _ = y;
            for &r in row {
                raw.extend_from_slice(&[0, 0, r, 255]); // B G R A
            }
        }
        let img = downscale(&raw, 4, 2, 44, 2, 1).unwrap();
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(img.get(0, 0), [100, 0, 0]);
        assert_eq!(img.get(1, 0), [255, 0, 0]);
        // threads do not change the result
        let big: Vec<u8> = (0..64 * 48 * 4).map(|i| ((i * 31) % 251) as u8).collect();
        let a = downscale(&big, 64, 48, 37, 4, 1).unwrap();
        for t in [2, 5, 16] {
            assert_eq!(downscale(&big, 64, 48, 37, 4, t).unwrap(), a);
        }
        // 10 bit full scale stays 255
        let v: u32 = 1023 | (1023 << 10) | (1023 << 20);
        let white: Vec<u8> = (0..16).flat_map(|_| v.to_le_bytes()).collect();
        assert_eq!(
            downscale(&white, 4, 4, 64, 2, 1).unwrap().get(1, 1),
            [255, 255, 255]
        );
    }

    #[test]
    fn downscale_errors() {
        assert!(downscale(&[0; 64], 4, 4, 97, 1, 1).is_err());
        assert!(downscale(&[0; 64], 4, 4, 44, 0, 1).is_err());
        assert!(downscale(&[0; 64], 4, 4, 44, 8, 1).is_err());
        assert!(downscale(&[0; 10], 4, 4, 44, 1, 1).is_err());
    }

    fn sample(w: u32, h: u32) -> Image {
        let mut im = Image::new(w, h, [0.0; 3]);
        for y in 0..h {
            for x in 0..w {
                im.set(
                    x as i32,
                    y as i32,
                    [(x * 20 + 1) as u8, (y * 20 + 1) as u8, 7],
                );
            }
        }
        im
    }

    #[test]
    fn rotate_cw_moves_the_top_left_corner_to_the_top_right() {
        let im = sample(3, 2);
        let r = rotate_cw(&im, 1);
        assert_eq!((r.w, r.h), (2, 3));
        assert_eq!(r.get(1, 0), im.get(0, 0)); // top-left -> top-right
        assert_eq!(r.get(1, 2), im.get(2, 0)); // top-right -> bottom-right
        assert_eq!(r.get(0, 0), im.get(0, 1)); // bottom-left -> top-left
        assert_eq!(rotate_cw(&im, 4), im);
        assert_eq!(rotate_cw(&rotate_cw(&im, 1), 3), im);
        assert_eq!(rotate_cw(&im, 2).get(2, 1), im.get(0, 0));
        assert_eq!(rotate_cw(&im, 0), im);
    }

    #[test]
    fn compose_restores_what_the_user_sees() {
        // the user-facing eye images; the compositor pre-rotates left by 90 cw and right by 270 cw
        let (ul, ur) = (sample(4, 3), {
            let mut r = sample(4, 3);
            r.set(0, 0, [250, 0, 0]);
            r
        });
        let (dl, dr) = (rotate_cw(&ul, 1), rotate_cw(&ur, 3));
        let mut dump = Image::new(dl.w + dr.w, dl.h, [0.0; 3]);
        for y in 0..dl.h {
            for x in 0..dl.w {
                dump.set(x as i32, y as i32, dl.get(x, y));
                dump.set((dl.w + x) as i32, y as i32, dr.get(x, y));
            }
        }
        let out = compose(&dump, 90, 270, 2);
        assert_eq!((out.w, out.h), (4 + 2 + 4, 3));
        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(out.get(x, y), ul.get(x, y), "left {x},{y}");
                assert_eq!(out.get(6 + x, y), ur.get(x, y), "right {x},{y}");
            }
        }
        assert_eq!(out.get(5, 1), [31, 31, 31]); // the gap
    }

    #[test]
    fn checksum_detects_a_single_bit() {
        let a = vec![7u8; 1000];
        let mut b = a.clone();
        assert_eq!(checksum(&a), checksum(&b));
        b[500] ^= 1;
        assert_ne!(checksum(&a), checksum(&b));
        assert_eq!(checksum(b""), 0xcbf2_9ce4_8422_2325);
    }

    fn bgra(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                v.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        v
    }

    #[test]
    fn diff_of_identical_and_slightly_different_dumps() {
        let a = bgra(8, 6, |x, y| [(x * 30) as u8, (y * 40) as u8, 9]);
        let (st, map) = diff(&a, &a, 8, 6, 44).unwrap();
        assert!(
            st.identical() && st.max_diff == 0 && st.bbox.is_none() && map.iter().all(|&d| d == 0)
        );
        assert_eq!(st.total, 48);
        // two pixels differ: one by 1 code in blue, one by 10 in red
        let mut b = a.clone();
        b[(2 * 8 + 3) * 4] += 1;
        b[(4 * 8 + 6) * 4 + 2] += 10;
        let (st, map) = diff(&a, &b, 8, 6, 44).unwrap();
        assert_eq!(st.differing, 2);
        assert_eq!(st.max_diff, 40); // 10 codes of 8 bit = 40 in 10-bit units
        assert_eq!(st.first, Some((3, 2)));
        assert_eq!(st.bbox, Some((3, 2, 6, 4)));
        assert_eq!(map[2 * 8 + 3], 4); // one 8-bit code = 4 in 10-bit units
                                       // alpha-only differences do not count
        let mut c = a.clone();
        c[3] = 0;
        assert!(diff(&a, &c, 8, 6, 44).unwrap().0.identical());
        assert!(diff(&a, &a[..10], 8, 6, 44).is_err());
        assert!(diff(&a, &a, 8, 6, 97).is_err());
    }

    #[test]
    fn diff_image_marks_blocks_with_differences() {
        let mut map = vec![0u16; 8 * 8];
        map[5 * 8 + 6] = 3;
        let img = diff_image(&map, 8, 8, 4);
        assert_eq!((img.w, img.h), (2, 2));
        assert_eq!(img.get(1, 1), [48, 48, 48]);
        assert_eq!(img.get(0, 0), [0, 0, 0]);
        let big = diff_image(&[1000], 1, 1, 1);
        assert_eq!(big.get(0, 0), [255, 255, 255]);
    }

    #[test]
    fn diff_files_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lenstest-diff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = bgra(16, 8, |x, _| [x as u8, 0, 0]);
        let mut b = a.clone();
        b[20 * 4 + 1] = 5;
        let (fa, fb, fc) = (
            dir.join("frame1_16x8_fmt44.raw"),
            dir.join("frame2_16x8_fmt44.raw"),
            dir.join("frame3_8x16_fmt44.raw"),
        );
        std::fs::write(&fa, &a).unwrap();
        std::fs::write(&fb, &b).unwrap();
        std::fs::write(&fc, &a).unwrap();
        let png = dir.join("d.png");
        let st = diff_files(&fa, &fb, Some(&png), 2).unwrap();
        assert_eq!(st.differing, 1);
        assert!(png.exists());
        assert!(diff_files(&fa, &fa, None, 1).unwrap().identical());
        assert!(diff_files(&fa, &fc, None, 1)
            .unwrap_err()
            .contains("different dumps"));
        assert!(diff_files(&fa, &dir.join("x.raw"), None, 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn convert_writes_a_png_of_the_expected_size() {
        let dir = std::env::temp_dir().join(format!("lenstest-dump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 16 x 8 dump (two 8 x 8 eyes), BGRA, left half red, right half blue
        let mut raw = Vec::new();
        for _ in 0..8 {
            for x in 0..16 {
                raw.extend_from_slice(if x < 8 {
                    &[0, 0, 255, 255]
                } else {
                    &[255, 0, 0, 255]
                });
            }
        }
        let f = dir.join("frame7_16x8_fmt44.raw");
        std::fs::write(&f, &raw).unwrap();
        let out = dir.join("o.png");
        let conv = convert(&f, &out, 2, 90, 270).unwrap();
        assert_eq!(
            conv.info,
            DumpInfo {
                frame: 7,
                w: 16,
                h: 8,
                vk_format: 44
            }
        );
        assert_eq!(conv.checksum, checksum(&raw));
        let d = pimax_lens::png::decode(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!((d.width, d.height, d.channels), (4 + 4 + 4, 4, 3));
        assert_eq!(&d.data[0..3], &[255, 0, 0]);
        assert!(convert(&dir.join("nope.raw"), &out, 2, 90, 270).is_err());
        assert!(convert(&out, &out, 2, 90, 270).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
