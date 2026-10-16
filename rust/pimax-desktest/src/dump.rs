//! Reader for the compositor target dump written by `XRT_COMPOSITOR_DUMP_DIR`
//! (`frame<N>_<w>x<h>_fmt<vk>.raw`, tightly packed rows of the final image before present).

use std::io;
use std::path::Path;

pub const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
pub const VK_FORMAT_R8G8B8A8_SRGB: u32 = 43;
pub const VK_FORMAT_B8G8R8A8_UNORM: u32 = 44;
pub const VK_FORMAT_B8G8R8A8_SRGB: u32 = 50;
pub const VK_FORMAT_A2B10G10R10_UNORM_PACK32: u32 = 64;

/// Fields parsed from a dump file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DumpName {
    pub frame: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
}

/// Parses `frame<N>_<w>x<h>_fmt<vk>.raw`.
pub fn parse_name(file_name: &str) -> Option<DumpName> {
    let stem = file_name.strip_suffix(".raw")?;
    let rest = stem.strip_prefix("frame")?;
    let (frame, rest) = rest.split_once('_')?;
    let (dims, fmt) = rest.split_once("_fmt")?;
    let (w, h) = dims.split_once('x')?;
    Some(DumpName { frame: frame.parse().ok()?, width: w.parse().ok()?, height: h.parse().ok()?, format: fmt.parse().ok()? })
}

/// Which channel [`Dump::gray_crop_channel`] extracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    R,
    G,
    B,
    /// Rec. 709 luma of the display-encoded values.
    Luma,
}

impl Channel {
    /// Parses `r|g|b|y`.
    pub fn parse(s: &str) -> Option<Channel> {
        match s {
            "r" => Some(Channel::R),
            "g" => Some(Channel::G),
            "b" => Some(Channel::B),
            "y" | "luma" => Some(Channel::Luma),
            _ => None,
        }
    }
}

/// A rectangle in image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// Parses `x,y,w,h`.
    pub fn parse(s: &str) -> Option<Rect> {
        let v: Vec<u32> = s.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        match v[..] {
            [x, y, w, h] if w > 0 && h > 0 => Some(Rect { x, y, w, h }),
            _ => None,
        }
    }
}

/// An 8-bit RGB image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgb8 {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// A single-channel float image (encoded luma, 0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.w + x]
    }
}

/// The raw target image.
pub struct Dump {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    bytes: Vec<u8>,
}

