//! `pimax vr dump-stats FILE...`: numbers (and an optional preview) for the raw target images that Monado writes with
//! `XRT_COMPOSITOR_DUMP_DIR` (`frame<N>_<W>x<H>_fmt<vkformat>.raw`, the compositor output right before it is handed
//! to the display). Replaces eyeballing: "is it black?", "is the left eye different from the right one?", "how bright
//! is the background?". Values are reported on a 0-255 scale whatever the pixel format, so 10-bit and 8-bit targets
//! can be compared with each other.
use crate::CliError;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;

/// Pixel layouts the compositor can pick as target format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixFmt {
    /// `VK_FORMAT_A2B10G10R10_UNORM_PACK32` (64): R in bits 0-9, G 10-19, B 20-29, A 30-31.
    A2B10G10R10,
    /// 8 bit, memory order R G B A (`R8G8B8A8_UNORM` 37 / `_SRGB` 43).
    Rgba8,
    /// 8 bit, memory order B G R A (`B8G8R8A8_UNORM` 44 / `_SRGB` 50).
    Bgra8,
    /// `A8B8G8R8_*_PACK32` (51 UNORM, 57 SRGB): same memory order as `Rgba8` on little endian.
    A8B8G8R8,
}

impl PixFmt {
    /// Map a Vulkan format number from the file name.
    pub fn from_vk(fmt: u32) -> Option<Self> {
        Some(match fmt {
            64 => PixFmt::A2B10G10R10,
            37 | 43 => PixFmt::Rgba8,
            44 | 50 => PixFmt::Bgra8,
            51 | 57 => PixFmt::A8B8G8R8,
            _ => return None,
        })
    }

    /// Human readable name.
    pub fn name(self) -> &'static str {
        match self {
            PixFmt::A2B10G10R10 => "A2B10G10R10 (10 bit)",
            PixFmt::Rgba8 => "R8G8B8A8 (8 bit)",
            PixFmt::Bgra8 => "B8G8R8A8 (8 bit)",
            PixFmt::A8B8G8R8 => "A8B8G8R8 (8 bit)",
        }
    }

    /// RGB of one 4 byte pixel on a 0..255 scale.
    pub fn rgb(self, p: [u8; 4]) -> [f32; 3] {
        match self {
            PixFmt::A2B10G10R10 => {
                let v = u32::from_le_bytes(p);
                let c = |s: u32| ((v >> s) & 0x3ff) as f32 * (255.0 / 1023.0);
                [c(0), c(10), c(20)]
            }
            PixFmt::Rgba8 | PixFmt::A8B8G8R8 => [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])],
            PixFmt::Bgra8 => [f32::from(p[2]), f32::from(p[1]), f32::from(p[0])],
        }
    }
}

/// What the file name says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DumpMeta {
    /// Compositor frame number.
    pub frame: u32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
    /// Vulkan format number.
    pub fmt: u32,
}

/// Parse `frame300_7104x3840_fmt64.raw` (any directory in front).
pub fn parse_name(path: &Path) -> Option<DumpMeta> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("frame")?.strip_suffix(".raw")?;
    let mut it = rest.split('_');
    let frame = it.next()?.parse().ok()?;
    let (w, h) = it.next()?.split_once('x')?;
    let fmt = it.next()?.strip_prefix("fmt")?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(DumpMeta {
        frame,
        w: w.parse().ok()?,
        h: h.parse().ok()?,
        fmt,
    })
}

/// Mean of a region, how many pixels were black, and a histogram of the colours (eighth steps of the 0..255 scale, so
/// neighbouring 10 bit values stay distinguishable) to find the dominant ones of flat test patterns.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Region {
    sum: [f64; 3],
    n: u64,
    black: u64,
    colours: HashMap<[u16; 3], u64>,
}

/// Stop adding new histogram entries beyond this many distinct colours (smooth gradients would use all memory).
const MAX_DISTINCT_COLOURS: usize = 200_000;

