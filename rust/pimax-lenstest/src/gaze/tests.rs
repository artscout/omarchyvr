use super::*;

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Reading looking exactly at the target from the head origin, then distorted by `f`.
fn look(t: &Target, f: &dyn Fn(V3) -> V3) -> Reading {
    Reading {
        dir: normalize(f(t.dir())),
        origin: [0.0; 3],
    }
}

fn same(d: V3) -> V3 {
    d
}

#[test]
fn target_directions_follow_the_axes() {
    let c = dir_from_az_el(0.0, 0.0);
    assert!(approx(c[2], -1.0, 1e-12) && approx(c[0], 0.0, 1e-12) && approx(c[1], 0.0, 1e-12));
    let r = dir_from_az_el(15.0, 0.0);
    assert!(r[0] > 0.25 && approx(r[1], 0.0, 1e-12) && r[2] < 0.0);
    assert!(approx(angle_deg(r, c), 15.0, 1e-9));
    let u = dir_from_az_el(0.0, 15.0);
    assert!(u[1] > 0.25 && approx(angle_deg(u, c), 15.0, 1e-9));
    let l = dir_from_az_el(-15.0, 0.0);
    assert!(l[0] < 0.0);
    for (az, el) in [(25.0, 20.0), (-35.0, 0.0), (12.5, -10.0)] {
        let d = dir_from_az_el(az, el);
        assert!(approx(len(d), 1.0, 1e-12));
        // tangent matches the lens-test convention (azimuth atan(tx), elevation from the horizontal)
        let (tx, ty) = tangent_of(d).unwrap();
        let (a2, e2) = crate::geometry::az_el_deg(tx as f32, ty as f32);
        assert!(
            approx(f64::from(a2), az, 1e-3) && approx(f64::from(e2), el, 1e-3),
            "{az} {el}"
        );
    }
    let p = target_point(0.0, 0.0);
    assert_eq!(p, [0.0, 0.0, -2.0]);
    assert!(approx(len(target_point(25.0, 20.0)), 2.0, 1e-12));
}

#[test]
fn angular_error_is_exact_for_small_and_large_angles() {
    assert!(approx(
        angle_deg([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        90.0,
        1e-12
    ));
    assert!(approx(
        angle_deg([0.0, 0.0, -1.0], [0.0, 0.0, 1.0]),
        180.0,
        1e-9
    ));
    let a = dir_from_az_el(0.0, 0.0);
    let b = dir_from_az_el(0.001, 0.0);
    assert!(approx(angle_deg(a, b), 0.001, 1e-9));
    assert!(angle_deg(a, a) < 1e-12);
}

#[test]
fn cursor_lies_on_the_targets_sphere() {
    let t = Target::new(25.0, 20.0, "t");
    let hit = plane_hit([0.0; 3], t.dir()).unwrap();
    for (h, p) in hit.iter().zip(t.point()) {
        assert!(approx(*h, p, 1e-9));
    }
    // an eye 3 cm to the right looking straight ahead puts the cursor 3 cm right of the head axis
    let h2 = plane_hit([0.03, 0.0, 0.0], [0.0, 0.0, -1.0]).unwrap();
    assert!(approx(h2[0], 0.03, 1e-12) && approx(h2[2], -2.0, 1e-12));
    assert!(plane_hit([0.0; 3], [0.0, 0.0, 1.0]).is_none());
    assert!(plane_hit([0.0; 3], [1.0, 0.0, -0.01]).is_none());
}

#[test]
fn axis_and_grid_target_sets() {
    let a = axis_targets();
    assert_eq!(a.len(), 5);
    assert_eq!((a[0].az, a[0].el), (0.0, 0.0));
    assert_eq!(
        (a[1].az, a[2].az, a[3].el, a[4].el),
        (15.0, -15.0, 15.0, -15.0)
    );
    for n in [13, 17] {
        let g = grid_targets(n);
        assert_eq!(g.len(), n);
        assert_eq!((g[0].az, g[0].el), (0.0, 0.0), "starts at the centre");
        for t in &g {
            assert!(t.az.abs() <= 35.0 && t.el.abs() <= 20.0, "{t:?}");
            assert!(
                t.az.abs() <= 25.0 || t.az.abs() == 35.0 && t.el == 0.0,
                "{t:?}"
            );
        }
        let mut seen: Vec<(i64, i64)> = g
            .iter()
            .map(|t| ((t.az * 10.0) as i64, (t.el * 10.0) as i64))
            .collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), n, "all points differ");
        assert!(g.iter().any(|t| t.az == 35.0) && g.iter().any(|t| t.az == -35.0));
        assert!(g.iter().any(|t| t.az == 25.0 && t.el == -20.0));
        // symmetric
        for t in &g {
            assert!(
                g.iter().any(|u| u.az == -t.az && u.el == t.el) || t.az == 0.0,
                "{t:?}"
            );
        }
        // a path, not jumps across the field: no hop longer than 36 deg
        for w in g.windows(2) {
            assert!(
                angle_deg(w[0].dir(), w[1].dir()) < 36.0,
                "{:?} {:?}",
                w[0],
                w[1]
            );
        }
    }
    // the 17-point grid is the 13-point grid plus four mid points
    let g13 = grid_targets(13);
    assert!(g13.iter().all(|t| t.az.abs() != 12.5));
    assert_eq!(
        grid_targets(17)
            .iter()
            .filter(|t| t.az.abs() == 12.5)
            .count(),
        4
    );
}

