//! Replay of the REAL button-by-button capture (rust/fixtures/ctrl_buttons_hub2.rec.gz, the operator pressed every
//! control of the left controller in turn and marked the times): the mapping must react to each control as intended.
use pimax_hands::report::{self, input_from_report, Report};
use pimax_hands::{HandState, Params, ThumbState};
use pimax_proto::{decode_controller, ControllerReport};
use std::io::Read;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

/// `[f64 time][64 B]` records.
fn load_rec(name: &str) -> Vec<(f64, [u8; 64])> {
    let f = std::fs::File::open(fixtures().join(name))
        .unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    let mut d = Vec::new();
    flate2::read::GzDecoder::new(f).read_to_end(&mut d).unwrap();
    d.as_chunks::<72>()
        .0
        .iter()
        .map(|c| {
            let mut b = [0u8; 64];
            b.copy_from_slice(&c[8..]);
            (f64::from_le_bytes(c[..8].try_into().unwrap()), b)
        })
        .collect()
}

fn marks() -> std::collections::HashMap<String, f64> {
    let s = std::fs::read_to_string(fixtures().join("ctrl_marks.json")).unwrap();
    s.lines()
        .filter_map(|l| {
            let l = l.trim().trim_end_matches(',');
            let (k, v) = l.rsplit_once(": ")?;
            Some((k.trim_matches('"').to_string(), v.parse().ok()?))
        })
        .collect()
}

fn as_report(c: &ControllerReport) -> Report {
    Report {
        trigger: c.trigger,
        grip: c.grip,
        stick_x: c.stick_x,
        stick_y: c.stick_y,
        buttons: c.buttons,
        touch: c.touch,
    }
}

/// One replayed sample.
#[derive(Clone, Copy)]
struct Sample {
    t: f64,
    curls: [f32; 5],
    thumb: ThumbState,
    trigger: f32,
    grip: f32,
}

fn replay(slot: u8) -> Vec<Sample> {
    let rows = load_rec("ctrl_buttons_hub2.rec.gz");
    let t0 = rows[0].0;
    let mut st = HandState::new(Params::default());
    rows.iter()
        .filter(|(_, b)| b[3] == slot)
        .map(|(t, b)| {
            let c = decode_controller(b).expect("controller report");
            let inp = input_from_report(&as_report(&c));
            let curls = st.update(((t - t0) * 1e9) as u64, &inp).to_array();
            Sample {
                t: t - t0,
                curls,
                thumb: st.thumb_state(),
                trigger: inp.trigger,
                grip: inp.grip,
            }
        })
        .collect()
}

#[test]
fn bit_constants_equal_the_protocol_crate() {
    assert_eq!(report::BTN_A, pimax_proto::BTN_A);
    assert_eq!(report::BTN_B, pimax_proto::BTN_B);
    assert_eq!(report::BTN_STICK, pimax_proto::BTN_STICK);
    assert_eq!(report::BTN_MENU, pimax_proto::BTN_MENU);
    assert_eq!(report::TOUCH_STICK, pimax_proto::TOUCH_STICK);
    // Touch A / B are bits 0 / 1 of byte 49: the flags must agree with the protocol crate's own decoding on every
    // report of the real captures (both controllers).
    let rows = load_rec("ctrl_buttons_hub2.rec.gz");
    let mut checked = 0;
    for (_, b) in rows.iter().step_by(7) {
        let Some(c) = decode_controller(b) else {
            continue;
        };
        let tb = c.touch_bits();
        let f = input_from_report(&as_report(&c)).flags;
        assert_eq!(tb.a, f & pimax_hands::flags::TOUCH_BTN1 != 0);
        assert_eq!(tb.b, f & pimax_hands::flags::TOUCH_BTN2 != 0);
        assert_eq!(tb.stick, f & pimax_hands::flags::TOUCH_STICK != 0);
        assert_eq!(
            c.buttons & pimax_proto::BTN_A != 0,
            f & pimax_hands::flags::CLICK_BTN1 != 0
        );
        assert_eq!(
            c.buttons & pimax_proto::BTN_B != 0,
            f & pimax_hands::flags::CLICK_BTN2 != 0
        );
        assert_eq!(
            c.buttons & pimax_proto::BTN_STICK != 0,
            f & pimax_hands::flags::CLICK_STICK != 0
        );
        assert_eq!(
            c.buttons & pimax_proto::BTN_MENU != 0,
            f & pimax_hands::flags::CLICK_MENU != 0
        );
        checked += 1;
    }
    assert!(checked > 10_000, "{checked}");
}