impl Region {
    fn add(&mut self, rgb: [f32; 3]) {
        for (s, c) in self.sum.iter_mut().zip(rgb) {
            *s += f64::from(c);
        }
        self.n += 1;
        if rgb.iter().all(|c| *c < 2.0) {
            self.black += 1;
        }
        let key = rgb.map(|c| (c * 8.0).round().clamp(0.0, 2040.0) as u16);
        if let Some(n) = self.colours.get_mut(&key) {
            *n += 1;
        } else if self.colours.len() < MAX_DISTINCT_COLOURS {
            self.colours.insert(key, 1);
        }
    }

    /// The `n` most frequent colours (0..255 scale) with their share of the region in percent.
    pub fn dominant(&self, n: usize) -> Vec<([f32; 3], f64)> {
        let mut v: Vec<(&[u16; 3], &u64)> = self.colours.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter()
            .take(n)
            .map(|(k, c)| {
                (
                    k.map(|x| f32::from(x) / 8.0),
                    100.0 * *c as f64 / self.n.max(1) as f64,
                )
            })
            .collect()
    }

    /// Mean RGB (0..255).
    pub fn mean(&self) -> [f64; 3] {
        let n = self.n.max(1) as f64;
        [self.sum[0] / n, self.sum[1] / n, self.sum[2] / n]
    }

    /// Percentage of pixels with all channels below 2/255.
    pub fn black_percent(&self) -> f64 {
        100.0 * self.black as f64 / self.n.max(1) as f64
    }
}

/// Result of analysing one dump.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// From the file name.
    pub meta: DumpMeta,
    /// Pixel layout.
    pub fmt: PixFmt,
    /// Whole image.
    pub all: Region,
    /// Left half (left eye).
    pub left: Region,
    /// Right half (right eye).
    pub right: Region,
    /// Requested probe pixels: ((x, y), rgb 0..255).
    pub probes: Vec<((u32, u32), [f32; 3])>,
}

/// Analyse raw pixel data of `meta.w * meta.h * 4` bytes. Every `step`-th pixel in x and y is sampled.
pub fn analyse(
    meta: DumpMeta,
    data: &mut dyn Read,
    step: u32,
    probes: &[(u32, u32)],
) -> Result<Stats, CliError> {
    let fmt = PixFmt::from_vk(meta.fmt).ok_or_else(|| {
        CliError::new(format!(
            "unsupported Vulkan format {} in the file name",
            meta.fmt
        ))
    })?;
    let step = step.max(1);
    let mut st = Stats {
        meta,
        fmt,
        all: Region::default(),
        left: Region::default(),
        right: Region::default(),
        probes: vec![],
    };
    let mut row = vec![0u8; meta.w as usize * 4];
    for y in 0..meta.h {
        data.read_exact(&mut row)
            .map_err(|e| CliError::new(format!("short file at row {y}: {e}")))?;
        let sample_row = y % step == 0;
        for (px, py) in probes {
            if *py == y && *px < meta.w {
                let i = *px as usize * 4;
                st.probes.push((
                    (*px, *py),
                    fmt.rgb([row[i], row[i + 1], row[i + 2], row[i + 3]]),
                ));
            }
        }
        if !sample_row {
            continue;
        }
        for x in (0..meta.w).step_by(step as usize) {
            let i = x as usize * 4;
            let rgb = fmt.rgb([row[i], row[i + 1], row[i + 2], row[i + 3]]);
            st.all.add(rgb);
            if x < meta.w / 2 {
                st.left.add(rgb);
            } else {
                st.right.add(rgb);
            }
        }
    }
    Ok(st)
}

/// Write a P6 preview, every `step`-th pixel, 8 bit.
pub fn write_ppm(
    meta: DumpMeta,
    data: &mut dyn Read,
    step: u32,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    let fmt = PixFmt::from_vk(meta.fmt)
        .ok_or_else(|| CliError::new(format!("unsupported Vulkan format {}", meta.fmt)))?;
    let step = step.max(1);
    let (ow, oh) = (meta.w.div_ceil(step), meta.h.div_ceil(step));
    writeln!(out, "P6 {ow} {oh} 255")?;
    let mut row = vec![0u8; meta.w as usize * 4];
    let mut line = Vec::with_capacity(ow as usize * 3);
    for y in 0..meta.h {
        data.read_exact(&mut row)
            .map_err(|e| CliError::new(format!("short file at row {y}: {e}")))?;
        if y % step != 0 {
            continue;
        }
        line.clear();
        for x in (0..meta.w).step_by(step as usize) {
            let i = x as usize * 4;
            let rgb = fmt.rgb([row[i], row[i + 1], row[i + 2], row[i + 3]]);
            line.extend(rgb.iter().map(|c| c.round().clamp(0.0, 255.0) as u8));
        }
        out.write_all(&line)?;
    }
    Ok(())
}