#[test]
fn regions_split_by_eccentricity() {
    let r = |az, el| region_of(&Target::new(az, el, ""));
    assert_eq!(r(0.0, 0.0), Region::Centre);
    assert_eq!(r(0.0, 10.0), Region::Centre);
    assert_eq!(r(12.5, 10.0), Region::Mid);
    assert_eq!(r(25.0, 0.0), Region::Mid);
    assert_eq!(r(0.0, 20.0), Region::Mid);
    assert_eq!(r(25.0, 20.0), Region::Edge);
    assert_eq!(r(35.0, 0.0), Region::Edge);
}

#[test]
fn stats_and_percentiles() {
    let s = stats(&[4.0, 1.0, 3.0, 2.0, 10.0]);
    assert_eq!((s.n, s.mean, s.median, s.max), (5, 4.0, 3.0, 10.0));
    assert!(approx(s.p95, 4.0 + 0.8 * 6.0, 1e-12));
    assert_eq!(stats(&[]), Stats::default());
    assert_eq!(percentile(&[7.0], 0.95), 7.0);
}

#[test]
fn perfect_tracker_has_zero_error_and_full_validity() {
    let t = Target::new(25.0, -20.0, "t");
    let r: Vec<Reading> = (0..70).map(|_| look(&t, &same)).collect();
    let res = compute_target(&t, &r, 70);
    assert!(res.err_deg < 1e-6 && res.bias_deg < 1e-6 && res.precision_deg < 1e-6);
    assert!(approx(res.valid_ratio, 1.0, 1e-12) && res.ok && res.used == 70);
}

