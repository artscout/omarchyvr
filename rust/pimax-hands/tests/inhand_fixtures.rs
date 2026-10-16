//! The in-hand detector (src/inhand.rs) on the REAL left-controller IMU log of the session 2026-10-16-1818
//! (rust/fixtures/inhand_*.csv.gz, cut by rust/fixtures/make_inhand_fixtures.py; `dt_us,gx,gy,gz,ax,ay,az`).

use flate2::read::GzDecoder;
use pimax_hands::inhand::{Config, InHand, Why};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

type Sample = (u64, [i16; 3], [i16; 3]);

fn load(name: &str) -> Vec<Sample> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(format!("inhand_{name}.csv.gz"));
    let f = std::fs::File::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut t = 1_000_000_000u64;
    BufReader::new(GzDecoder::new(f))
        .lines()
        .map(|l| {
            let l = l.unwrap();
            let v: Vec<i64> = l.split(',').map(|x| x.parse().unwrap()).collect();
            t += v[0] as u64 * 1000;
            let i = |k: usize| v[k] as i16;
            (t, [i(1), i(2), i(3)], [i(4), i(5), i(6)])
        })
        .collect()
}

/// Feed the samples; the (time from the start in s, held) transitions.
fn run(d: &mut InHand, s: &[Sample]) -> Vec<(f64, bool, Why)> {
    let t0 = s[0].0;
    let mut out = Vec::new();
    let mut held = d.is_held();
    for &(t, g, a) in s {
        d.sample(t, g, a);
        if d.held(t) != held {
            held = !held;
            out.push(((t - t0) as f64 * 1e-9, held, d.last_change().0));
        }
    }
    out
}

/// Pick up first (a button), so the "put down" path is tested on real data.
fn picked_up(t: u64) -> InHand {
    let mut d = InHand::new(Config::default());
    d.input(t, 0, 0, 0, 0, 0, 0);
    d.input(t + 1, 0, 0, 0, 0, pimax_hands::report::BTN_A, 0);
    assert!(d.is_held());
    d
}

#[test]
fn lying_on_the_desk_never_shows() {
    for name in ["desk", "desk_bump"] {
        let s = load(name);
        let mut d = InHand::default();
        let tr = run(&mut d, &s);
        eprintln!("{}: {tr:?}", line!());
        assert!(tr.is_empty(), "{name}: {tr:?}");
    }
}

#[test]
fn held_still_on_the_desk_is_put_down_within_6_s() {
    // a button press at the start of the desk recording, then nothing: down after ~5 s of stillness
    let s = load("desk");
    let mut d = picked_up(s[0].0);
    let tr = run(&mut d, &s);
    eprintln!("{}: {tr:?}", line!());
    assert_eq!(tr.len(), 1, "{tr:?}");
    let (at, held, why) = tr[0];
    assert!(
        !held && why == Why::Still && (5.0..6.5).contains(&at),
        "{tr:?}"
    );
}

#[test]
fn picking_up_shows_within_250_ms() {
    // desk until 20.5 s of the session (6.5 s into this cut), then picked up and waved
    let s = load("pickup");
    let mut d = InHand::default();
    let tr = run(&mut d, &s);
    eprintln!("{}: {tr:?}", line!());
    assert!(!tr.is_empty(), "never picked up");
    let (at, held, why) = tr[0];
    assert!(held && why == Why::Motion, "{tr:?}");
    assert!(
        (6.45..6.75).contains(&at),
        "picked up at {at:.3} s (motion starts at ~6.5 s)"
    );
    assert!(tr.len() == 1, "dropped again while waved: {tr:?}");
}

#[test]
fn quiet_in_the_hand_is_never_put_down() {
    // one minute in the hand: pointing, small moves, near-still stretches (gyro std 3..20 LSB)
    let s = load("held");
    let mut d = picked_up(s[0].0);
    let tr = run(&mut d, &s);
    eprintln!("{}: {tr:?}", line!());
    assert!(tr.is_empty(), "{tr:?}");
}

#[test]
fn put_down_and_picked_up_again() {
    // in the hand until 290.5 s of the session (7.5 s into the cut), on the desk until 305 s (22 s)
    let s = load("putdown");
    let mut d = picked_up(s[0].0);
    let tr = run(&mut d, &s);
    eprintln!("{}: {tr:?}", line!());
    assert_eq!(tr.len(), 2, "{tr:?}");
    assert!(
        !tr[0].1 && tr[0].2 == Why::Still && (12.5..14.5).contains(&tr[0].0),
        "down: {tr:?}"
    );
    assert!(
        tr[1].1 && tr[1].2 == Why::Motion && (21.9..22.6).contains(&tr[1].0),
        "up: {tr:?}"
    );
}

#[test]
fn typing_on_the_desk_stays_down() {
    // SYNTHETIC: the desk recording with keystroke knocks added (no typing was recorded with the controllers): every
    // 120..200 ms a 10 ms burst of +-15 LSB gyro and +-40 LSB accel noise, 6 keys a second for 20 s
    let mut s = load("desk");
    let t0 = s[0].0;
    let mut seed = 12345u64;
    let mut rnd = |m: i32| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as i32 % (2 * m + 1)) - m
    };
    let mut next_key = t0 + 100_000_000;
    let mut key_end = 0;
    for x in s.iter_mut() {
        if x.0 >= next_key {
            key_end = x.0 + 10_000_000;
            next_key = x.0 + 120_000_000 + (rnd(40) + 40) as u64 * 1_000_000;
        }
        if x.0 < key_end {
            for k in 0..3 {
                x.1[k] += rnd(15) as i16;
                x.2[k] += rnd(40) as i16;
            }
        }
    }
    let mut d = InHand::default();
    let tr = run(&mut d, &s);
    eprintln!("{}: {tr:?}", line!());
    assert!(tr.is_empty(), "typing picked it up: {tr:?}");
}

#[test]
fn no_reports_puts_it_down() {
    let s = load("held");
    let mut d = picked_up(s[0].0);
    let half = &s[..s.len() / 2];
    run(&mut d, half);
    let last = half.last().unwrap().0;
    assert!(d.held(last + 1_000_000_000));
    assert!(!d.held(last + 2_000_000_000));
    assert_eq!(d.last_change().0, Why::NoReports);
}