/// Result of comparing two dumps of the same size and format.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiffStats {
    /// Pixels compared.
    pub pixels: u64,
    /// Pixels whose raw value differs.
    pub differing: u64,
    /// Differing pixels none of whose 4-neighbours differs from them in the first image: flat-area differences, which
    /// head-pose noise cannot explain (it only moves edges).
    pub interior: u64,
    /// Differing pixels per largest channel difference on the 0..255 scale: up to 0.5, 2, 8, more.
    pub buckets: [u64; 4],
    /// Largest channel difference seen (0..255 scale).
    pub max_diff: f32,
    /// First interior difference: (x, y, a, b).
    pub first_interior: Option<(u32, u32, [f32; 3], [f32; 3])>,
}

/// Compare two dumps pixel by pixel.
pub fn diff(meta: DumpMeta, a: &mut dyn Read, b: &mut dyn Read) -> Result<DiffStats, CliError> {
    let fmt = PixFmt::from_vk(meta.fmt)
        .ok_or_else(|| CliError::new(format!("unsupported Vulkan format {}", meta.fmt)))?;
    let w = meta.w as usize;
    let mut st = DiffStats::default();
    if meta.w == 0 || meta.h == 0 {
        return Ok(st);
    }
    // rows of `a` (raw), rows of `b`, and the differing flags of the row above the newest one
    let mut ra: [Vec<u8>; 3] = [vec![0; w * 4], vec![0; w * 4], vec![0; w * 4]];
    let mut rb: [Vec<u8>; 3] = [vec![0; w * 4], vec![0; w * 4], vec![0; w * 4]];
    // Process row y-1 once row y has been read, so that its upper and lower neighbours are available.
    let judge = |y: u32,
                 up: Option<&[u8]>,
                 cur: &[u8],
                 down: Option<&[u8]>,
                 curb: &[u8],
                 st: &mut DiffStats| {
        for x in 0..w {
            let i = x * 4;
            st.pixels += 1;
            // compare the decoded colour: the alpha / padding byte of the target is not part of the picture
            let rgb_at = |o: &[u8], j: usize| fmt.rgb([o[j], o[j + 1], o[j + 2], o[j + 3]]);
            let ca = rgb_at(cur, i);
            let cb = rgb_at(curb, i);
            if ca == cb {
                continue;
            }
            st.differing += 1;
            let m = (0..3).map(|k| (ca[k] - cb[k]).abs()).fold(0.0f32, f32::max);
            st.max_diff = st.max_diff.max(m);
            let bucket = if m <= 0.5 {
                0
            } else if m <= 2.0 {
                1
            } else if m <= 8.0 {
                2
            } else {
                3
            };
            st.buckets[bucket] += 1;
            let same = |o: &[u8], j: usize| rgb_at(o, j) == ca;
            let on_edge = (x > 0 && !same(cur, i - 4))
                || (x + 1 < w && !same(cur, i + 4))
                || up.is_some_and(|u| !same(u, i))
                || down.is_some_and(|d| !same(d, i));
            if !on_edge {
                st.interior += 1;
                if st.first_interior.is_none() {
                    st.first_interior = Some((x as u32, y, ca, cb));
                }
            }
        }
    };
    for y in 0..meta.h as usize {
        let slot = y % 3;
        a.read_exact(&mut ra[slot])
            .map_err(|e| CliError::new(format!("first file: short at row {y}: {e}")))?;
        b.read_exact(&mut rb[slot])
            .map_err(|e| CliError::new(format!("second file: short at row {y}: {e}")))?;
        if y >= 1 {
            let (pu, pc, pd) = ((y + 1) % 3, (y - 1) % 3, y % 3);
            let up = (y >= 2).then(|| ra[pu].as_slice());
            judge((y - 1) as u32, up, &ra[pc], Some(&ra[pd]), &rb[pc], &mut st);
        }
    }
    let last = meta.h as usize - 1;
    let up = (last >= 1).then(|| ra[(last + 2) % 3].as_slice());
    judge(last as u32, up, &ra[last % 3], None, &rb[last % 3], &mut st);
    Ok(st)
}