#[test]
fn bias_noise_blinks_and_invalid_frames() {
    let t = Target::new(0.0, 0.0, "t");
    // constant 2 deg offset to the right
    let off = dir_from_az_el(2.0, 0.0);
    let mut r: Vec<Reading> = (0..60)
        .map(|_| Reading {
            dir: off,
            origin: [0.0; 3],
        })
        .collect();
    // 5 blink samples looking far away (rejected), the window had 80 frames (15 invalid)
    for _ in 0..5 {
        r.push(Reading {
            dir: dir_from_az_el(0.0, -40.0),
            origin: [0.0; 3],
        });
    }
    let res = compute_target(&t, &r, 80);
    assert_eq!((res.valid, res.used), (65, 60));
    assert!(approx(res.err_deg, 2.0, 1e-6) && approx(res.bias_deg, 2.0, 1e-6));
    assert!(approx(res.valid_ratio, 60.0 / 80.0, 1e-12));
    assert!(res.precision_deg < 1e-6, "no jitter among the kept samples");
    assert!(res.ok);
    // alternating +-0.5 deg jitter: precision (sample to sample) is the full 1 deg step
    let jit: Vec<Reading> = (0..40)
        .map(|i| Reading {
            dir: dir_from_az_el(if i % 2 == 0 { 0.5 } else { -0.5 }, 0.0),
            origin: [0.0; 3],
        })
        .collect();
    let res = compute_target(&t, &jit, 40);
    assert!(
        approx(res.precision_deg, 1.0, 1e-3),
        "{}",
        res.precision_deg
    );
    assert!(res.bias_deg < 1e-6, "symmetric jitter has no bias");
    assert!(approx(res.err_deg, 0.5, 1e-3));
    // too few samples: not ok; none at all: zeros
    let few = compute_target(&t, &jit[..5], 70);
    assert!(!few.ok);
    let none = compute_target(&t, &[], 70);
    assert!(!none.ok && none.used == 0 && none.valid_ratio == 0.0);
}

#[test]
fn target_direction_is_taken_from_the_gaze_origin() {
    // eye 6 cm to the right of the head origin looking straight ahead: the target at 2 m is 1.7 deg to its left
    let t = Target::new(0.0, 0.0, "t");
    let r = vec![
        Reading {
            dir: [0.0, 0.0, -1.0],
            origin: [0.06, 0.0, 0.0]
        };
        30
    ];
    let res = compute_target(&t, &r, 30);
    assert!(
        approx(res.err_deg, (0.06f64 / 2.0).atan().to_degrees(), 1e-6),
        "{}",
        res.err_deg
    );
}

fn means_for(flip: [bool; 3], swap: bool) -> [V3; 5] {
    let t = axis_targets();
    std::array::from_fn(|i| {
        let mut d = t[i].dir();
        if swap {
            d = [d[1], d[0], d[2]];
        }
        [
            if flip[0] { -d[0] } else { d[0] },
            if flip[1] { -d[1] } else { d[1] },
            if flip[2] { -d[2] } else { d[2] },
        ]
    })
}

#[test]
fn sign_flips_are_detected_for_every_combination() {
    for bits in 0..8u8 {
        let f = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
        let v = axis_verdict(&means_for(f, false));
        assert_eq!(v.flip_x, Some(f[0]), "{f:?}");
        assert_eq!(v.flip_y, Some(f[1]), "{f:?}");
        assert_eq!(v.flip_z, Some(f[2]), "{f:?}");
        assert!(!v.swapped && v.conclusive());
        assert_eq!(v.all_ok(), bits == 0);
        assert!(approx(v.span_x_deg, 30.0, 1e-6) && approx(v.span_y_deg, 30.0, 1e-6));
    }
}

#[test]
fn swapped_axes_and_no_movement_are_reported() {
    let v = axis_verdict(&means_for([false; 3], true));
    assert!(v.swapped && !v.all_ok());
    let still = axis_verdict(&[[0.0, 0.0, -1.0]; 5]);
    assert_eq!((still.flip_x, still.flip_y), (None, None));
    assert_eq!(still.flip_z, Some(false));
    assert!(!still.conclusive());
    // tiny movement (1 deg for 15 deg targets) does not decide
    let small: [V3; 5] = std::array::from_fn(|i| {
        let t = axis_targets();
        dir_from_az_el(t[i].az / 15.0, t[i].el / 15.0)
    });
    assert_eq!(axis_verdict(&small).flip_x, None);
}

