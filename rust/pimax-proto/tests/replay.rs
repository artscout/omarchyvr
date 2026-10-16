//! Port of linux/test_controllers_replay.py: the real captures drive the decoder and every button
//! event must show up at the time and in the order the operator marked it.
mod common;
use common::*;
use pimax_proto::*;

fn marks() -> std::collections::HashMap<String, f64> {
    let s = std::fs::read_to_string(fixtures_dir().join("ctrl_marks.json")).unwrap();
    s.lines()
        .filter_map(|l| {
            let l = l.trim().trim_end_matches(',');
            let (k, v) = l.rsplit_once(": ")?;
            Some((k.trim_matches('"').to_string(), v.parse().ok()?))
        })
        .collect()
}

fn first_active(
    f: &[(f64, ControllerReport)],
    pred: impl Fn(&ControllerReport) -> bool,
) -> Option<f64> {
    f.iter().find(|(_, c)| pred(c)).map(|(t, _)| *t)
}

#[test]
fn left_controller_buttons_capture() {
    let rows = load_rec("ctrl_buttons_hub2.rec.gz");
    assert!(rows.len() > 80_000, "{}", rows.len());
    let t0 = rows[0].0;
    let left: Vec<(f64, ControllerReport)> = rows
        .iter()
        .filter(|(_, b)| b[3] == 2)
        .map(|(t, b)| (t - t0, decode_controller(b).unwrap()))
        .collect();
    let m = marks();
    assert_eq!(m.len(), 10);
    let mk = |k: &str| m[k] - t0;
    const LEAD: f64 = 6.5;
    type Pred = Box<dyn Fn(&ControllerReport) -> bool>;
    let table: Vec<(&str, &str, f64, Pred)> = vec![
        ("trigger", "l trigger", 0.5, Box::new(|c| c.trigger > 200)),
        ("grip", "l grip", 2.0, Box::new(|c| c.grip > 100)),
        ("A", "l a", 0.5, Box::new(|c| c.buttons & BTN_A != 0)),
        ("B", "l b", 0.5, Box::new(|c| c.buttons & BTN_B != 0)),
        (
            "stick",
            "l sick click",
            0.5,
            Box::new(|c| c.buttons & BTN_STICK != 0),
        ),
        (
            "menu",
            "l menu",
            0.5,
            Box::new(|c| c.buttons & BTN_MENU != 0),
        ),
        ("up", "l stick up", 0.5, Box::new(|c| c.stick_y > 100)),
        ("down", "l stick down", 0.5, Box::new(|c| c.stick_y < -100)),
        ("right", "l stck right", 0.5, Box::new(|c| c.stick_x > 100)),
        ("left", "l sticvk left", 0.5, Box::new(|c| c.stick_x < -100)),
    ];
    let mut first: Vec<(&str, f64)> = vec![];
    for (name, mark, slack, pred) in &table {
        let t = first_active(&left, pred).unwrap_or_else(|| panic!("left {name} never active"));
        let mt = mk(mark);
        assert!(
            mt - LEAD <= t && t <= mt + slack,
            "left {name}: first at {t:.2}, marker {mt:.2}"
        );
        first.push((name, t));
    }
    // activation order == marker order
    let mut by_time = first.clone();
    by_time.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let order: Vec<&str> = by_time.iter().map(|x| x.0).collect();
    let want: Vec<&str> = table.iter().map(|x| x.0).collect();
    assert_eq!(order, want, "all button events in marker order");

    let t_of = |n: &str| first.iter().find(|x| x.0 == n).unwrap().1;
    // single-button edges are exclusive
    for (name, bit) in [("A", BTN_A), ("B", BTN_B), ("menu", BTN_MENU)] {
        let t = t_of(name);
        let near: u32 = left
            .iter()
            .filter(|(tt, _)| *tt >= t - 0.2 && *tt <= t + 0.1)
            .fold(0, |a, (_, c)| a | c.buttons);
        assert_eq!(near, bit, "{name} edge exclusive");
    }
    let touch = |t: f64, span: f64, f: &dyn Fn(TouchBits) -> bool| {
        left.iter()
            .any(|(tt, c)| *tt >= t - span && *tt <= t + span && f(c.touch_bits()))
    };
    assert!(
        touch(t_of("A") + 0.05, 0.05, &|b| b.a),
        "touch-A during A press"
    );
    assert!(
        touch(t_of("B") + 0.05, 0.05, &|b| b.b),
        "touch-B during B press"
    );
    assert!(
        touch(t_of("stick") + 0.5, 0.6, &|b| b.stick),
        "touch-stick during click"
    );
    assert!(
        touch(t_of("up") + 1.0, 1.0, &|b| b.stick),
        "touch-stick during stick up"
    );
    assert!(
        left.iter().all(|(_, c)| c.touch_bits().hand_on),
        "bit4 (hand on the controller) is set the whole take on the left: it was held"
    );
    // analog sanity
    assert_eq!(left.iter().map(|x| x.1.trigger).max(), Some(255));
    assert_eq!(left.iter().map(|x| x.1.grip).max(), Some(255));
    assert!(left.iter().map(|x| x.1.stick_x).min().unwrap() <= -127);
    assert_eq!(left.iter().map(|x| x.1.stick_y).max(), Some(127));
    assert!(left
        .iter()
        .filter(|x| x.0 < 20.0)
        .all(|(_, c)| c.stick_x.abs() < 4 && c.stick_y.abs() < 4));
    let mags: Vec<f64> = left[..200]
        .iter()
        .map(|(_, c)| {
            c.raw_accel
                .iter()
                .map(|&a| f64::from(a).powi(2))
                .sum::<f64>()
                .sqrt()
                / 1024.0
        })
        .collect();
    let mean = mags.iter().sum::<f64>() / mags.len() as f64;
    assert!(0.9 < mean && mean < 1.1, "accel magnitude {mean}");
    assert!(left.windows(2).all(|w| {
        let d = w[1].1.ts_us.wrapping_sub(w[0].1.ts_us);
        d > 0 && d < 20_000
    }));
    assert!(
        left.iter().all(|x| x.1.battery == 72 && !x.1.charging),
        "left battery 72%"
    );
}