/// Text report of a comparison.
pub fn diff_report(a: &Path, b: &Path, s: &DiffStats) -> Vec<String> {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut v = vec![format!("{} vs {}", name(a), name(b))];
    v.push(format!(
        "  differing pixels {} of {} ({:.4} %), largest channel difference {:.2} (0-255 scale)",
        s.differing,
        s.pixels,
        100.0 * s.differing as f64 / s.pixels.max(1) as f64,
        s.max_diff
    ));
    v.push(format!(
        "  by size: <=0.5 {}, <=2 {}, <=8 {}, >8 {}",
        s.buckets[0], s.buckets[1], s.buckets[2], s.buckets[3]
    ));
    v.push(format!(
        "  in flat areas (no neighbour of the first image differs): {}{}",
        s.interior,
        match &s.first_interior {
            Some((x, y, ca, cb)) => format!(
                "  first at ({x},{y}): {:.2},{:.2},{:.2} vs {:.2},{:.2},{:.2}",
                ca[0], ca[1], ca[2], cb[0], cb[1], cb[2]
            ),
            None => String::new(),
        }
    ));
    v
}

/// A dump holds exactly `w * h * 4` bytes; anything else is a damaged or mislabelled file.
pub fn check_len(path: &Path, meta: &DumpMeta) -> Result<(), CliError> {
    let want = u64::from(meta.w) * u64::from(meta.h) * 4;
    let got = std::fs::metadata(path)
        .map_err(|e| CliError::new(format!("{}: {e}", path.display())))?
        .len();
    if got != want {
        return Err(CliError::new(format!(
            "{}: {got} bytes, but {}x{} at 4 bytes per pixel is {want} bytes (damaged or still being written?)",
            path.display(),
            meta.w,
            meta.h
        )));
    }
    Ok(())
}

fn fmt_rgb(m: [f64; 3]) -> String {
    format!("{:6.1} {:6.1} {:6.1}", m[0], m[1], m[2])
}

/// Text report for one file.
pub fn report(path: &Path, s: &Stats) -> Vec<String> {
    let mut v = vec![format!(
        "{}: frame {}, {}x{}, {}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        s.meta.frame,
        s.meta.w,
        s.meta.h,
        s.fmt.name()
    )];
    v.push(format!(
        "  mean RGB (0-255)  all   {}",
        fmt_rgb(s.all.mean())
    ));
    v.push(format!(
        "                    left  {}",
        fmt_rgb(s.left.mean())
    ));
    v.push(format!(
        "                    right {}",
        fmt_rgb(s.right.mean())
    ));
    v.push(format!(
        "  black pixels      all {:.1} %, left {:.1} %, right {:.1} %{}",
        s.all.black_percent(),
        s.left.black_percent(),
        s.right.black_percent(),
        if s.all.black_percent() > 99.0 {
            "  <-- BLACK"
        } else {
            ""
        }
    ));
    for (name, r) in [("left ", &s.left), ("right", &s.right)] {
        let d: Vec<String> = r
            .dominant(3)
            .iter()
            .map(|(c, p)| format!("({:.2}, {:.2}, {:.2}) {p:.1} %", c[0], c[1], c[2]))
            .collect();
        v.push(format!("  dominant colours  {name} {}", d.join("; ")));
    }
    for ((x, y), c) in &s.probes {
        v.push(format!(
            "  pixel ({x},{y})     {:6.1} {:6.1} {:6.1}   = #{:02x}{:02x}{:02x}",
            c[0],
            c[1],
            c[2],
            c[0].round() as u8,
            c[1].round() as u8,
            c[2].round() as u8
        ));
    }
    v
}