#[test]
fn verdict_text_names_the_env_variable_to_toggle() {
    let v = axis_verdict(&means_for([true, false, true], false));
    let l = v.lines([false, false, true]).join("\n");
    assert!(
        l.contains("PIMAX_EYE_FLIP_X=1 in the service environment (it is 0 now)"),
        "{l}"
    );
    assert!(l.contains("also swaps which eye is left"));
    assert!(
        l.contains("PIMAX_EYE_FLIP_Z=0 in the service environment (it is 1 now)"),
        "{l}"
    );
    assert!(l.contains("axis y: ok"));
    let h = v.hud([false, false, true]).join("|");
    assert!(
        h.contains("SET PIMAX_EYE_FLIP_X=1") && h.contains("SET PIMAX_EYE_FLIP_Z=0"),
        "{h}"
    );
    let ok = axis_verdict(&means_for([false; 3], false));
    assert_eq!(
        ok.hud([false, false, true]),
        vec!["AXES OK: RIGHT +X, UP +Y, FORWARD -Z"]
    );
}

/// Fit points from wanted tangents through the device model `m` (gaze = m(want)).
fn points(m: &dyn Fn(f64, f64) -> (f64, f64), targets: &[Target]) -> Vec<FitPoint> {
    targets
        .iter()
        .map(|t| {
            let want = tangent_of(t.dir()).unwrap();
            FitPoint {
                gaze: m(want.0, want.1),
                want,
            }
        })
        .collect()
}

#[test]
fn affine_fit_recovers_a_known_affine() {
    // the tracker reads 7 % too wide, 4 % too narrow vertically, shifted and a little sheared
    let m = |u: f64, v: f64| (1.07 * u + 0.02 * v + 0.03, 0.96 * v - 0.015 * u - 0.02);
    let pts = points(&m, &grid_targets(17));
    let rep = fit(&pts);
    let chosen = rep.chosen_model().expect("a correction is offered");
    assert_eq!(
        chosen.name,
        "affine",
        "{:?}",
        rep.models
            .iter()
            .map(|m| (&m.name, m.loo.mean))
            .collect::<Vec<_>>()
    );
    assert!(
        chosen.train.max < 1e-6 && chosen.loo.max < 1e-6,
        "{:?}",
        chosen.train
    );
    assert!(rep.before.mean > 1.0);
    // the fitted map is the inverse of the device model
    let c = &chosen.correction;
    for p in &pts {
        let (u, v) = c.map_tangent(p.gaze.0, p.gaze.1);
        assert!(approx(u, p.want.0, 1e-7) && approx(v, p.want.1, 1e-7));
    }
    // quadratic terms of an affine fit are exactly zero
    assert!(c.cx[3..].iter().chain(c.cy[3..].iter()).all(|x| *x == 0.0));
    // and through Correction::apply on directions
    let d = dir_of(m(0.3, 0.1).0, m(0.3, 0.1).1);
    let fixed = tangent_of(c.apply(d)).unwrap();
    assert!(approx(fixed.0, 0.3, 1e-7) && approx(fixed.1, 0.1, 1e-7));
}

#[test]
fn quadratic_error_chooses_the_quadratic_model() {
    // barrel-like edge error: u grows with u^3-ish; model it with u*u*sign terms (quadratic in the features)
    let m = |u: f64, v: f64| {
        (
            u + 0.25 * u * u + 0.1 * u * v,
            v - 0.2 * v * v + 0.15 * u * u,
        )
    };
    let pts = points(&m, &grid_targets(17));
    let rep = fit(&pts);
    let chosen = rep.chosen_model().expect("quadratic offered");
    assert_eq!(chosen.name, "quadratic");
    // the data is not exactly invertible by a quadratic, but it is far better than affine and none
    assert!(
        chosen.loo.mean < 0.5 * rep.models[0].loo.mean,
        "{:?}",
        rep.models.iter().map(|m| m.loo.mean).collect::<Vec<_>>()
    );
    assert!(chosen.loo.mean < 0.5 * rep.before.mean);
    // with only 13 points quadratic is still fitted (13 >= 12)
    assert_eq!(fit(&points(&m, &grid_targets(13))).models.len(), 2);
}

