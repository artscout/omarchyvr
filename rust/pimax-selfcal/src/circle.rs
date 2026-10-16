//! Lens image circle: an intrinsics check that needs no matching.
//!
//! Each fisheye throws a circular image (radius ~360 px) on the 640x480 sensor, so the four corners of a tile are
//! dark (only about 12 degrees of arc per corner lie inside the tile). The centre of that circle is fixed by the lens over the sensor, like the principal point of the
//! calibration, and the two normally agree within a few pixels. A larger offset in one tile means that the
//! picture in the tile is not where the calibration expects it (a shifted readout window, or a principal point
//! that is off), which a camera pair then shows as a large "rotation".
//!
//! [`fit`] finds the bright-to-dark edge along rays from the expected centre and fits a circle to it. It needs a
//! bright scene near the corners (daylight frames; a dark room gives no edge points).

/// A fitted circle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    /// Centre x (pixels).
    pub cx: f64,
    /// Centre y.
    pub cy: f64,
    /// Radius.
    pub r: f64,
    /// Edge points used.
    pub points: usize,
    /// Root mean square distance of those points from the circle (pixels).
    pub rms: f64,
    /// Angular coverage: how many of the four image corners contributed at least 10 points.
    pub corners: usize,
}

fn at(img: &[u8], w: usize, h: usize, x: f64, y: f64) -> Option<f64> {
    if x < 0.0 || y < 0.0 || x > (w - 1) as f64 || y > (h - 1) as f64 {
        return None;
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let p = |xx: usize, yy: usize| img[yy * w + xx] as f64;
    Some(
        p(x0, y0) * (1.0 - fx) * (1.0 - fy)
            + p(x1, y0) * fx * (1.0 - fy)
            + p(x0, y1) * (1.0 - fx) * fy
            + p(x1, y1) * fx * fy,
    )
}

/// Edge points `(x, y)` of the image circle: along each ray from `(cx0, cy0)`, the outermost radius where the
/// brightness falls halfway from the level just inside to the dark level outside. Only rays that end in the
/// dark (those aimed at the tile corners) and come from a bright region count; edges of the scene itself are
/// ignored because the search runs inwards from the dark end of the ray.
pub fn edge_points(img: &[u8], w: usize, h: usize, cx0: f64, cy0: f64) -> Vec<(f64, f64)> {
    const R0: usize = 290;
    let mut out = Vec::new();
    for k in 0..2880 {
        let phi = k as f64 * std::f64::consts::TAU / 2880.0;
        let (c, s) = (phi.cos(), phi.sin());
        // radial profile while inside the image
        let prof: Vec<f64> = (R0..R0 + 200)
            .map_while(|r| at(img, w, h, cx0 + r as f64 * c, cy0 + r as f64 * s))
            .collect();
        let n = prof.len();
        if n < 50 {
            continue;
        }
        let mean = |a: usize, b: usize| prof[a..b].iter().sum::<f64>() / (b - a) as f64;
        // the ray must end in the dark corner
        let outer = mean(n - 6, n);
        if outer > 14.0 {
            continue;
        }
        // from the dark end inwards: first clearly lit sample
        let Some(e0) = (0..n - 6).rev().find(|&i| prof[i] > outer + 30.0) else {
            continue;
        };
        if e0 < 30 {
            continue;
        }
        let inner = mean(e0 - 26, e0 - 8);
        if inner < 60.0 {
            continue;
        }
        let mid = 0.5 * (inner + outer);
        // outermost crossing of the mid level
        let Some(j) = (e0.saturating_sub(12)..n - 1)
            .rev()
            .find(|&j| prof[j] >= mid && prof[j + 1] < mid)
        else {
            continue;
        };
        let r = R0 as f64 + j as f64 + (prof[j] - mid) / (prof[j] - prof[j + 1]);
        out.push((cx0 + r * c, cy0 + r * s));
    }
    out
}

/// Algebraic circle fit (Kasa) to the points.
fn kasa(p: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    if p.len() < 3 {
        return None;
    }
    // x^2 + y^2 + a x + b y + c = 0, centred coordinates for conditioning
    let n = p.len() as f64;
    let (mx, my) = (
        p.iter().map(|q| q.0).sum::<f64>() / n,
        p.iter().map(|q| q.1).sum::<f64>() / n,
    );
    let mut m = vec![vec![0.0; 3]; 3];
    let mut v = vec![0.0; 3];
    for &(x, y) in p {
        let (x, y) = (x - mx, y - my);
        let row = [x, y, 1.0];
        let rhs = -(x * x + y * y);
        for i in 0..3 {
            v[i] += row[i] * rhs;
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
        }
    }
    let s = crate::ba::solve_dense(&m, &v)?;
    let (cx, cy) = (-s[0] / 2.0, -s[1] / 2.0);
    let r2 = cx * cx + cy * cy - s[2];
    (r2 > 0.0).then(|| (cx + mx, cy + my, r2.sqrt()))
}

/// Fit the image circle of a tile, starting from the expected centre `(cx0, cy0)` (the calibration's principal
/// point). `None` without enough edge points in at least three corners.
pub fn fit(img: &[u8], w: usize, h: usize, cx0: f64, cy0: f64) -> Option<Circle> {
    fit_points(edge_points(img, w, h, cx0, cy0), cx0, cy0)
}

/// Fit a circle to edge points (of one or several frames of the same camera); `(cx0, cy0)` only decides which
/// corner a point belongs to. `None` without enough points in at least three corners.
pub fn fit_points(mut pts: Vec<(f64, f64)>, cx0: f64, cy0: f64) -> Option<Circle> {
    let mut c = kasa(&pts)?;
    for _ in 0..4 {
        let res: Vec<f64> = pts
            .iter()
            .map(|&(x, y)| ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() - c.2)
            .collect();
        let mut a: Vec<f64> = res.iter().map(|r| r.abs()).collect();
        a.sort_by(|p, q| p.partial_cmp(q).unwrap());
        let thr = (2.5 * 1.4826 * a[a.len() / 2]).max(1.0);
        let keep: Vec<(f64, f64)> = pts
            .iter()
            .zip(&res)
            .filter(|(_, r)| r.abs() <= thr)
            .map(|(p, _)| *p)
            .collect();
        if keep.len() == pts.len() || keep.len() < 30 {
            break;
        }
        pts = keep;
        c = kasa(&pts)?;
    }
    let mut quad = [0usize; 4];
    for &(x, y) in &pts {
        quad[(x > cx0) as usize + 2 * (y > cy0) as usize] += 1;
    }
    let corners = quad.iter().filter(|&&n| n >= 10).count();
    if pts.len() < 40 || corners < 3 {
        return None;
    }
    let rms = (pts
        .iter()
        .map(|&(x, y)| (((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() - c.2).powi(2))
        .sum::<f64>()
        / pts.len() as f64)
        .sqrt();
    Some(Circle {
        cx: c.0,
        cy: c.1,
        r: c.2,
        points: pts.len(),
        rms,
        corners,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bright disc with a soft edge and some scene structure inside.
    fn disc(cx: f64, cy: f64, r: f64) -> Vec<u8> {
        let (w, h) = (640usize, 480usize);
        let mut img = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                let inside = (0.5 - (d - r) / 3.0).clamp(0.0, 1.0);
                let scene = 170.0 + 50.0 * ((x as f64 * 0.05).sin() * (y as f64 * 0.07).cos());
                img[y * w + x] = (4.0 + inside * scene) as u8;
            }
        }
        img
    }

    #[test]
    fn finds_an_offset_circle() {
        for &(cx, cy, r) in &[
            (320.0, 240.0, 362.0),
            (331.5, 236.0, 358.0),
            (312.0, 251.5, 366.0),
        ] {
            let img = disc(cx, cy, r);
            // start from the nominal centre, 10 px away from the truth
            let c = fit(&img, 640, 480, 322.0, 243.0).expect("circle");
            assert!(
                (c.cx - cx).abs() < 0.3 && (c.cy - cy).abs() < 0.3,
                "{c:?} vs {cx},{cy}"
            );
            assert!((c.r - r).abs() < 0.5, "{c:?}");
            assert_eq!(c.corners, 4);
            assert!(c.rms < 0.5 && c.points > 100, "{c:?}");
        }
    }

    #[test]
    fn dark_or_flat_images_give_nothing() {
        assert!(fit(&vec![3u8; 640 * 480], 640, 480, 320.0, 240.0).is_none());
        assert!(fit(&vec![200u8; 640 * 480], 640, 480, 320.0, 240.0).is_none());
        assert!(kasa(&[(0.0, 0.0), (1.0, 1.0)]).is_none());
    }
}