#[test]
fn right_controller_idle_and_round1() {
    let rows = load_rec("ctrl_buttons_hub2.rec.gz");
    let right: Vec<ControllerReport> = rows
        .iter()
        .filter(|(_, b)| b[3] == 3)
        .map(|(_, b)| decode_controller(b).unwrap())
        .collect();
    assert!(right.len() > 40_000);
    assert!(right.iter().all(|c| c.buttons == 0
        && c.touch_bits() == TouchBits::default()
        && c.trigger == 0
        && c.grip == 0
        && c.stick_x == 0
        && c.stick_y == 0));
    assert!(right.iter().all(|c| c.battery == 99));

    let p82 = load_rec("p82.rec.gz");
    let r1: Vec<(f64, ControllerReport)> = p82
        .iter()
        .filter(|(_, b)| b[3] == 3)
        .map(|(t, b)| (*t, decode_controller(b).unwrap()))
        .collect();
    let win = |a: f64, b: f64| -> Vec<&ControllerReport> {
        r1.iter()
            .filter(|x| x.0 >= a && x.0 <= b)
            .map(|x| &x.1)
            .collect()
    };
    let grip = win(72.8, 73.4);
    assert!(grip.iter().map(|c| c.grip).max().unwrap() > 150 && grip[0].grip < 20);
    let trig = win(75.4, 75.8);
    assert_eq!(trig.iter().map(|c| c.trigger).max(), Some(255));
    assert!(trig[0].trigger < 30);
    assert!(win(75.5, 75.7).iter().map(|c| c.stick_y).max().unwrap() >= 10);
    assert!(
        r1.iter().all(|x| x.1.buttons == 0),
        "round 1 never pressed a digital button"
    );
    assert!(r1.iter().any(|x| x.1.touch_bits().hand_on));
    assert!(r1
        .iter()
        .filter(|x| x.0 < 70.0)
        .all(|x| !x.1.touch_bits().hand_on));
}