#[test]
fn a_perfect_or_noise_only_tracker_gets_no_correction() {
    let id = |u: f64, v: f64| (u, v);
    let rep = fit(&points(&id, &grid_targets(17)));
    assert!(rep.chosen.is_none());
    assert!(rep.before.mean < 1e-9);
    // tiny random-looking errors that no smooth map can follow
    let mut k = 0usize;
    let noisy = points(&id, &grid_targets(17))
        .into_iter()
        .map(|mut p| {
            k += 1;
            p.gaze.0 += if (k * 7 + k / 3) & 1 == 0 {
                0.002
            } else {
                -0.002
            };
            p.gaze.1 += if (k * 5 + k / 2) & 1 == 0 {
                0.002
            } else {
                -0.002
            };
            p
        })
        .collect::<Vec<_>>();
    let rep = fit(&noisy);
    assert!(
        rep.chosen.is_none(),
        "{:?}",
        rep.models.iter().map(|m| m.loo.mean).collect::<Vec<_>>()
    );
    // too few points: no models at all
    assert!(fit(&noisy[..5]).models.is_empty());
}

#[test]
fn solver_handles_singular_systems() {
    assert!(solve(vec![vec![1.0, 2.0], vec![2.0, 4.0]], vec![1.0, 2.0]).is_none());
    let x = solve(vec![vec![2.0, 1.0], vec![1.0, 3.0]], vec![3.0, 5.0]).unwrap();
    assert!(approx(x[0], 0.8, 1e-12) && approx(x[1], 1.4, 1e-12));
    // all points the same: singular fit is refused, not NaN
    let p = FitPoint {
        gaze: (0.1, 0.1),
        want: (0.0, 0.0),
    };
    assert!(fit_coeffs(&[p; 8], 3).is_some()); // ridge keeps it solvable
    assert!(fit_coeffs(&[p; 2], 3).is_none());
}

/// Run the whole procedure against a simulated tracker at 72 Hz.
fn run_test(t: &mut GazeTest, dev: &dyn Fn(V3) -> V3) -> Vec<Effect> {
    let mut fx = Vec::new();
    fx.extend(t.command(Cmd::Next));
    let mut guard = 0;
    while t.measuring() {
        // the simulated wearer looks at the displayed target
        let target = match t.phase {
            Phase::Axis(i) => t.axis[i].clone(),
            Phase::Grid(i) => t.grid[i].clone(),
            _ => unreachable!(),
        };
        fx.extend(t.frame(1.0 / 72.0, Some(look(&target, dev))));
        guard += 1;
        assert!(guard < 100_000);
        if t.phase == Phase::AxisDone {
            fx.extend(t.command(Cmd::Next));
        }
    }
    fx
}

