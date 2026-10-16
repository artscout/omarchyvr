//! The tag: family, raster, printable SVG/PDF, and a fisheye renderer for tests.

use apriltag_sys as sys;
use pimax_cam::geom::{self, Pose};
use pimax_cam::kb4::Kb4;

pub const FAMILY: &str = "tag36h11";

/// Owned apriltag family (tag36h11).
pub struct Family {
    pub(crate) fam: *mut sys::apriltag_family_t,
}

// The family is immutable after creation.
unsafe impl Send for Family {}

impl Family {
    pub fn tag36h11() -> Family {
        Family {
            fam: unsafe { sys::tag36h11_create() },
        }
    }

    pub fn ncodes(&self) -> u32 {
        unsafe { (*self.fam).ncodes }
    }

    /// The tag as a raster: `(cells per side, pixels)` with 255 = white. For tag36h11 it is 10x10: a white ring,
    /// the 8x8 black square (the "tag size" is its edge) and the 6x6 data bits inside.
    pub fn raster(&self, id: u32) -> (usize, Vec<u8>) {
        assert!(id < self.ncodes(), "tag id out of range");
        unsafe {
            let im = sys::apriltag_to_image(self.fam, id);
            let w = (*im).width as usize;
            let stride = (*im).stride as usize;
            let mut out = vec![0u8; w * w];
            for y in 0..w {
                for x in 0..w {
                    out[y * w + x] = *(*im).buf.add(y * stride + x);
                }
            }
            sys::image_u8_destroy(im);
            (w, out)
        }
    }
}

impl Drop for Family {
    fn drop(&mut self) {
        unsafe { sys::tag36h11_destroy(self.fam) }
    }
}

/// Tag frame: origin at the centre, x right, y up (as printed), z out of the tag face. The four corners of the black
/// square in the order the AprilTag detector reports them (counter-clockwise as seen on the printed tag, starting at the
/// bottom-left corner; verified by the synthetic tests), in units of half the tag size.
pub const CORNERS: [(f64, f64); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];

/// Corner `i` of a tag of edge `size` (m) in the tag frame.
pub fn corner(i: usize, size: f64) -> [f64; 3] {
    [CORNERS[i].0 * size / 2.0, CORNERS[i].1 * size / 2.0, 0.0]
}

/// Printable SVG, millimetres, A4 portrait, tag centred in the upper part, 100 mm ruler below.
pub fn svg(fam: &Family, id: u32, size_mm: f64) -> String {
    let (w, px) = fam.raster(id);
    let cell = size_mm / 8.0;
    let (pw, ph) = (210.0, 297.0);
    let (cx, cy) = (pw / 2.0, 20.0 + size_mm / 2.0 + cell);
    let x0 = cx - 5.0 * cell;
    let y0 = cy - 5.0 * cell;
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{pw}mm\" height=\"{ph}mm\" viewBox=\"0 0 {pw} {ph}\">\n<rect width=\"{pw}\" height=\"{ph}\" fill=\"white\"/>\n"
    );
    s += &format!(
        "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" fill=\"black\"/>\n",
        x0 + cell,
        y0 + cell,
        8.0 * cell,
        8.0 * cell
    );
    for r in 2..w - 2 {
        for c in 2..w - 2 {
            if px[r * w + c] > 127 {
                s += &format!(
                    "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" fill=\"white\"/>\n",
                    x0 + c as f64 * cell,
                    y0 + r as f64 * cell,
                    cell,
                    cell
                );
            }
        }
    }
    let ry = y0 + 10.0 * cell + 18.0;
    s += &format!("<path d=\"M{} {ry}h100M{} {}v6M{} {}v6\" stroke=\"black\" stroke-width=\"0.4\"/>\n", cx - 50.0, cx - 50.0, ry - 3.0, cx + 50.0, ry - 3.0);
    s += &format!(
        "<text x=\"{cx}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"4\" text-anchor=\"middle\">{FAMILY} id {id}, black square {size_mm} mm. Print at 100% (no fit to page); this bar must measure 100 mm.</text>\n",
        ry + 9.0
    );
    s + "</svg>\n"
}

