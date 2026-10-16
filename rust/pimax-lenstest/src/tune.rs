//! Live distortion tuning by pre-warping the client image.
//!
//! The lens distortion is applied by Monado's compositor from `lens.conf` (read once at service start); the client
//! cannot change it. What the client CAN do is draw its pattern through a small radial warp, which is the same as a
//! small change of the compositor's model for small residuals:
//!
//! * The compositor shows, at panel point `p`, the client image at tangent `t = M(p)`.
//! * A pixel of the client image at tangent `t` is drawn as the pattern ray
//!   `W(t) = s * t * (1 + dk * u^2 + dk2 * B(u))`, `u = |t|/g`
//!   (radial, about the image tangent 0 = the optical axis; `g` = centre gain `base_g * scale`, tangent per
//!   half-height, so `dk` has the units of `k1`). `B` is a localised bump, see [`bump`]: `dk2` is the fractional
//!   change of the radial magnification at the mid zone (about 24 degrees off axis), and it fades out towards the
//!   visible edge, so it reshapes the mid zone without moving the edge (the edge is `dk1` / `s`).
//! * So the wearer sees ray `W(M(p))` at `p`: exactly the model `M' = W o M`.
//!
//! `M' ` is put back into the `lens.conf` parameterisation: per channel `t = A r (1 + k1 x + ... + k5 x^5)`,
//! `x = r^2`, `A = base * scale`. `W` multiplies the whole curve by `s`, so `scale' = scale * s` EXACTLY, and the
//! polynomial must reproduce `P'(x) = P(x) * (1 + dk u^2)`, `u = A r P / g`, which is a degree-7 polynomial in `x`
//! (for the usual 3 terms): the five `k` that lens.conf takes (`k4`, `k5` are only printed when they matter) are fitted by weighted least squares over the visible radius range
//! (weights: derivative of the ray angle, so the fit minimises the angle error). The residual error in degrees is
//! reported; above [`WARN_DEG`] a warning is printed.

use pimax_lens::model::{radial_tangent, Lens};

/// Fit error above which a warning is printed.
pub const WARN_DEG: f32 = 0.1;
/// Number of polynomial terms fitted (`k1`..`k5`, all that `lens.conf` accepts).
pub const NK: usize = 5;
/// One key step of `dk1`.
pub const DK_STEP: f32 = 0.01;
/// One key step of the residual scale (0.5 %).
pub const SCALE_STEP: f32 = 0.005;
pub const MAX_UNITS: i32 = 400;

/// One key step of `dk2`.
pub const DK2_STEP: f32 = 0.01;
/// Peak of the mid-zone bump in units of `u = |t| / g` (tangent 0.455 for g 0.65: 24.5 degrees off axis).
pub const BUMP_U: f64 = 0.7;

/// The mid-zone bump `B(u) = a e^(1-a)`, `a = (u/BUMP_U)^2`: 0 on the axis (like `u^2`), exactly 1 at `u = BUMP_U`,
/// 0.02 at the visible edge (u about 1.8). Its derivative term for Newton: `d(u B)/du = B (3 - 2a)`.
pub fn bump(u: f64) -> f64 {
    let a = (u / BUMP_U) * (u / BUMP_U);
    a * (1.0 - a).exp()
}

/// The tuning state in key steps (integers: exact, hashable).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tune {
    pub dk: i32,
    pub sc: i32,
    pub dk2: i32,
}

impl Tune {
    pub fn is_zero(&self) -> bool {
        *self == Tune::default()
    }

    pub fn dk1(&self) -> f32 {
        self.dk as f32 * DK_STEP
    }

    pub fn scale(&self) -> f32 {
        1.0 + self.sc as f32 * SCALE_STEP
    }

    pub fn dk2(&self) -> f32 {
        self.dk2 as f32 * DK2_STEP
    }

    pub fn add2(&mut self, n: i32) {
        self.dk2 = (self.dk2 + n).clamp(-MAX_UNITS, MAX_UNITS);
    }

    pub fn add(&mut self, dk: i32, sc: i32) {
        self.dk = (self.dk + dk).clamp(-MAX_UNITS, MAX_UNITS);
        self.sc = (self.sc + sc).clamp(-MAX_UNITS, MAX_UNITS);
    }

    /// The pixel warp for a centre gain `g`.
    pub fn warp(&self, g: f32) -> Warp {
        Warp {
            s: self.scale(),
            dk: self.dk1(),
            dk2: self.dk2(),
            g,
        }
    }