#[test]
fn full_run_with_a_perfect_tracker() {
    let mut t = GazeTest::new(17, [false, false, true]);
    assert_eq!(t.phase, Phase::Intro);
    let fx = run_test(&mut t, &same);
    assert_eq!(t.phase, Phase::Done);
    assert!(t.verdict.as_ref().unwrap().all_ok());
    let r = t.report.as_ref().unwrap();
    assert_eq!((r.targets, r.targets_ok), (17, 17));
    assert!(r.overall.max < 1e-6 && r.valid_ratio > 0.99);
    assert!(t.candidate.is_none(), "nothing to correct");
    let printed: Vec<&String> = fx
        .iter()
        .filter_map(|e| {
            if let Effect::Print(s) = e {
                Some(s)
            } else {
                None
            }
        })
        .collect();
    assert!(printed.iter().any(|s| s.contains("signs are right")));
    assert!(printed
        .iter()
        .any(|s| s.contains("eye-accuracy: 17/17 targets")));
    let json = fx
        .iter()
        .find_map(|e| {
            if let Effect::SaveResults(j) = e {
                Some(j)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        json.contains("\"grid_targets\"") && json.contains("\"axis_check\": {\"flip_x\": false")
    );
    // each target takes settle + collect seconds: 22 targets
    // accept without a candidate says so and writes nothing
    let fx = t.command(Cmd::Accept);
    assert!(matches!(&fx[..], [Effect::Print(s)] if s.contains("no correction")));
    // live cursor: next toggles, marks follow the gaze
    t.command(Cmd::Next);
    assert_eq!(t.phase, Phase::Cursor);
    let m = t.marks(Some(&Reading {
        dir: dir_from_az_el(10.0, 5.0),
        origin: [0.0; 3],
    }));
    assert_eq!(
        m.len(),
        1,
        "only the raw cursor when there is no correction"
    );
    assert!(matches!(m[0].kind, MarkKind::Ring(_)));
    assert!(t.marks(None).is_empty());
}

#[test]
fn full_run_with_a_distorted_tracker_offers_and_writes_a_correction() {
    // the device sees angles 8 % too wide horizontally and is shifted 1.5 deg up
    let dev = |d: V3| {
        let (u, v) = tangent_of(d).unwrap();
        let (u2, v2) = (u * 1.08, v + 0.026);
        dir_of(u2, v2)
    };
    let mut t = GazeTest::new(13, [false, false, true]);
    let fx = run_test(&mut t, &dev);
    assert!(fx.iter().any(|e| matches!(e, Effect::SaveResults(_))));
    let r = t.report.as_ref().unwrap();
    assert!(r.overall.mean > 1.0);
    let after = r.after.as_ref().expect("a correction was chosen");
    assert!(after.mean < 0.05, "{after:?}");
    let c = t.candidate.clone().unwrap();
    assert_eq!(c.flips, Some([false, false, true]));
    assert!(
        approx(c.cx[1], 1.0 / 1.08, 1e-6) && approx(c.cy[0], -0.026, 1e-6),
        "{c:?}"
    );
    // the cursors: raw and corrected, the corrected one lands on the real gaze point
    let look_at = dir_from_az_el(10.0, 5.0);
    let m = t.marks(Some(&Reading {
        dir: dev(look_at),
        origin: [0.0; 3],
    }));
    assert_eq!(m.len(), 2);
    let want = plane_hit([0.0; 3], look_at).unwrap();
    assert!(len(sub(m[1].pos, want)) < 1e-6, "{:?} {:?}", m[1].pos, want);
    // Y writes the TOML with notes; it parses back to the same map
    let fx = t.command(Cmd::Accept);
    let Effect::WriteCorrection(text) = &fx[0] else {
        panic!("{fx:?}")
    };
    assert!(text.contains("leave-one-out mean error") && text.contains("eye-accuracy: 13/13"));
    let back = Correction::parse(text).unwrap();
    assert!(approx(back.cx[1], c.cx[1], 1e-8) && back.flips == c.flips);
    assert!(t.hud(true).join(" ").contains("CORRECTION SAVED"));
}

#[test]
fn flipped_x_tracker_is_caught_by_the_axis_check() {
    let dev = |d: V3| [-d[0], d[1], d[2]];
    let mut t = GazeTest::new(13, [false, false, true]);
    t.command(Cmd::Next);
    let mut fx = Vec::new();
    while matches!(t.phase, Phase::Axis(_)) {
        let Phase::Axis(i) = t.phase else {
            unreachable!()
        };
        let tg = t.axis[i].clone();
        fx.extend(t.frame(1.0 / 72.0, Some(look(&tg, &dev))));
    }
    assert_eq!(t.phase, Phase::AxisDone);
    let v = t.verdict.clone().unwrap();
    assert_eq!(
        (v.flip_x, v.flip_y, v.flip_z),
        (Some(true), Some(false), Some(false))
    );
    let text = fx
        .iter()
        .filter_map(|e| {
            if let Effect::Print(s) = e {
                Some(s.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("PIMAX_EYE_FLIP_X=1"), "{text}");
    assert!(text.contains("fix the flips"));
    assert!(t
        .hud(true)
        .join(" ")
        .contains("AXIS X MIRRORED: SET PIMAX_EYE_FLIP_X=1"));
}

#[test]
fn missing_and_invalid_gaze_is_handled() {
    let mut t = GazeTest::new(13, [false, false, true]);
    t.command(Cmd::Next);
    // eyes closed throughout: every target has no data, the axis check is not decided, nothing panics
    for _ in 0..2000 {
        t.frame(1.0 / 72.0, None);
        if t.phase == Phase::AxisDone {
            break;
        }
    }
    assert_eq!(t.phase, Phase::AxisDone);
    assert!(t.verdict.is_none());
    assert!(t.hud(false).join(" ").contains("TOO FEW SAMPLES"));
    // a huge frame time counts for at most MAX_DT
    let mut t = GazeTest::new(13, [false; 3]);
    t.command(Cmd::Next);
    t.frame(5.0, None);
    assert!(approx(t.t, MAX_DT, 1e-12));
    // Next in the middle of a target does nothing; restart goes back to the start
    t.command(Cmd::Next);
    assert_eq!(t.phase, Phase::Axis(0));
    t.command(Cmd::Restart);
    assert_eq!(t.phase, Phase::Intro);
    assert!(t.axis_results().is_empty() && t.report.is_none());
    // accept before the grid is done
    assert!(
        matches!(&t.command(Cmd::Accept)[..], [Effect::Print(s)] if s.contains("finish the accuracy grid"))
    );
}

#[test]
fn unavailable_extension_shows_a_message_and_ignores_input() {
    let mut t = GazeTest::new(17, [false; 3]);
    t.unavailable = Some("the runtime does not offer XR_EXT_eye_gaze_interaction".to_string());
    assert!(t.command(Cmd::Next).is_empty());
    assert_eq!(t.phase, Phase::Intro);
    let h = t.hud(false).join("\n");
    assert!(
        h.contains("EYE GAZE NOT AVAILABLE") && h.contains("PIMAX_EYE=1"),
        "{h}"
    );
}

#[test]
fn service_side_correction_blocks_accepting() {
    let dev = |d: V3| {
        let (u, v) = tangent_of(d).unwrap();
        dir_of(u * 1.1, v)
    };
    let mut t = GazeTest::new(13, [false, false, true]);
    t.service_correction = Some("/home/x/eye-correction.toml".to_string());
    run_test(&mut t, &dev);
    assert!(t.candidate.is_some());
    let fx = t.command(Cmd::Accept);
    assert!(
        matches!(&fx[..], [Effect::Print(s)] if s.contains("not saved") && s.contains("PIMAX_EYE_CORRECTION=none")),
        "{fx:?}"
    );
}

#[test]
fn shrinking_dot_and_hud_texts() {
    assert!(approx(dot_radius_deg(0.0), 1.2, 1e-12));
    assert!(approx(dot_radius_deg(SETTLE_S), 0.3, 1e-12));
    assert!(approx(dot_radius_deg(5.0), 0.3, 1e-12));
    assert!(dot_radius_deg(0.3) < dot_radius_deg(0.1));
    let mut t = GazeTest::new(17, [false; 3]);
    for l in t.hud(true).iter().chain(t.hud(false).iter()) {
        assert!(l.chars().count() < 70, "{l}");
    }
    t.command(Cmd::Next);
    assert_eq!(t.hud(true), vec!["AXIS CHECK 1/5   Q QUIT".to_string()]);
    let m = t.marks(Some(&Reading {
        dir: [0.0, 0.0, -1.0],
        origin: [0.0; 3],
    }));
    assert_eq!(m.len(), 1, "only the target while measuring");
    assert_eq!(m[0].kind, MarkKind::Disc);
    assert_eq!(m[0].pos, [0.0, 0.0, -2.0]);
}

#[test]
fn json_is_balanced_and_has_the_numbers() {
    let t = Target::new(25.0, 0.0, "x");
    let r: Vec<Reading> = (0..40).map(|_| look(&t, &same)).collect();
    let res = vec![compute_target(&t, &r, 50)];
    let rep = build_report(&res);
    let j = results_json(&JsonInput {
        time_unix: 1,
        flips: [false, true, false],
        service_correction: Some("a \"b\"\n"),
        axis: &res,
        verdict: None,
        grid: &res,
        report: &rep,
        correction: None,
    });
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for c in j.chars() {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, '\\') => esc = true,
                (false, '"') => in_str = false,
                _ => {}
            }
        } else {
            match c {
                '"' => in_str = true,
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0);
        }
    }
    assert_eq!((depth, in_str), (0, false));
    assert!(j.contains("\"env_flip_xyz\": [false, true, false]"));
    assert!(j.contains("\"service_correction\": \"a \\\"b\\\"\\n\""));
    assert!(j.contains("\"valid_ratio\": 0.8000"), "{j}");
    assert!(j.contains("\"region\": \"mid\""));
    assert!(j.contains("\"axis_check\": null") && j.contains("\"correction_candidate\": null"));
}

#[test]
fn summary_line_has_the_requested_fields() {
    let mut res = Vec::new();
    for t in grid_targets(13) {
        let r: Vec<Reading> = (0..60)
            .map(|_| Reading {
                dir: dir_from_az_el(t.az + 1.0, t.el),
                origin: [0.0; 3],
            })
            .collect();
        res.push(compute_target(&t, &r, 72));
    }
    let rep = build_report(&res);
    let s = &rep.summary;
    for k in [
        "mean",
        "median",
        "p95",
        "max",
        "precision",
        "valid",
        "centre",
        "mid",
        "edge",
    ] {
        assert!(s.contains(k), "{k}: {s}");
    }
    assert!(
        s.contains("13/13 targets") && s.contains("valid 83%"),
        "{s}"
    );
    assert!(rep.regions.iter().all(|g| g.targets > 0));
    assert!(rep.text().contains("targets with data: 13/13"));
}

#[test]
fn stamp_formats_utc() {
    assert_eq!(stamp(0), "19700101-000000");
    assert_eq!(stamp(1_000_000_000), "20010909-014640");
    assert_eq!(
        stamp(1_781_000_000 + 86_400 * 366),
        stamp(1_781_000_000 + 86_400 * 366)
    );
    assert_eq!(stamp(951_782_400), "20000229-000000");
}

#[test]
fn writing_keeps_backups_and_never_loses_the_old_file() {
    let dir = std::env::temp_dir().join(format!("pimax-gaze-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = dir.join("sub/eye-correction.toml");
    // first write: creates the directory, no backup
    assert_eq!(write_with_backup(&p, "one", "S").unwrap(), None);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "one");
    // second: old content kept
    let b = write_with_backup(&p, "two", "S").unwrap().unwrap();
    assert!(b.to_string_lossy().ends_with("eye-correction.toml.bak-S"));
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "one");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
    // same stamp again: numbered, the first backup is not overwritten
    let b2 = write_with_backup(&p, "three", "S").unwrap().unwrap();
    assert!(b2.to_string_lossy().ends_with("bak-S-1"));
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "one");
    assert_eq!(std::fs::read_to_string(&b2).unwrap(), "two");
    // execute() writes through the same path and reports the backup
    let msg = execute(&Effect::WriteCorrection("four".into()), None, Some(&p));
    assert!(
        msg.contains("previous file kept as") && msg.contains("ninja -C"),
        "{msg}"
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "four");
    // results go to --gaze-out as a directory or a file
    let out = dir.join("res");
    let m = execute(
        &Effect::SaveResults("{\"time_unix\": 0,\n}".into()),
        Some(&out),
        None,
    );
    assert!(m.starts_with("results saved: "), "{m}");
    let saved: Vec<_> = std::fs::read_dir(&out).unwrap().collect();
    assert_eq!(saved.len(), 1);
    let text = std::fs::read_to_string(saved[0].as_ref().unwrap().path()).unwrap();
    assert!(!text.contains("\"time_unix\": 0,"), "{text}");
    assert_eq!(
        results_path(Some(Path::new("/x/y.json")), "S"),
        PathBuf::from("/x/y.json")
    );
    assert_eq!(
        results_path(Some(Path::new("/x")), "S"),
        PathBuf::from("/x/eye-accuracy-S.json")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