/// Printable PDF (one A4 page, vector, same layout as [`svg`] without the caption text).
pub fn pdf(fam: &Family, id: u32, size_mm: f64) -> Vec<u8> {
    let (w, px) = fam.raster(id);
    let mm = 72.0 / 25.4;
    let cell = size_mm / 8.0;
    let (cx, top) = (105.0, 20.0 + size_mm / 2.0 + cell);
    let x0 = cx - 5.0 * cell;
    let y0 = top - 5.0 * cell; // from the page top
    let ph = 297.0;
    let mut c = String::new();
    let rect = |x: f64, y: f64, wd: f64, ht: f64| {
        // y measured from the top; PDF origin is bottom-left
        format!("{:.3} {:.3} {:.3} {:.3} re f\n", x * mm, (ph - y - ht) * mm, wd * mm, ht * mm)
    };
    c += "0 g\n";
    c += &rect(x0 + cell, y0 + cell, 8.0 * cell, 8.0 * cell);
    c += "1 g\n";
    for r in 2..w - 2 {
        for k in 2..w - 2 {
            if px[r * w + k] > 127 {
                c += &rect(x0 + k as f64 * cell, y0 + r as f64 * cell, cell, cell);
            }
        }
    }
    // 100 mm ruler with end ticks
    let ry = y0 + 10.0 * cell + 18.0;
    c += "0 g\n";
    c += &rect(cx - 50.0, ry - 0.2, 100.0, 0.4);
    c += &rect(cx - 50.0, ry - 3.0, 0.4, 6.0);
    c += &rect(cx + 50.0 - 0.4, ry - 3.0, 0.4, 6.0);
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: String| {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", offs.len(), body).as_bytes());
    };
    obj(&mut out, "<< /Type /Catalog /Pages 2 0 R >>".into());
    obj(&mut out, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into());
    obj(
        &mut out,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.3} {:.3}] /Contents 4 0 R >>",
            210.0 * mm,
            297.0 * mm
        ),
    );
    obj(&mut out, format!("<< /Length {} >>\nstream\n{}endstream", c.len(), c));
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offs.len() + 1).as_bytes());
    for o in &offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offs.len() + 1).as_bytes(),
    );
    out
}

/// Render a tag seen by a KB4 fisheye camera (for tests). `cam_from_tag` places the tag (edge `size` m) in the camera
/// frame. The sheet around the tag is white for one cell, the desk beyond it grey. 3x3 supersampling, a [1 2 1]
/// blur for the lens and a little deterministic noise.
pub fn render(kb: &Kb4, cam_from_tag: &Pose, size: f64, raster: (usize, &[u8]), seed: u64) -> Vec<u8> {
    let (rw, px) = raster;
    let cell = size / 8.0;
    let half = rw as f64 / 2.0;
    let n = geom::mat_vec(&cam_from_tag.r, [0.0, 0.0, 1.0]);
    let t = cam_from_tag.t;
    let tn = geom::dot(t, n);
    let (w, h) = (kb.width as usize, kb.height as usize);
    let mut img = vec![0f32; w * h];
    const SS: usize = 3;
    for v in 0..h {
        for u in 0..w {
            let mut acc = 0.0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let uu = u as f64 + (sx as f64 + 0.5) / SS as f64 - 0.5;
                    let vv = v as f64 + (sy as f64 + 0.5) / SS as f64 - 0.5;
                    let val = match kb.unproject(uu, vv) {
                        None => 90.0,
                        Some(d) => {
                            let dn = geom::dot(d, n);
                            if dn.abs() < 1e-9 {
                                90.0
                            } else {
                                let lam = tn / dn;
                                if lam <= 0.0 {
                                    90.0
                                } else {
                                    let p = geom::sub(geom::scale(d, lam), t);
                                    let q = geom::mat_vec(&geom::transpose(&cam_from_tag.r), p);
                                    let c = q[0] / cell + half;
                                    let r = -q[1] / cell + half;
                                    if c < 0.0 || r < 0.0 || c >= rw as f64 || r >= rw as f64 {
                                        90.0
                                    } else {
                                        // paper 215, black 20
                                        20.0 + px[r as usize * rw + c as usize] as f64 / 255.0 * 195.0
                                    }
                                }
                            }
                        }
                    };
                    acc += val;
                }
            }
            img[v * w + u] = (acc / (SS * SS) as f64) as f32;
        }
    }
    // blur [1 2 1] separable
    let mut tmp = img.clone();
    for v in 0..h {
        for u in 1..w - 1 {
            tmp[v * w + u] = (img[v * w + u - 1] + 2.0 * img[v * w + u] + img[v * w + u + 1]) / 4.0;
        }
    }
    let mut out = vec![0u8; w * h];
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    for v in 0..h {
        for u in 0..w {
            let a = if v > 0 && v < h - 1 {
                (tmp[(v - 1) * w + u] + 2.0 * tmp[v * w + u] + tmp[(v + 1) * w + u]) / 4.0
            } else {
                tmp[v * w + u]
            };
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let noise = ((s >> 33) % 7) as f32 - 3.0;
            out[v * w + u] = (a + noise).clamp(0.0, 255.0) as u8;
        }
    }
    out
}