    pub fn short(&self) -> String {
        format!(
            "TUNE DK1 {:.2} DK2 {:.2} SCALE {:.3}",
            self.dk1(),
            self.dk2(),
            self.scale()
        )
    }
}

/// The radial warp `W` (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub s: f32,
    pub dk: f32,
    /// Amplitude of the mid-zone bump ([`bump`]).
    pub dk2: f32,
    pub g: f32,
}

impl Warp {
    pub const IDENTITY: Warp = Warp {
        s: 1.0,
        dk: 0.0,
        dk2: 0.0,
        g: 1.0,
    };

    pub fn is_identity(&self) -> bool {
        self.s == 1.0 && self.dk == 0.0 && self.dk2 == 0.0
    }

    /// Image tangent -> pattern tangent.
    pub fn apply(&self, t: (f32, f32)) -> (f32, f32) {
        if self.is_identity() {
            return t;
        }
        let u = t.0.hypot(t.1) / self.g;
        let f = self.s * (1.0 + self.dk * u * u + self.dk2 * bump(f64::from(u)) as f32);
        (t.0 * f, t.1 * f)
    }

    /// Pattern tangent -> image tangent (Newton on the radius).
    pub fn invert(&self, t: (f32, f32)) -> (f32, f32) {
        if self.is_identity() {
            return t;
        }
        let rho = t.0.hypot(t.1);
        if rho == 0.0 {
            return t;
        }
        let mut r = rho / self.s;
        for _ in 0..30 {
            let u = r / self.g;
            let bu = bump(f64::from(u));
            let a = (u / BUMP_U as f32).powi(2);
            let f = self.s * r * (1.0 + self.dk * u * u + self.dk2 * bu as f32) - rho;
            let d = self.s * (1.0 + 3.0 * self.dk * u * u + self.dk2 * bu as f32 * (3.0 - 2.0 * a));
            if d < 0.05 {
                break;
            }
            r -= f / d;
            if f.abs() < 1e-7 {
                break;
            }
        }
        let k = r / rho;
        (t.0 * k, t.1 * k)
    }
}

/// Fit of one channel.
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    pub k: [f64; NK],
    /// Largest ray-angle difference between the warped model and the fitted one over the visible range (degrees).
    pub max_err_deg: f64,
    /// Radius range of the fit (panel half-heights, in the channel's own p').
    pub rmax: f64,
}

/// Solve the `N`x`N` system given as an augmented matrix (Gaussian elimination with pivoting).
fn solve<const N: usize>(mut a: [[f64; 6]; N]) -> Option<[f64; N]> {
    for c in 0..N {
        let piv = (c..N).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[piv][c].abs() < 1e-18 {
            return None;
        }
        a.swap(c, piv);
        for r in 0..N {
            if r != c {
                let f = a[r][c] / a[c][c];
                let pivot_row = a[c];
                for (cell, pv) in a[r].iter_mut().zip(pivot_row).skip(c) {
                    *cell -= f * pv;
                }
            }
        }
    }
    let mut x = [0.0; N];
    for i in 0..N {
        x[i] = a[i][5] / a[i][i];
    }
    Some(x)
}

/// Fit the model `t = A r P(r^2)` composed with the warp `(s, dk, dk2, g)` back to three `k` terms over `r in (0, rmax]`.
/// `k` are the existing terms (up to five).
pub fn fit_channel(a_gain: f64, k: &[f32], s: f64, dk: f64, dk2: f64, g: f64, rmax: f64) -> Fit {
    let poly = |x: f64| -> f64 {
        let mut p = 1.0;
        let mut pw = 1.0;
        for &ki in k {
            pw *= x;
            p += f64::from(ki) * pw;
        }
        p
    };
    // composed: t'(r) = s * t * (1 + dk (t/g)^2); target polynomial value P'(x) = t' / (A s r)
    let target = |r: f64| -> f64 {
        let t = a_gain * r * poly(r * r);
        let u = t / g;
        poly(r * r) * (1.0 + dk * u * u + dk2 * bump(u))
    };
    let n = 400;
    let mut m = [[0.0f64; 6]; NK];
    for i in 1..=n {
        let r = rmax * i as f64 / n as f64;
        let x = r * r;
        let t = s * a_gain * r * target(r);
        let w = a_gain * s * r / (1.0 + t * t);
        let w2 = w * w;
        let basis: [f64; NK] = std::array::from_fn(|j| x.powi(j as i32 + 1));
        let y = target(r) - 1.0;
        for (row, bi) in m.iter_mut().zip(basis) {
            for (cell, bj) in row.iter_mut().zip(basis) {
                *cell += w2 * bi * bj;
            }
            row[5] += w2 * bi * y;
        }
    }
    let kk = solve(m).unwrap_or([0.0; NK]);
    let mut max_err = 0.0f64;
    for i in 1..=n {
        let r = rmax * i as f64 / n as f64;
        let x = r * r;
        let want = s * a_gain * r * target(r);
        let got = s
            * a_gain
            * r
            * (1.0
                + kk.iter()
                    .enumerate()
                    .map(|(j, k)| k * x.powi(j as i32 + 1))
                    .sum::<f64>());
        let e = (want.atan() - got.atan()).to_degrees().abs();
        max_err = max_err.max(e);
    }
    Fit {
        k: kk,
        max_err_deg: max_err,
        rmax,
    }
}

