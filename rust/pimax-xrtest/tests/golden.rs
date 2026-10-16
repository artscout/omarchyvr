//! The analysis against joint sets produced by the REAL Monado hand simulation (tests/golden/hand_joints.txt,
//! driven by the Rust curl mapping exactly like the driver does).
use pimax_hands::pose::{grip_to_wrist, Hand};
use pimax_xrtest::joints::*;

struct Case {
    name: String,
    hand: Hand,
    curls: [f64; 5],
    joints: Joints,
}

fn load() -> Vec<Case> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/hand_joints.txt"
    ))
    .unwrap();
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    let mut cases = vec![];
    while let Some(head) = lines.next() {
        let t: Vec<&str> = head.split_whitespace().collect();
        assert_eq!(t[0], "case", "{head}");
        let ci = t.iter().position(|x| *x == "curls").unwrap();
        let mut curls = [0.0; 5];
        for (k, c) in curls.iter_mut().enumerate() {
            *c = t[ci + 1 + k].parse().unwrap();
        }
        let mut joints = [Joint::default(); JOINT_COUNT];
        for j in &mut joints {
            let v: Vec<f64> = lines
                .next()
                .unwrap()
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            *j = Joint {
                p: [v[0], v[1], v[2]],
                q: [v[3], v[4], v[5], v[6]],
            };
        }
        cases.push(Case {
            name: t[1].to_string(),
            hand: if t[2] == "L" { Hand::Left } else { Hand::Right },
            curls,
            joints,
        });
    }
    cases
}

#[test]
fn golden_file_has_every_case_for_both_hands() {
    let cases = load();
    assert_eq!(cases.len(), 16);
    assert_eq!(cases.iter().filter(|c| c.hand == Hand::Left).count(), 8);
}

#[test]
fn flexion_matches_what_the_simulation_was_asked_for() {
    for c in load() {
        for (k, f) in Finger::ALL.iter().enumerate() {
            let measured = flexion(&c.joints, *f);
            let want = expected_flexion(*f, c.curls[k]);
            assert!(
                (measured - want).abs() < 0.01,
                "{} {:?} {}: measured {measured:.4} want {want:.4}",
                c.name,
                c.hand,
                f.name()
            );
        }
    }
}

#[test]
fn curl_estimates_recover_the_curls() {
    for c in load() {
        let est = curl_estimates(&c.joints);
        for (k, f) in Finger::ALL.iter().enumerate() {
            // the thumb is ambiguous below ~0.25 (see curl_estimate)
            let tol = if *f == Finger::Thumb && c.curls[k] < 0.3 {
                0.15
            } else {
                0.01
            };
            assert!(
                (est[k] - c.curls[k]).abs() < tol,
                "{} {:?} {}: est {:.3} curl {:.3}",
                c.name,
                c.hand,
                f.name(),
                est[k],
                c.curls[k]
            );
        }
    }
}

#[test]
fn curls_order_fingers_correctly() {
    // grip_full bends little/ring/middle only, trigger_full the index only, thumb_btn1 the thumb only
    let cases = load();
    let get = |n: &str| {
        cases
            .iter()
            .find(|c| c.name == n && c.hand == Hand::Left)
            .unwrap()
    };
    let rest = curl_estimates(&get("rest").joints);
    let grip = curl_estimates(&get("grip_full").joints);
    let trig = curl_estimates(&get("trigger_full").joints);
    let thumb = curl_estimates(&get("thumb_btn1").joints);
    for k in 0..3 {
        assert!(grip[k] > rest[k] + 0.5, "lower finger {k}");
    }
    assert!((grip[3] - rest[3]).abs() < 0.01 && (grip[4] - rest[4]).abs() < 0.2);
    assert!(trig[3] > rest[3] + 0.5);
    for k in [0, 1, 2] {
        assert!((trig[k] - rest[k]).abs() < 0.01);
    }
    assert!(thumb[4] > rest[4] + 0.2);
    for k in 0..4 {
        assert!((thumb[k] - rest[k]).abs() < 0.01);
    }
}

#[test]
fn grip_rule_applied_to_the_real_joints_gives_the_offset_used_by_the_driver() {
    // The wrist (joint 1) is the origin of the simulated hand; the grip derived from the real joints must equal
    // grip_in_wrist = inverse of grip_to_wrist, for every finger pose (it only uses palm, index/ring proximal).
    for c in load() {
        let grip = grip_from_joint_set(&c.joints, c.hand);
        let want = grip_to_wrist(c.hand).inverse();
        let (dp, da) = pose_error(&grip, &want);
        assert!(
            dp < 0.0005,
            "{} {:?}: grip position off by {:.2} mm",
            c.name,
            c.hand,
            dp * 1000.0
        );
        assert!(
            da < 1.0_f64.to_radians(),
            "{} {:?}: grip rotation off by {:.2} deg",
            c.name,
            c.hand,
            da.to_degrees()
        );
    }
}

#[test]
fn the_wrist_is_the_origin() {
    for c in load() {
        assert!(c.joints[WRIST].p.iter().all(|v| v.abs() < 1e-6));
    }
}