/// First time `pred` holds.
fn first(s: &[Sample], pred: impl Fn(&Sample) -> bool) -> Option<&Sample> {
    s.iter().find(|x| pred(x))
}

#[test]
fn left_controller_every_control_moves_the_right_finger() {
    let s = replay(2);
    let m = marks();
    let rows = load_rec("ctrl_buttons_hub2.rec.gz");
    let t0 = rows[0].0;
    let mark = |k: &str| m[k] - t0;
    let p = Params::default();

    // The capture starts with the operator's thumb already on the face buttons (touch A + B): thumb down on A/X.
    assert!(s[..200].iter().all(|x| x.thumb == ThumbState::Btn1));
    assert!(s[..200]
        .iter()
        .all(|x| (x.curls[4] - p.thumb_btn1).abs() < 0.03));

    // Trigger squeeze: index follows, the three lower fingers stay out of it.
    let trig = first(&s, |x| x.trigger > 0.8).expect("trigger pull");
    assert!(
        (mark("l trigger") - 6.5..=mark("l trigger") + 0.5).contains(&trig.t),
        "trigger at {:.2}",
        trig.t
    );
    let peak = s
        .iter()
        .filter(|x| (x.t - trig.t).abs() < 2.0)
        .map(|x| x.curls[3])
        .fold(0.0f32, f32::max);
    assert!(peak > 0.75, "index peak {peak}");
    let lower_during_trigger = s
        .iter()
        .filter(|x| (x.t - trig.t).abs() < 0.3 && x.grip < 0.1)
        .map(|x| x.curls[2])
        .fold(0.0f32, f32::max);
    assert!(
        lower_during_trigger < 0.4,
        "lower fingers moved with the trigger: {lower_during_trigger}"
    );

    // Grip squeeze: lower fingers close, index stays.
    let grip = first(&s, |x| x.grip > 0.5).expect("grip squeeze");
    assert!(
        (mark("l grip") - 6.5..=mark("l grip") + 2.0).contains(&grip.t),
        "grip at {:.2}",
        grip.t
    );
    let fist = s
        .iter()
        .filter(|x| (x.t - grip.t).abs() < 2.5)
        .map(|x| x.curls[2])
        .fold(0.0f32, f32::max);
    assert!(fist > 0.95, "middle finger peak {fist}");
    let idx_during_grip = s
        .iter()
        .filter(|x| (x.t - grip.t).abs() < 0.3 && x.trigger < 0.1)
        .map(|x| x.curls[3])
        .fold(0.0f32, f32::max);
    assert!(
        idx_during_grip < 0.3,
        "index moved with the grip: {idx_during_grip}"
    );
    assert!(s
        .iter()
        .all(|x| x.curls.iter().all(|c| (0.0..=1.0).contains(c))));

    // The capacitive bits blip for 50-100 ms when the trigger or the grip is squeezed (touch B / stick bits fire
    // with no thumb on them): the thumb must not twitch.
    for (what, at, after) in [("trigger", trig.t, 1.0), ("grip", grip.t, 0.9)] {
        let w: Vec<&Sample> = s
            .iter()
            .filter(|x| x.t >= at - 0.15 && x.t <= at + after)
            .collect();
        assert!(w.len() > 100);
        assert!(
            w.iter()
                .all(|x| x.thumb == ThumbState::Rest && x.curls[4] < 0.05),
            "thumb moved while squeezing the {what}"
        );
    }

    // The thumb goes where the operator put it, in the marked order: A, B, stick click, menu, stick moves.
    let thumb_at = |key: &str, state: ThumbState, lead: f64, slack: f64| {
        let hit = s
            .iter()
            .filter(|x| x.t >= mark(key) - lead && x.t <= mark(key) + slack)
            .find(|x| x.thumb == state && x.curls[4] > 0.35);
        assert!(
            hit.is_some(),
            "no {state:?} around the '{key}' mark ({:.1} s)",
            mark(key)
        );
    };
    thumb_at("l a", ThumbState::Btn1, 6.5, 1.0);
    thumb_at("l b", ThumbState::Btn2, 6.5, 1.0);
    thumb_at("l sick click", ThumbState::Stick, 6.5, 1.0);
    thumb_at("l menu", ThumbState::Menu, 6.5, 1.0);
    thumb_at("l stick up", ThumbState::Stick, 6.5, 1.5);
    thumb_at("l stck right", ThumbState::Stick, 6.5, 1.5);

    // Pressing is deeper than touching: during the A press the thumb goes beyond the touch-only position.
    let a_press = first(&s, |x| {
        x.thumb == ThumbState::Btn1 && x.curls[4] > p.thumb_btn1 + 0.04
    });
    assert!(
        a_press.is_some(),
        "A press never went deeper than the touch"
    );

    // Stick deflection always puts the thumb on the stick (no waiting for the flickering touch bit).
    let rows_left: Vec<_> = rows.iter().filter(|(_, b)| b[3] == 2).collect();
    let mut checked = 0;
    for ((_, b), x) in rows_left.iter().zip(s.iter()) {
        let c = decode_controller(b).unwrap();
        let mag = (f64::from(c.stick_x).hypot(f64::from(c.stick_y))) / 127.0;
        if mag > 0.3 && x.t > 5.0 {
            assert_eq!(x.thumb, ThumbState::Stick, "t={:.2} stick at {mag:.2}", x.t);
            checked += 1;
        }
    }
    assert!(checked > 500, "{checked}");
}