/// Entry point of `vr dump-diff A.raw B.raw`.
pub fn run_diff(args: &mut crate::args::Args) -> Result<(), CliError> {
    args.no_unknown_flags()?;
    if args.0.len() != 2 {
        return Err(CliError::usage("vr dump-diff A.raw B.raw"));
    }
    let (pa, pb) = (Path::new(&args.0[0]), Path::new(&args.0[1]));
    let meta = |p: &Path| {
        parse_name(p).ok_or_else(|| {
            CliError::new(format!(
                "{}: not named frame<N>_<W>x<H>_fmt<F>.raw",
                p.display()
            ))
        })
    };
    let (ma, mb) = (meta(pa)?, meta(pb)?);
    check_len(pa, &ma)?;
    check_len(pb, &mb)?;
    if (ma.w, ma.h, ma.fmt) != (mb.w, mb.h, mb.fmt) {
        return Err(CliError::new(format!(
            "the dumps differ in size or format: {}x{} fmt{} vs {}x{} fmt{}",
            ma.w, ma.h, ma.fmt, mb.w, mb.h, mb.fmt
        )));
    }
    let open = |p: &Path| {
        File::open(p)
            .map(BufReader::new)
            .map_err(|e| CliError::new(format!("{}: {e}", p.display())))
    };
    let st = diff(ma, &mut open(pa)?, &mut open(pb)?)?;
    for l in diff_report(pa, pb, &st) {
        vout!("{l}");
    }
    Ok(())
}

