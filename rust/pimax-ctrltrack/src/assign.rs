//! Which controller is which: a rule-based body model for the live tracker (head frame = OpenXR head axes, x right,
//! y up, z back). Used for every pose found by the brute-force search, and to settle two tracked poses that collapsed
//! onto one controller.
//!
//! Rules:
//! * reach: a hand is at most `reach` from its shoulder (shoulders 0.18 m to the side, 0.25 m below and 0.10 m
//!   behind the eyes; the yaw-only part of the head is ignored here, so looking down shortens the reach a bit);
//! * side: the left hand is not more than `cross` to the right of the head centre, and the other way round;
//! * continuity: a slot seen less than `memory_s` ago only takes a pose within `0.12 m + max_speed * age` of where it
//!   was;
//! * separation: two hands are never closer than `min_sep`;
//! * among the feasible assignments the one with the lowest cost wins (distance to the slot's last pose, a penalty for
//!   the wrong side of the head centre and for the other hand's LED-shape label).

pub type V3 = [f64; 3];

#[derive(Clone, Copy, Debug)]
pub struct Body {
    pub shoulder: [V3; 2],
    pub reach: f64,
    pub cross: f64,
    pub min_sep: f64,
    pub max_speed: f64,
    pub memory_s: f64,
}

impl Default for Body {
    fn default() -> Self {
        Body {
            shoulder: [[-0.18, -0.25, 0.10], [0.18, -0.25, 0.10]],
            reach: 0.80,
            cross: 0.30,
            min_sep: 0.08,
            max_speed: 2.5,
            memory_s: 1.5,
        }
    }
}

fn dist(a: V3, b: V3) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A pose found this frame: head-frame position and the LED model it fitted best (0 left, 1 right).
#[derive(Clone, Copy, Debug)]
pub struct Cand {
    pub p: V3,
    pub model: usize,
}

/// What we know about a slot: its last pose and how old it is (s), and whether it is already tracked this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Slot {
    pub last: Option<(V3, f64)>,
    pub tracked_now: Option<V3>,
}

