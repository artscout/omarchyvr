//! Property tests: whatever the controllers send, the hand stays sane.
use pimax_hands::mapping::{flags, to_joints};
use pimax_hands::{HandState, Input, Params};
use proptest::prelude::*;

const MS: u64 = 1_000_000;

/// Floats including the nasty ones.
fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -1.5f32..1.5,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::NEG_INFINITY),
        1 => Just(f32::MAX),
        1 => Just(f32::MIN),
    ]
}

fn any_input() -> impl Strategy<Value = Input> {
    (any_f32(), any_f32(), any_f32(), any_f32(), any::<u32>()).prop_map(
        |(trigger, grip, stick_x, stick_y, flags)| Input {
            trigger,
            grip,
            stick_x,
            stick_y,
            flags,
        },
    )
}

/// Sane inputs (0..1 values, defined flags).
fn clean_input() -> impl Strategy<Value = Input> {
    (
        0f32..=1.0,
        0f32..=1.0,
        -1f32..=1.0,
        -1f32..=1.0,
        0u32..=flags::ALL,
    )
        .prop_map(|(trigger, grip, stick_x, stick_y, flags)| Input {
            trigger,
            grip,
            stick_x,
            stick_y,
            flags,
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn output_is_always_finite_and_in_range(
        steps in proptest::collection::vec((0u64..400, any_input()), 1..200),
        start in 0u64..u64::MAX / 4,
    ) {
        let mut s = HandState::new(Params::default());
        let mut t = start;
        for (dt_ms, i) in steps {
            t = t.saturating_add(dt_ms * MS);
            let c = s.update(t, &i);
            for v in c.to_array() {
                prop_assert!(v.is_finite() && (0.0..=1.0).contains(&v), "{c:?}");
            }
            let j = to_joints(&c);
            for f in [j.little, j.ring, j.middle, j.index, j.thumb] {
                prop_assert!(f.splay.is_finite());
                for v in f.joint_curls {
                    prop_assert!(v.is_finite() && (0.0..=2.0).contains(&v));
                }
            }
        }
    }

    #[test]
    fn timestamps_may_go_backwards_without_trouble(
        times in proptest::collection::vec(any::<u64>(), 1..100),
        i in clean_input(),
    ) {
        let mut s = HandState::new(Params::default());
        for t in times {
            let c = s.update(t, &i);
            for v in c.to_array() {
                prop_assert!(v.is_finite() && (0.0..=1.0).contains(&v));
            }
        }
    }

    #[test]
    fn deterministic_and_shift_invariant(
        steps in proptest::collection::vec((1u64..30, clean_input()), 1..150),
        shift in 0u64..1_000_000_000_000,
    ) {
        let mut a = HandState::new(Params::default());
        let mut b = HandState::new(Params::default());
        let (mut ta, mut tb) = (5 * MS, 5 * MS + shift);
        for (dt_ms, i) in steps {
            ta += dt_ms * MS;
            tb += dt_ms * MS;
            let ca = a.update(ta, &i);
            let cb = b.update(tb, &i);
            prop_assert_eq!(ca, cb);
        }
    }

    #[test]
    fn steady_state_forgets_the_history(
        history in proptest::collection::vec((1u64..30, clean_input()), 0..80),
        trigger in 0f32..=1.0,
        grip in 0f32..=1.0,
    ) {
        let hold = Input { trigger, grip, ..Input::default() };
        let mut a = HandState::new(Params::default());
        let mut t = 10 * MS;
        for (dt_ms, i) in history {
            t += dt_ms * MS;
            a.update(t, &i);
        }
        let mut b = HandState::new(Params::default());
        let mut tb = 10 * MS;
        let (mut ca, mut cb) = (a.curls(), b.curls());
        for _ in 0..600 { // 2.4 s at 4 ms
            t += 4 * MS;
            tb += 4 * MS;
            ca = a.update(t, &hold);
            cb = b.update(tb, &hold);
        }
        for (x, y) in ca.to_array().iter().zip(cb.to_array().iter()) {
            prop_assert!((x - y).abs() < 0.02, "{ca:?} vs {cb:?}");
        }
    }

    #[test]
    fn more_trigger_never_means_less_index_curl(a in 0f32..=1.0, b in 0f32..=1.0) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let settle = |v: f32| {
            let mut s = HandState::new(Params::default());
            let mut c = s.curls();
            for k in 1..=300u64 {
                c = s.update(k * 4 * MS, &Input { trigger: v, ..Input::default() });
            }
            c
        };
        prop_assert!(settle(lo).index <= settle(hi).index + 1e-4);
        prop_assert_eq!(settle(lo).thumb, settle(hi).thumb);
    }

    #[test]
    fn joint_curls_grow_with_curl(c1 in 0f32..=1.0, c2 in 0f32..=1.0) {
        let (lo, hi) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };
        let mk = |v: f32| pimax_hands::Curls { little: v, ring: v, middle: v, index: v, thumb: v };
        let (a, b) = (to_joints(&mk(lo)), to_joints(&mk(hi)));
        for (x, y) in [(a.little, b.little), (a.ring, b.ring), (a.middle, b.middle), (a.index, b.index), (a.thumb, b.thumb)] {
            for k in 0..4 {
                prop_assert!(x.joint_curls[k] <= y.joint_curls[k] + 1e-6);
            }
        }
    }
}