/// Visible radius of channel `c`: where the (unwarped) model reaches the widest FOV half-angle.
fn visible_radius(base: f64, scale: f64, k: &[f32; 5], fov_deg: [f32; 4]) -> f64 {
    let want = f64::from(fov_deg.iter().cloned().fold(0.0, f32::max))
        .to_radians()
        .tan();
    let mut r = 0.0;
    while r < 3.0 {
        r += 0.002;
        if radial_tangent(base, scale, k, r).0 >= want {
            return r;
        }
    }
    1.5
}

/// The `lens.conf` lines for one eye and the worst fit error (degrees).
pub fn eye_conf(lens: &Lens, eye: usize, tune: &Tune) -> (Vec<String>, f32) {
    let e = lens.eye(eye);
    let g = f64::from(e.base[1]) * f64::from(e.scale);
    let s = f64::from(tune.scale());
    let mut lines = vec![format!("scale = {:.5}", f64::from(e.scale) * s)];
    let mut worst = 0.0f64;
    for (c, name) in [(1usize, "k_g"), (0, "k_r"), (2, "k_b")] {
        let a = f64::from(e.base[c]) * f64::from(e.scale);
        let rmax = visible_radius(f64::from(e.base[c]), f64::from(e.scale), &e.k[c], e.fov);
        let fit = fit_channel(
            a,
            &e.k[c],
            s,
            f64::from(tune.dk1()),
            f64::from(tune.dk2()),
            g,
            rmax,
        );
        worst = worst.max(fit.max_err_deg);
        let n = if fit.k[3].abs() < 5e-6 && fit.k[4].abs() < 5e-6 {
            3
        } else {
            NK
        };
        let vals: Vec<String> = fit.k[..n].iter().map(|v| format!("{v:.5}")).collect();
        lines.push(format!("{name} = {}", vals.join(" ")));
    }
    (lines, worst as f32)
}