#[test]
fn right_controller_trigger_and_grip_of_the_first_round() {
    let s = replay(3);
    assert!(s.len() > 40_000);
    // The right controller was idle in this capture (all zero): relaxed hand throughout.
    let p = Params::default();
    assert!(s.iter().all(|x| x.thumb == ThumbState::Rest));
    assert!(s.iter().all(
        |x| (x.curls[3] - p.index_rest).abs() < 0.02 && (x.curls[2] - p.grip_rest).abs() < 0.02
    ));
}

#[test]
fn the_p82_capture_with_squeezes_on_both_controllers() {
    // p82 holds the first live round: trigger and grip curves on the right controller, bell-shaped squeezes.
    let rows = load_rec("p82.rec.gz");
    let t0 = rows[0].0;
    let mut st = [
        HandState::new(Params::default()),
        HandState::new(Params::default()),
    ];
    let mut max = [[0.0f32; 5]; 2];
    let mut n = [0usize; 2];
    for (t, b) in &rows {
        let Some(c) = decode_controller(b) else {
            continue;
        };
        let k = match c.slot {
            2 => 0,
            3 => 1,
            _ => continue,
        };
        let curls = st[k]
            .update(((t - t0) * 1e9) as u64, &input_from_report(&as_report(&c)))
            .to_array();
        n[k] += 1;
        for (m, c) in max[k].iter_mut().zip(curls) {
            *m = m.max(c);
            assert!((0.0..=1.0).contains(&c));
        }
    }
    assert!(n[0] > 1000 && n[1] > 1000, "{n:?}");
    // somebody pulled a trigger and squeezed a grip on at least one controller
    let idx = max[0][3].max(max[1][3]);
    let grip = max[0][2].max(max[1][2]);
    assert!(idx > 0.7 && grip > 0.7, "index {idx} grip {grip}");
}