impl Dump {
    /// Wraps raw bytes; fails when the format is unsupported or the size does not match.
    pub fn from_parts(width: u32, height: u32, format: u32, bytes: Vec<u8>) -> io::Result<Dump> {
        let supported = [
            VK_FORMAT_R8G8B8A8_UNORM,
            VK_FORMAT_R8G8B8A8_SRGB,
            VK_FORMAT_B8G8R8A8_UNORM,
            VK_FORMAT_B8G8R8A8_SRGB,
            VK_FORMAT_A2B10G10R10_UNORM_PACK32,
        ];
        if !supported.contains(&format) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsupported VkFormat {format}")));
        }
        if bytes.len() != width as usize * height as usize * 4 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file size does not match width*height*4"));
        }
        Ok(Dump { width, height, format, bytes })
    }

    /// Loads a dump, taking width, height and format from the file name.
    pub fn load(path: &Path) -> io::Result<Dump> {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let n = parse_name(name).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a frame<N>_<w>x<h>_fmt<vk>.raw name"))?;
        Dump::from_parts(n.width, n.height, n.format, std::fs::read(path)?)
    }

    /// Display-encoded colour of a pixel, 0..=255 per channel (10-bit values are rounded down to 8 bit).
    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let o = (y as usize * self.width as usize + x as usize) * 4;
        let b = &self.bytes[o..o + 4];
        match self.format {
            VK_FORMAT_R8G8B8A8_UNORM | VK_FORMAT_R8G8B8A8_SRGB => [b[0], b[1], b[2]],
            VK_FORMAT_B8G8R8A8_UNORM | VK_FORMAT_B8G8R8A8_SRGB => [b[2], b[1], b[0]],
            _ => {
                let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                [((v & 1023) >> 2) as u8, (((v >> 10) & 1023) >> 2) as u8, (((v >> 20) & 1023) >> 2) as u8]
            }
        }
    }

    /// Full-precision channels in 0..1 (10-bit data keeps its precision).
    pub fn rgb_f(&self, x: u32, y: u32) -> [f32; 3] {
        let o = (y as usize * self.width as usize + x as usize) * 4;
        let b = &self.bytes[o..o + 4];
        match self.format {
            VK_FORMAT_A2B10G10R10_UNORM_PACK32 => {
                let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                [(v & 1023) as f32 / 1023.0, ((v >> 10) & 1023) as f32 / 1023.0, ((v >> 20) & 1023) as f32 / 1023.0]
            }
            _ => {
                let c = self.rgb(x, y);
                [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0]
            }
        }
    }

    /// Rect covering one eye (the target is the left eye then the right eye side by side).
    pub fn eye(&self, right: bool) -> Rect {
        let half = self.width / 2;
        Rect { x: if right { half } else { 0 }, y: 0, w: half, h: self.height }
    }

    fn clamp(&self, r: Rect) -> Rect {
        let x = r.x.min(self.width.saturating_sub(1));
        let y = r.y.min(self.height.saturating_sub(1));
        Rect { x, y, w: r.w.min(self.width - x).max(1), h: r.h.min(self.height - y).max(1) }
    }

    /// Crops `r`, box-averages by `down` (>= 1) and then repeats pixels `up` times (>= 1, for viewing details).
    pub fn crop_rgb8(&self, r: Rect, down: u32, up: u32) -> Rgb8 {
        let r = self.clamp(r);
        let down = down.max(1);
        let up = up.max(1);
        let ow = (r.w / down).max(1);
        let oh = (r.h / down).max(1);
        let mut small = vec![0u8; ow as usize * oh as usize * 3];
        for oy in 0..oh {
            for ox in 0..ow {
                let mut acc = [0u32; 3];
                let mut n = 0u32;
                for dy in 0..down {
                    for dx in 0..down {
                        let (x, y) = (r.x + ox * down + dx, r.y + oy * down + dy);
                        if x < self.width && y < self.height {
                            let c = self.rgb(x, y);
                            acc[0] += u32::from(c[0]);
                            acc[1] += u32::from(c[1]);
                            acc[2] += u32::from(c[2]);
                            n += 1;
                        }
                    }
                }
                let o = (oy as usize * ow as usize + ox as usize) * 3;
                for c in 0..3 {
                    small[o + c] = ((acc[c] + n / 2) / n.max(1)) as u8;
                }
            }
        }
        if up == 1 {
            return Rgb8 { w: ow, h: oh, data: small };
        }
        let (uw, uh) = (ow * up, oh * up);
        let mut big = vec![0u8; uw as usize * uh as usize * 3];
        for y in 0..uh {
            for x in 0..uw {
                let s = ((y / up) as usize * ow as usize + (x / up) as usize) * 3;
                let d = (y as usize * uw as usize + x as usize) * 3;
                big[d..d + 3].copy_from_slice(&small[s..s + 3]);
            }
        }
        Rgb8 { w: uw, h: uh, data: big }
    }

    /// Luma (Rec. 709 weights on the display-encoded values) of `r`, 0..1.
    pub fn gray_crop(&self, r: Rect) -> Gray {
        self.gray_crop_channel(r, Channel::Luma)
    }

    /// One channel (or luma) of `r`, 0..1. Use a single channel to measure sharpness without the lateral chromatic
    /// aberration correction (which shifts R, G and B differently) smearing the luma.
    pub fn gray_crop_channel(&self, r: Rect, ch: Channel) -> Gray {
        let r = self.clamp(r);
        let mut data = Vec::with_capacity(r.w as usize * r.h as usize);
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let c = self.rgb_f(x, y);
                data.push(match ch {
                    Channel::R => c[0],
                    Channel::G => c[1],
                    Channel::B => c[2],
                    Channel::Luma => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2],
                });
            }
        }
        Gray { w: r.w as usize, h: r.h as usize, data }
    }

    /// Fraction of pixels in `r` whose channels are all <= `thresh` (8-bit units): how black an area is.
    pub fn black_fraction(&self, r: Rect, thresh: u8) -> f64 {
        let r = self.clamp(r);
        let mut n = 0u64;
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let c = self.rgb(x, y);
                if c.iter().all(|&v| v <= thresh) {
                    n += 1;
                }
            }
        }
        n as f64 / (u64::from(r.w) * u64::from(r.h)) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names() {
        assert_eq!(
            parse_name("frame2400_7104x3840_fmt64.raw"),
            Some(DumpName { frame: 2400, width: 7104, height: 3840, format: 64 })
        );
        assert_eq!(parse_name("frame1_2x2_fmt37.raw").unwrap().format, 37);
        assert_eq!(parse_name("frame_1x1.raw"), None);
        assert_eq!(parse_name("frame1_2x2_fmt37.png"), None);
        assert_eq!(parse_name("x"), None);
    }

    #[test]
    fn rect_parse() {
        assert_eq!(Rect::parse("1,2,3,4"), Some(Rect { x: 1, y: 2, w: 3, h: 4 }));
        assert_eq!(Rect::parse("1,2,0,4"), None);
        assert_eq!(Rect::parse("1,2,3"), None);
        assert_eq!(Rect::parse("a,b,c,d"), None);
    }

    fn a2b10g10r10(r: u32, g: u32, b: u32) -> [u8; 4] {
        (r | (g << 10) | (b << 20) | (3 << 30)).to_le_bytes()
    }

    #[test]
    fn decodes_a2b10g10r10() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&a2b10g10r10(1023, 0, 0));
        bytes.extend_from_slice(&a2b10g10r10(0, 512, 0));
        bytes.extend_from_slice(&a2b10g10r10(0, 0, 1023));
        bytes.extend_from_slice(&a2b10g10r10(4, 8, 12));
        let d = Dump::from_parts(2, 2, VK_FORMAT_A2B10G10R10_UNORM_PACK32, bytes).unwrap();
        assert_eq!(d.rgb(0, 0), [255, 0, 0]);
        assert_eq!(d.rgb(1, 0), [0, 128, 0]);
        assert_eq!(d.rgb(0, 1), [0, 0, 255]);
        assert_eq!(d.rgb(1, 1), [1, 2, 3]);
        let f = d.rgb_f(1, 0);
        assert!((f[1] - 512.0 / 1023.0).abs() < 1e-6);
    }

    #[test]
    fn decodes_8bit_formats_and_checks_sizes() {
        let px = [10u8, 20, 30, 255];
        let rgba = Dump::from_parts(1, 1, VK_FORMAT_R8G8B8A8_UNORM, px.to_vec()).unwrap();
        let bgra = Dump::from_parts(1, 1, VK_FORMAT_B8G8R8A8_SRGB, px.to_vec()).unwrap();
        assert_eq!(rgba.rgb(0, 0), [10, 20, 30]);
        assert_eq!(bgra.rgb(0, 0), [30, 20, 10]);
        assert!(Dump::from_parts(1, 1, 99, px.to_vec()).is_err());
        assert!(Dump::from_parts(2, 1, VK_FORMAT_R8G8B8A8_UNORM, px.to_vec()).is_err());
    }

    #[test]
    fn crop_scale_and_eyes() {
        // 4x2 image: left half red, right half green
        let mut bytes = Vec::new();
        for _y in 0..2 {
            for x in 0..4 {
                bytes.extend_from_slice(&if x < 2 { [255, 0, 0, 255] } else { [0, 255, 0, 255] });
            }
        }
        let d = Dump::from_parts(4, 2, VK_FORMAT_R8G8B8A8_UNORM, bytes).unwrap();
        assert_eq!(d.eye(false), Rect { x: 0, y: 0, w: 2, h: 2 });
        assert_eq!(d.eye(true), Rect { x: 2, y: 0, w: 2, h: 2 });
        let c = d.crop_rgb8(Rect { x: 0, y: 0, w: 4, h: 2 }, 2, 1);
        assert_eq!((c.w, c.h), (2, 1));
        assert_eq!(&c.data, &[255, 0, 0, 0, 255, 0]);
        let u = d.crop_rgb8(Rect { x: 1, y: 0, w: 2, h: 1 }, 1, 3);
        assert_eq!((u.w, u.h), (6, 3));
        assert_eq!(&u.data[..3], &[255, 0, 0]);
        assert_eq!(&u.data[u.data.len() - 3..], &[0, 255, 0]);
        // out of range crops are clamped, never panic
        let o = d.crop_rgb8(Rect { x: 3, y: 1, w: 50, h: 50 }, 1, 1);
        assert_eq!((o.w, o.h), (1, 1));
    }

    #[test]
    fn channel_selection() {
        let d = Dump::from_parts(1, 1, VK_FORMAT_R8G8B8A8_UNORM, vec![255, 0, 51, 255]).unwrap();
        let r = Rect { x: 0, y: 0, w: 1, h: 1 };
        assert!((d.gray_crop_channel(r, Channel::R).data[0] - 1.0).abs() < 1e-6);
        assert!(d.gray_crop_channel(r, Channel::G).data[0].abs() < 1e-6);
        assert!((d.gray_crop_channel(r, Channel::B).data[0] - 0.2).abs() < 1e-6);
        assert_eq!(Channel::parse("g"), Some(Channel::G));
        assert_eq!(Channel::parse("y"), Some(Channel::Luma));
        assert_eq!(Channel::parse("q"), None);
    }

    #[test]
    fn gray_and_black_fraction() {
        let bytes = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let d = Dump::from_parts(2, 1, VK_FORMAT_R8G8B8A8_UNORM, bytes).unwrap();
        let g = d.gray_crop(Rect { x: 0, y: 0, w: 2, h: 1 });
        assert!(g.data[0] < 1e-6 && (g.data[1] - 1.0).abs() < 1e-4);
        assert!((d.black_fraction(Rect { x: 0, y: 0, w: 2, h: 1 }, 2) - 0.5).abs() < 1e-12);
    }
}