/// Entry point of `vr dump-stats`: `files... [--step N] [--probe X,Y]... [--ppm OUT]`.
pub fn run(args: &mut crate::args::Args) -> Result<(), CliError> {
    let step: u32 = args.opt_parse("--step")?.unwrap_or(4);
    let ppm = args.opt("--ppm")?;
    let mut probes = Vec::new();
    while let Some(p) = args.opt("--probe")? {
        let (x, y) = p
            .split_once(',')
            .ok_or_else(|| CliError::usage("--probe wants X,Y"))?;
        probes.push((
            x.trim()
                .parse()
                .map_err(|_| CliError::usage("--probe wants X,Y"))?,
            y.trim()
                .parse()
                .map_err(|_| CliError::usage("--probe wants X,Y"))?,
        ));
    }
    args.no_unknown_flags()?;
    if args.0.is_empty() {
        return Err(CliError::usage(
            "vr dump-stats FILE.raw... [--step N] [--probe X,Y]... [--ppm OUT.ppm] (one file with --ppm)",
        ));
    }
    if ppm.is_some() && args.0.len() != 1 {
        return Err(CliError::usage("--ppm works on exactly one file"));
    }
    for f in &args.0 {
        let path = Path::new(f);
        let meta = parse_name(path).ok_or_else(|| {
            CliError::new(format!(
                "{f}: not named frame<N>_<W>x<H>_fmt<F>.raw (Monado dump)"
            ))
        })?;
        let open = || {
            File::open(path)
                .map(BufReader::new)
                .map_err(|e| CliError::new(format!("{f}: {e}")))
        };
        check_len(path, &meta)?;
        let st = analyse(meta, &mut open()?, step, &probes)?;
        for l in report(path, &st) {
            vout!("{l}");
        }
        if let Some(out) = &ppm {
            let mut w = std::io::BufWriter::new(
                File::create(out).map_err(|e| CliError::new(format!("{out}: {e}")))?,
            );
            write_ppm(meta, &mut open()?, step.max(8), &mut w)?;
            w.flush()?;
            vout!("  preview written to {out} (convert: magick {out} preview.png)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn px10(r: u32, g: u32, b: u32) -> [u8; 4] {
        (r | g << 10 | b << 20 | 3 << 30).to_le_bytes()
    }

    #[test]
    fn file_names() {
        let m = parse_name(Path::new("/tmp/d/frame300_7104x3840_fmt64.raw")).unwrap();
        assert_eq!(
            m,
            DumpMeta {
                frame: 300,
                w: 7104,
                h: 3840,
                fmt: 64
            }
        );
        for bad in [
            "frame300_7104x3840.raw",
            "frame_x_fmt64.raw",
            "frame1_2x3_fmt4.raw.bak",
            "other.raw",
            "frame1_2x3_fmt4_extra.raw",
        ] {
            assert!(parse_name(Path::new(bad)).is_none(), "{bad}");
        }
    }

    #[test]
    fn pixel_formats() {
        // 10 bit: R=1023 -> 255
        let c = PixFmt::A2B10G10R10.rgb(px10(1023, 512, 0));
        assert!((c[0] - 255.0).abs() < 1e-3 && (c[1] - 127.6).abs() < 0.2 && c[2] == 0.0);
        // 8 bit memory orders
        assert_eq!(PixFmt::Rgba8.rgb([1, 2, 3, 255]), [1.0, 2.0, 3.0]);
        assert_eq!(PixFmt::Bgra8.rgb([1, 2, 3, 255]), [3.0, 2.0, 1.0]);
        assert_eq!(PixFmt::A8B8G8R8.rgb([1, 2, 3, 255]), [1.0, 2.0, 3.0]);
        assert_eq!(PixFmt::from_vk(64), Some(PixFmt::A2B10G10R10));
        assert_eq!(PixFmt::from_vk(57), Some(PixFmt::A8B8G8R8));
        assert_eq!(PixFmt::from_vk(44), Some(PixFmt::Bgra8));
        assert_eq!(PixFmt::from_vk(123), None);
    }

    /// 8x4 image: left half dark blue (0,32,128 in 8 bit scale), right half dark red, one black column pair.
    fn image() -> (DumpMeta, Vec<u8>) {
        let (w, h) = (8u32, 4u32);
        let meta = DumpMeta {
            frame: 1,
            w,
            h,
            fmt: 64,
        };
        let mut d = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                let p = match x {
                    0 => px10(0, 0, 0),
                    x if x < 4 => px10(0, 128, 512),
                    _ => px10(512, 128, 128),
                };
                d.extend_from_slice(&p);
            }
        }
        (meta, d)
    }

    #[test]
    fn statistics_split_the_eyes_and_count_black() {
        let (meta, d) = image();
        let s = analyse(meta, &mut Cursor::new(d), 1, &[(1, 2), (7, 3), (99, 0)]).unwrap();
        assert_eq!(s.all.n, 32);
        assert_eq!(s.left.n, 16);
        assert!(
            (s.all.black_percent() - 12.5).abs() < 1e-9,
            "one column of eight is black"
        );
        assert!((s.left.black_percent() - 25.0).abs() < 1e-9);
        assert_eq!(s.right.black_percent(), 0.0);
        let lm = s.left.mean();
        let rm = s.right.mean();
        assert!(lm[2] > lm[0], "left eye is blue: {lm:?}");
        assert!(rm[0] > rm[2], "right eye is red: {rm:?}");
        assert_eq!(s.probes.len(), 2, "out-of-range probe ignored");
        assert!((s.probes[0].1[2] - 512.0 * 255.0 / 1023.0).abs() < 1e-3);
        let text = report(Path::new("frame1_8x4_fmt64.raw"), &s).join("\n");
        assert!(text.contains("A2B10G10R10"), "{text}");
        assert!(!text.contains("BLACK"), "{text}");
    }

    #[test]
    fn dominant_colours_find_the_flat_background() {
        let (meta, d) = image();
        let s = analyse(meta, &mut Cursor::new(d), 1, &[]).unwrap();
        let left = s.left.dominant(2);
        // left half: 3 of 4 columns are (0, 128/4, 512/4) in 10 bit, one is black
        assert_eq!(left.len(), 2);
        assert!((left[0].1 - 75.0).abs() < 1e-9, "{left:?}");
        assert!(
            (left[0].0[2] - 127.62).abs() < 0.2 && left[0].0[0] == 0.0,
            "{left:?}"
        );
        assert!(
            (left[1].1 - 25.0).abs() < 1e-9 && left[1].0 == [0.0, 0.0, 0.0],
            "{left:?}"
        );
        let right = s.right.dominant(1);
        assert!(
            (right[0].1 - 100.0).abs() < 1e-9 && right[0].0[0] > 127.0,
            "{right:?}"
        );
        let text = report(Path::new("frame1_8x4_fmt64.raw"), &s).join("\n");
        assert!(
            text.contains(
                "dominant colours  left  (0.00, 31.88, 127.62) 75.0 %; (0.00, 0.00, 0.00) 25.0 %"
            ),
            "{text}"
        );
    }

    #[test]
    fn histogram_is_bounded_and_ties_are_deterministic() {
        let mut r = Region::default();
        // 300 000 distinct colours: only the first MAX_DISTINCT_COLOURS are tracked, the count still adds up
        for i in 0..300_000u32 {
            r.add([
                (i % 256) as f32,
                ((i / 256) % 256) as f32,
                (i / 65_536) as f32,
            ]);
        }
        assert_eq!(r.n, 300_000);
        assert!(r.colours.len() <= MAX_DISTINCT_COLOURS);
        let mut t = Region::default();
        t.add([10.0, 0.0, 0.0]);
        t.add([20.0, 0.0, 0.0]);
        assert_eq!(
            t.dominant(2)[0].0,
            [10.0, 0.0, 0.0],
            "ties resolve to the smaller colour"
        );
    }

    fn flat_image(w: u32, h: u32) -> (DumpMeta, Vec<u8>) {
        let meta = DumpMeta {
            frame: 1,
            w,
            h,
            fmt: 64,
        };
        let mut d = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                // left half colour A, right half colour B: a vertical edge in the middle
                d.extend_from_slice(&if x < w / 2 {
                    px10(100, 200, 300)
                } else {
                    px10(500, 600, 700)
                });
            }
        }
        (meta, d)
    }

    #[test]
    fn diff_of_identical_dumps_is_empty() {
        let (meta, d) = flat_image(8, 6);
        let s = diff(meta, &mut Cursor::new(d.clone()), &mut Cursor::new(d)).unwrap();
        assert_eq!((s.pixels, s.differing, s.interior), (48, 0, 0));
        assert_eq!(s.max_diff, 0.0);
    }

    #[test]
    fn a_shifted_edge_differs_only_on_the_edge_but_a_changed_flat_pixel_is_interior() {
        let (meta, a) = flat_image(8, 6);
        // b: the edge moved one pixel to the right on rows 2..4 (pixel x=4 keeps colour A)
        let mut b = a.clone();
        for y in 2..4usize {
            let i = (y * 8 + 4) * 4;
            b[i..i + 4].copy_from_slice(&px10(100, 200, 300));
        }
        let s = diff(
            meta,
            &mut Cursor::new(a.clone()),
            &mut Cursor::new(b.clone()),
        )
        .unwrap();
        assert_eq!(s.differing, 2);
        assert_eq!(s.interior, 0, "edge pixels are not flat-area differences");
        assert!(s.max_diff > 8.0 && s.buckets[3] == 2);
        // now additionally change one pixel deep inside the left flat area by 1 code
        let i = (3 * 8 + 1) * 4;
        b[i..i + 4].copy_from_slice(&px10(101, 200, 300));
        let s = diff(meta, &mut Cursor::new(a), &mut Cursor::new(b)).unwrap();
        assert_eq!((s.differing, s.interior), (3, 1));
        assert_eq!(
            s.buckets[0], 1,
            "a one code (10 bit) step is in the smallest bucket"
        );
        let (x, y, ca, cb) = s.first_interior.unwrap();
        assert_eq!((x, y), (1, 3));
        assert!(cb[0] > ca[0]);
        let text = diff_report(Path::new("a.raw"), Path::new("b.raw"), &s).join("\n");
        assert!(text.contains("differing pixels 3 of 48"), "{text}");
        assert!(text.contains("first at (1,3)"), "{text}");
    }

    #[test]
    fn an_alpha_only_change_is_not_a_picture_difference_and_empty_images_do_not_panic() {
        let (meta, a) = flat_image(8, 6);
        let mut b = a.clone();
        for px in b.as_chunks_mut::<4>().0.iter_mut() {
            // A2B10G10R10: flip the two alpha bits only
            let v = u32::from_le_bytes(*px) ^ (3 << 30);
            *px = v.to_le_bytes();
        }
        assert_ne!(a, b);
        let s = diff(meta, &mut Cursor::new(a), &mut Cursor::new(b)).unwrap();
        assert_eq!((s.differing, s.interior), (0, 0));
        for (w, h) in [(0u32, 4u32), (4, 0), (0, 0)] {
            let m = DumpMeta {
                frame: 1,
                w,
                h,
                fmt: 64,
            };
            let s = diff(m, &mut Cursor::new(vec![]), &mut Cursor::new(vec![])).unwrap();
            assert_eq!(s.pixels, 0);
        }
    }

    #[test]
    fn dump_files_with_the_wrong_length_are_refused() {
        let dir = crate::vr::test_dir("dump-len");
        let p = dir.join("frame1_4x4_fmt64.raw");
        let meta = parse_name(&p).unwrap();
        std::fs::write(&p, vec![0u8; 64]).unwrap();
        assert!(check_len(&p, &meta).is_ok());
        std::fs::write(&p, vec![0u8; 60]).unwrap();
        let e = check_len(&p, &meta).unwrap_err();
        assert!(
            e.msg.contains("60 bytes") && e.msg.contains("64 bytes"),
            "{}",
            e.msg
        );
        assert!(check_len(&dir.join("missing.raw"), &meta).is_err());
    }

    #[test]
    fn diff_reports_truncated_input() {
        let (meta, a) = flat_image(4, 4);
        let e = diff(
            meta,
            &mut Cursor::new(a.clone()),
            &mut Cursor::new(a[..20].to_vec()),
        )
        .unwrap_err();
        assert!(e.msg.contains("second file"), "{}", e.msg);
        // single row and single column images still work
        let one = DumpMeta {
            frame: 1,
            w: 3,
            h: 1,
            fmt: 37,
        };
        let s = diff(
            one,
            &mut Cursor::new(vec![0u8; 12]),
            &mut Cursor::new(vec![0u8; 12]),
        )
        .unwrap();
        assert_eq!(s.pixels, 3);
    }

    #[test]
    fn a_black_frame_is_flagged() {
        let meta = DumpMeta {
            frame: 9,
            w: 4,
            h: 4,
            fmt: 37,
        };
        let s = analyse(meta, &mut Cursor::new(vec![0u8; 64]), 1, &[]).unwrap();
        assert!(s.all.black_percent() > 99.9);
        assert!(report(Path::new("frame9_4x4_fmt37.raw"), &s)
            .join("\n")
            .contains("<-- BLACK"));
    }

    #[test]
    fn sampling_step_and_truncated_files() {
        let (meta, d) = image();
        let s = analyse(meta, &mut Cursor::new(d.clone()), 2, &[]).unwrap();
        assert_eq!(s.all.n, 8, "4 columns x 2 rows");
        let e = analyse(meta, &mut Cursor::new(d[..d.len() - 4].to_vec()), 1, &[]).unwrap_err();
        assert!(e.msg.contains("short file"), "{}", e.msg);
        let bad = DumpMeta { fmt: 1, ..meta };
        assert!(analyse(bad, &mut Cursor::new(vec![]), 1, &[]).is_err());
    }

    #[test]
    fn ppm_preview() {
        let (meta, d) = image();
        let mut out = Vec::new();
        write_ppm(meta, &mut Cursor::new(d), 2, &mut out).unwrap();
        let header = b"P6 4 2 255\n";
        assert_eq!(&out[..header.len()], header);
        assert_eq!(out.len(), header.len() + 4 * 2 * 3);
        // second sampled pixel of the first row is x=2: left half (0,32,128)
        let px = &out[header.len() + 3..header.len() + 6];
        assert_eq!(px, [0, 32, 128]);
    }

    #[test]
    fn eight_bit_sources_compare_with_ten_bit_ones() {
        // the same colour stored as 10 bit (0,128,512) and as 8 bit (0,32,128) reports the same numbers
        let ten = PixFmt::A2B10G10R10.rgb(px10(0, 128, 512));
        let eight = PixFmt::Bgra8.rgb([128, 32, 0, 255]);
        for i in 0..3 {
            assert!((ten[i] - eight[i]).abs() < 0.5, "{ten:?} vs {eight:?}");
        }
    }
}