impl Body {
    /// Why `p` cannot be `slot`'s hand (None = plausible), ignoring continuity and the other hand.
    pub fn implausible(&self, slot: usize, p: V3) -> Option<&'static str> {
        if dist(p, self.shoulder[slot]) > self.reach {
            return Some("out of reach");
        }
        if (slot == 0 && p[0] > self.cross) || (slot == 1 && p[0] < -self.cross) {
            return Some("wrong side");
        }
        None
    }

    fn cost(&self, slot: usize, c: &Cand, s: &Slot, other: &Slot) -> Option<f64> {
        if self.implausible(slot, c.p).is_some() {
            return None;
        }
        if let Some(o) = other.tracked_now {
            if dist(o, c.p) < self.min_sep {
                return None;
            }
        }
        let mut cost = 0.0;
        if let Some((lp, age)) = s.last {
            if age < self.memory_s {
                let d = dist(lp, c.p);
                if d > 0.12 + self.max_speed * age {
                    return None;
                }
                cost += d;
            }
        }
        if (slot == 0 && c.p[0] > 0.0) || (slot == 1 && c.p[0] < 0.0) {
            cost += 0.15 * c.p[0].abs() / 0.1;
        }
        if c.model != slot {
            cost += 0.05;
        }
        Some(cost)
    }

    /// Assign candidates to the free slots: returns (candidate index, slot) pairs. Slots already tracked this frame
    /// are not free. Maximises the number of assigned candidates, then minimises the cost; two candidates closer
    /// than `min_sep` are never both assigned.
    pub fn assign(&self, cands: &[Cand], slots: &[Slot; 2]) -> Vec<(usize, usize)> {
        let free: Vec<usize> = (0..2).filter(|&s| slots[s].tracked_now.is_none()).collect();
        let mut best: (usize, f64, Vec<(usize, usize)>) = (0, 0.0, Vec::new());
        // every injective map of a subset of candidates onto free slots (at most 2 x 2)
        let n = cands.len().min(4);
        let mut options: Vec<Vec<(usize, usize)>> = vec![Vec::new()];
        for &s in &free {
            let mut next = Vec::new();
            for o in &options {
                next.push(o.clone());
                for c in 0..n {
                    if !o.iter().any(|(ci, _)| *ci == c) {
                        let mut o2 = o.clone();
                        o2.push((c, s));
                        next.push(o2);
                    }
                }
            }
            options = next;
        }
        for o in options {
            let mut total = 0.0;
            let mut ok = true;
            for &(c, s) in &o {
                let other = 1 - s;
                // the other slot's hand this frame: tracked, or assigned in this option
                let mut os = slots[other];
                if let Some(&(c2, _)) = o.iter().find(|(_, s2)| *s2 == other) {
                    os.tracked_now = Some(cands[c2].p);
                }
                match self.cost(s, &cands[c], &slots[s], &os) {
                    Some(v) => total += v,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok && (o.len() > best.0 || (o.len() == best.0 && total < best.1)) {
                best = (o.len(), total, o);
            }
        }
        best.2
    }

    /// Two tracked hands closer than `min_sep`: which slot to drop (the one that moved more from its last pose).
    pub fn collapsed(&self, now: [V3; 2], last: [Option<V3>; 2]) -> Option<usize> {
        if dist(now[0], now[1]) >= self.min_sep {
            return None;
        }
        let jump = |s: usize| last[s].map(|l| dist(l, now[s])).unwrap_or(f64::INFINITY);
        Some(if jump(0) > jump(1) { 0 } else { 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_solve_of_the_left_controller_is_not_given_to_the_lost_right_hand() {
        let b = Body::default();
        // left tracked at (-0.1, -0.1, -0.35); right was last at (0.2, -0.1, -0.35) 0.2 s ago
        let slots = [
            Slot { last: Some(([-0.1, -0.1, -0.35], 0.0)), tracked_now: Some([-0.1, -0.1, -0.35]) },
            Slot { last: Some(([0.2, -0.1, -0.35], 0.2)), tracked_now: None },
        ];
        // the search returns a pose 5 cm from the left hand: duplicate, not the right hand
        let a = b.assign(&[Cand { p: [-0.06, -0.12, -0.36], model: 1 }], &slots);
        assert!(a.is_empty(), "{a:?}");
        // a pose near where the right hand was is taken
        let a = b.assign(&[Cand { p: [0.22, -0.1, -0.33], model: 1 }], &slots);
        assert_eq!(a, vec![(0, 1)]);
    }

    #[test]
    fn two_new_poses_go_by_continuity_then_side() {
        let b = Body::default();
        let none = [Slot::default(), Slot::default()];
        // crossed hands without history: side rule (left = smaller x)
        let c = [Cand { p: [0.15, -0.1, -0.4], model: 0 }, Cand { p: [-0.15, -0.1, -0.4], model: 1 }];
        let mut a = b.assign(&c, &none);
        a.sort();
        assert_eq!(a, vec![(0, 1), (1, 0)]);
        // with history the hands keep their identity even when they cross the middle a little
        let hist = [
            Slot { last: Some(([0.05, -0.1, -0.4], 0.04)), tracked_now: None },
            Slot { last: Some(([-0.05, -0.1, -0.4], 0.04)), tracked_now: None },
        ];
        let c = [Cand { p: [0.07, -0.1, -0.4], model: 0 }, Cand { p: [-0.07, -0.1, -0.4], model: 1 }];
        let mut a = b.assign(&c, &hist);
        a.sort();
        assert_eq!(a, vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn teleports_and_impossible_poses_are_rejected() {
        let b = Body::default();
        let slots = [Slot { last: Some(([-0.2, -0.1, -0.3], 0.04)), tracked_now: None }, Slot::default()];
        // 0.5 m in 40 ms for the left slot, and wrong side for a fresh right: left cannot take it
        let a = b.assign(&[Cand { p: [0.3, -0.1, -0.3], model: 0 }], &slots);
        assert_eq!(a, vec![(0, 1)]);
        // 1.5 m away: out of reach for both
        assert!(b.assign(&[Cand { p: [0.0, 0.0, -1.5], model: 0 }], &[Slot::default(), Slot::default()]).is_empty());
        assert_eq!(b.collapsed([[0.0; 3], [0.03, 0.0, 0.0]], [Some([0.0; 3]), Some([0.3, 0.0, 0.0])]), Some(1));
    }
}