/// Terminal text: the tuning values and the equivalent `lens.conf` lines (a `[common]` block if both eyes agree).
pub fn report(lens: &Lens, tune: &Tune) -> (String, f32) {
    let (l, el) = eye_conf(lens, 0, tune);
    let (r, er) = eye_conf(lens, 1, tune);
    let worst = el.max(er);
    let mut s = format!(
        "tune: dk1 {:+.2} dk2 {:+.2} residual scale x{:.3} (applies to grid and swim) | fit error {:.3} deg\n",
        tune.dk1(),
        tune.dk2(),
        tune.scale(),
        worst
    );
    if l == r {
        s.push_str("  lens.conf, [common]:\n");
        for x in &l {
            s.push_str(&format!("    {x}\n"));
        }
    } else {
        for (name, v) in [("[left]", &l), ("[right]", &r)] {
            s.push_str(&format!("  lens.conf, {name}:\n"));
            for x in v {
                s.push_str(&format!("    {x}\n"));
            }
        }
    }
    if worst > WARN_DEG {
        s.push_str(&format!(
            "  WARNING: the 3-term fit is off by up to {worst:.2} deg (> {WARN_DEG}) over the visible range: use a smaller correction\n"
        ));
    }
    if lens.eye(0).fit_range > 0.0 || lens.eye(1).fit_range > 0.0 {
        s.push_str("  note: lens.conf has fit_range set; it is ignored by this fit\n");
    }
    (s, worst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_lens::config::parse;

    const CONF: &str = "fov = 50 50 44.5 44.5\nbase = 0.65307\nscale = 1.0\nk_g = 0.60779 -0.72489 0.58858\nk_r = 0.60779 -0.72489 0.58858\nk_b = 0.60779 -0.72489 0.58858\nca_r = 1.0045\nca_b = 0.995\n";

    fn lens() -> Lens {
        let (cfg, w) = parse(CONF, 3840.0 / 3552.0);
        assert!(w.is_empty(), "{w:?}");
        Lens::new(cfg)
    }

    #[test]
    fn tune_steps_and_limits() {
        let mut t = Tune::default();
        assert!(t.is_zero() && t.scale() == 1.0);
        t.add(3, -2);
        assert!((t.dk1() - 0.03).abs() < 1e-6 && (t.scale() - 0.99).abs() < 1e-6);
        t.add(100_000, -100_000);
        assert_eq!((t.dk, t.sc), (MAX_UNITS, -MAX_UNITS));
    }

    #[test]
    fn warp_identity_and_roundtrip() {
        let w = Tune::default().warp(0.65);
        assert!(w.is_identity());
        assert_eq!(w.apply((0.3, -0.2)), (0.3, -0.2));
        for (dk, sc) in [(3, 0), (-5, 2), (8, -4), (0, 6)] {
            let w = Tune { dk, sc, dk2: 0 }.warp(0.653);
            for t in [(0.0, 0.0), (0.2, -0.1), (-0.9, 0.5), (1.1, 0.0)] {
                let b = w.invert(w.apply(t));
                assert!(
                    (b.0 - t.0).abs() < 1e-4 && (b.1 - t.1).abs() < 1e-4,
                    "{t:?} {b:?}"
                );
            }
        }
    }

    #[test]
    fn warp_is_radial_and_matches_the_formula() {
        let w = Warp {
            s: 1.02,
            dk: 0.1,
            dk2: 0.0,
            g: 0.65,
        };
        let (x, y) = w.apply((0.65, 0.0));
        assert!((x - 0.65 * 1.02 * 1.1).abs() < 1e-6 && y == 0.0);
        // direction is kept
        let (a, b) = w.apply((0.3, 0.4));
        assert!((a / b - 0.75).abs() < 1e-5);
        // small angles: scale only
        let (x, _) = w.apply((1e-3, 0.0));
        assert!((x / 1e-3 - 1.02).abs() < 1e-3);
    }

    #[test]
    fn zero_tune_reproduces_the_config_exactly() {
        let l = lens();
        let (lines, err) = eye_conf(&l, 0, &Tune::default());
        assert!(err < 1e-4, "{err}");
        assert_eq!(lines[0], "scale = 1.00000");
        assert!(
            lines[1].starts_with("k_g = 0.60779 -0.72489 0.58858"),
            "{lines:?}"
        );
        assert!(
            lines[2].starts_with("k_r = 0.60779 -0.72489 0.58858"),
            "{lines:?}"
        );
    }

    #[test]
    fn pure_scale_changes_only_scale() {
        let l = lens();
        let t = Tune {
            dk: 0,
            sc: 4,
            dk2: 0,
        }; // x1.02
        let (lines, err) = eye_conf(&l, 1, &t);
        assert!(err < 1e-4);
        assert_eq!(lines[0], "scale = 1.02000");
        assert!(lines[1].starts_with("k_g = 0.60779 -0.72489 0.58858"));
    }

    #[test]
    fn dk_fit_matches_the_composed_mapping_within_a_tenth_of_a_degree() {
        let l = lens();
        for dk in [-5, -2, 1, 3, 5] {
            let t = Tune { dk, sc: 0, dk2: 0 };
            let (_, err) = eye_conf(&l, 0, &t);
            assert!(err < WARN_DEG, "dk {} err {err}", t.dk1());
        }
        // independent check of one case: sample the composed map and the fitted model by hand
        let e = l.eye(0);
        let t = Tune {
            dk: 3,
            sc: 2,
            dk2: 0,
        };
        let g = f64::from(e.base[1] * e.scale);
        let a = f64::from(e.base[1] * e.scale);
        let rmax = visible_radius(f64::from(e.base[1]), 1.0, &e.k[1], e.fov);
        let fit = fit_channel(
            a,
            &e.k[1],
            f64::from(t.scale()),
            f64::from(t.dk1()),
            0.0,
            g,
            rmax,
        );
        let warp = t.warp(g as f32);
        for i in 1..=20 {
            let r = rmax * f64::from(i) / 20.0;
            let tan = radial_tangent(f64::from(e.base[1]), 1.0, &e.k[1], r).0 as f32;
            let want = warp.apply((tan, 0.0)).0 as f64;
            let x = r * r;
            let got = f64::from(t.scale())
                * a
                * r
                * (1.0
                    + fit
                        .k
                        .iter()
                        .enumerate()
                        .map(|(j, k)| k * x.powi(j as i32 + 1))
                        .sum::<f64>());
            assert!(
                (want.atan() - got.atan()).to_degrees().abs() < 0.1,
                "r {r}: {want} vs {got}"
            );
        }
    }

    #[test]
    fn dk2_bump_shape_roundtrip_and_fit() {
        assert!((bump(BUMP_U) - 1.0).abs() < 1e-12 && bump(0.0) == 0.0);
        assert!(bump(1.8) < 0.03, "{}", bump(1.8));
        // at the bump peak the radial magnification changes by exactly dk2 (s = 1, dk = 0)
        let w = Tune {
            dk: 0,
            sc: 0,
            dk2: 3,
        }
        .warp(0.65);
        let t = 0.65 * BUMP_U as f32;
        let (x, _) = w.apply((t, 0.0));
        assert!((x / t - 1.03).abs() < 1e-5, "{}", x / t);
        // the edge hardly moves
        let (x, _) = w.apply((1.19, 0.0));
        assert!((x / 1.19 - 1.0).abs() < 0.001);
        for (dk, dk2) in [(0, 3), (2, -4), (-3, 8), (0, -10)] {
            let w = Tune { dk, sc: 1, dk2 }.warp(0.653);
            for t in [(0.0, 0.0), (0.2, -0.1), (-0.5, 0.3), (1.1, 0.0)] {
                let b = w.invert(w.apply(t));
                assert!(
                    (b.0 - t.0).abs() < 1e-4 && (b.1 - t.1).abs() < 1e-4,
                    "{t:?} {b:?}"
                );
            }
        }
        // the lens.conf fit follows it
        let l = lens();
        let (_, err) = eye_conf(
            &l,
            0,
            &Tune {
                dk: 0,
                sc: 0,
                dk2: 3,
            },
        );
        assert!(err < WARN_DEG, "{err}");
        let (lines, _) = eye_conf(
            &l,
            0,
            &Tune {
                dk: 0,
                sc: 0,
                dk2: 3,
            },
        );
        let (zero, _) = eye_conf(&l, 0, &Tune::default());
        assert_ne!(lines[1], zero[1]);
        let mut t = Tune::default();
        t.add2(7);
        t.add2(-100_000);
        assert_eq!(t.dk2, -MAX_UNITS);
    }

    #[test]
    fn large_correction_warns() {
        let (cfg, _) = parse(
            &CONF.replace(
                "k_g = 0.60779 -0.72489 0.58858",
                "k_g = 0.6 -0.7 0.6 -0.3 0.1",
            ),
            3840.0 / 3552.0,
        );
        let l = Lens::new(cfg);
        let (txt, err) = report(
            &l,
            &Tune {
                dk: -400,
                sc: 0,
                dk2: 0,
            },
        );
        assert!(err > WARN_DEG && txt.contains("WARNING"), "{err}\n{txt}");
        let (txt, err) = report(
            &l,
            &Tune {
                dk: 2,
                sc: 1,
                dk2: 0,
            },
        );
        assert!(err < WARN_DEG && !txt.contains("WARNING"), "{txt}");
        assert!(txt.contains("[common]") && txt.contains("k_b ="), "{txt}");
    }

    #[test]
    fn solve_solves() {
        let x = solve::<3>([
            [2.0, 1.0, 0.0, 0.0, 0.0, 4.0],
            [0.0, 3.0, 1.0, 0.0, 0.0, 9.0],
            [1.0, 0.0, 4.0, 0.0, 0.0, 13.0],
        ])
        .unwrap();
        assert!(
            (x[0] - 1.0).abs() < 1e-9 && (x[1] - 2.0).abs() < 1e-9 && (x[2] - 3.0).abs() < 1e-9
        );
        assert!(solve::<3>([[0.0; 6]; 3]).is_none());
    }
}
