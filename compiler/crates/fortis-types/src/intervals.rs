//! The interval domain of Language Reference 6.3 (DEL-13): a port of checker/intervals.py.

use std::collections::BTreeMap;

use crate::types::{int_bounds, Int, Type};

pub type Interval = (Int, Int);

pub const RANGED_LENGTH_FACT: bool = true;
pub const CHECKED_RESULT_INTERVAL: bool = true;

pub fn hull(a: Option<Interval>, b: Option<Interval>) -> Option<Interval> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
        _ => None,
    }
}

pub fn meet(a: Interval, b: Interval) -> Option<Interval> {
    let (lo, hi) = (a.0.max(b.0), a.1.min(b.1));
    if lo <= hi {
        Some((lo, hi))
    } else {
        None
    }
}

fn mul(a: Int, b: Int) -> Int {
    a.saturating_mul(b)
}

pub fn tdiv(x: Int, y: Int) -> Int {
    let q = x.abs() / y.abs();
    if (x >= 0) == (y >= 0) {
        q
    } else {
        -q
    }
}

fn shl(a: Int, b: Int) -> Int {
    if a == 0 {
        return 0;
    }
    if b >= 126 {
        return if a > 0 { Int::MAX } else { Int::MIN };
    }
    a.saturating_mul(1i128 << b)
}

/// Interval arithmetic of 6.3/6.4; None when the rules give no result.
pub fn arith(op: &str, a: Interval, b: Interval, width: Int) -> Option<Interval> {
    match op {
        "+" => Some((a.0.saturating_add(b.0), a.1.saturating_add(b.1))),
        "-" => Some((a.0.saturating_sub(b.1), a.1.saturating_sub(b.0))),
        "*" => {
            let ps = [mul(a.0, b.0), mul(a.0, b.1), mul(a.1, b.0), mul(a.1, b.1)];
            Some((*ps.iter().min().unwrap(), *ps.iter().max().unwrap()))
        }
        "/" => {
            if b.0 <= 0 && 0 <= b.1 {
                return None;
            }
            let qs = [tdiv(a.0, b.0), tdiv(a.0, b.1), tdiv(a.1, b.0), tdiv(a.1, b.1)];
            Some((*qs.iter().min().unwrap(), *qs.iter().max().unwrap()))
        }
        "%" => {
            if b.0 <= 0 && 0 <= b.1 {
                return None;
            }
            let m = b.0.abs().max(b.1.abs());
            if a.0 >= 0 {
                Some((0, m - 1))
            } else if a.1 <= 0 {
                Some((-(m - 1), 0))
            } else {
                Some((-(m - 1), m - 1))
            }
        }
        "<<" => {
            if b.0 < 0 || b.1 > width - 1 {
                return None;
            }
            let vals = [shl(a.0, b.0), shl(a.0, b.1), shl(a.1, b.0), shl(a.1, b.1)];
            Some((*vals.iter().min().unwrap(), *vals.iter().max().unwrap()))
        }
        ">>" => {
            if b.0 < 0 || b.1 > width - 1 {
                return None;
            }
            let vals = [a.0 >> b.0, a.0 >> b.1, a.1 >> b.0, a.1 >> b.1];
            Some((*vals.iter().min().unwrap(), *vals.iter().max().unwrap()))
        }
        _ => None,
    }
}

/// The interval of a place p after `p op c` holds.
pub fn narrow(op: &str, p: Interval, c: Interval) -> Option<Interval> {
    match op {
        "<" => meet(p, (p.0, c.1 - 1)),
        "<=" => meet(p, (p.0, c.1)),
        ">" => meet(p, (c.0 + 1, p.1)),
        ">=" => meet(p, (c.0, p.1)),
        "==" => meet(p, c),
        "!=" => {
            if c.0 == c.1 {
                if p.0 == c.0 {
                    return meet(p, (p.0 + 1, p.1));
                }
                if p.1 == c.0 {
                    return meet(p, (p.0, p.1 - 1));
                }
            }
            Some(p)
        }
        _ => Some(p),
    }
}

pub fn negate_op(op: &str) -> &'static str {
    match op {
        "<" => ">=",
        "<=" => ">",
        ">" => "<=",
        ">=" => "<",
        "==" => "!=",
        _ => "==",
    }
}

pub fn flip_op(op: &str) -> &'static str {
    match op {
        "<" => ">",
        "<=" => ">=",
        ">" => "<",
        ">=" => "<=",
        "==" => "==",
        _ => "!=",
    }
}

/// Flow-sensitive facts about places: integer intervals, presence of optionals, index-below-length facts.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub ints: BTreeMap<String, Interval>,
    pub present: BTreeMap<String, bool>,
    pub below: BTreeMap<String, String>,
}

/// The place `a` is `b`, a prefix of it, or a field of it: a write to one may change the other.
pub fn overlaps(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

impl Facts {
    /// The hull of every incoming path (6.3); `states` is not empty.
    pub fn join_all(states: &[Facts]) -> Facts {
        let mut out = states[0].clone();
        for s in &states[1..] {
            out = out.join(s);
        }
        out
    }

    pub fn join(&self, other: &Facts) -> Facts {
        let mut ints = BTreeMap::new();
        for (k, v) in &self.ints {
            if let Some(o) = other.ints.get(k) {
                if let Some(h) = hull(Some(*v), Some(*o)) {
                    ints.insert(k.clone(), h);
                }
            }
        }
        let present = self.present.iter().filter(|(k, v)| **v && other.present.get(*k) == Some(&true)).map(|(k, _)| (k.clone(), true)).collect();
        let below = self.below.iter().filter(|(k, v)| other.below.get(*k) == Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
        Facts { ints, present, below }
    }

    /// Every fact about the place, a prefix of it, or a field of it is discarded (6.3).
    pub fn invalidate(&mut self, place: &str) {
        let hit = |k: &str| k == place || k.starts_with(&format!("{place}.")) || place.starts_with(&format!("{k}."));
        self.ints.retain(|k, _| !hit(k));
        self.present.retain(|k, _| !hit(k));
        self.below.retain(|k, v| !hit(k) && !hit(v));
    }

    /// Facts about `names` (each place, a prefix of it, or a field of it), removed from `self`.
    /// A match arm uses this so a guard fact stays with the binding that established it (6.8, 9.8).
    pub fn split_off_names(&mut self, names: &[String]) -> Facts {
        let hit = |place: &str| names.iter().any(|n| overlaps(place, n));
        let mut saved = Facts::default();
        for k in self.ints.keys().filter(|k| hit(k)).cloned().collect::<Vec<_>>() {
            if let Some(v) = self.ints.remove(&k) {
                saved.ints.insert(k, v);
            }
        }
        for k in self.present.keys().filter(|k| hit(k)).cloned().collect::<Vec<_>>() {
            if let Some(v) = self.present.remove(&k) {
                saved.present.insert(k, v);
            }
        }
        for k in self.below.iter().filter(|(k, v)| hit(k) || hit(v)).map(|(k, _)| k.clone()).collect::<Vec<_>>() {
            if let Some(v) = self.below.remove(&k) {
                saved.below.insert(k, v);
            }
        }
        saved
    }

    /// Drop facts about `names`, then install `saved` (what those names meant for an outer binding).
    pub fn restore_names(&mut self, names: &[String], saved: &Facts) {
        let _ = self.split_off_names(names);
        self.ints.extend(saved.ints.clone());
        self.present.extend(saved.present.clone());
        self.below.extend(saved.below.clone());
    }

    /// A copy that carries `outer` for `names` instead of this state's facts about them.
    pub fn sealed(&self, names: &[String], outer: &Facts) -> Facts {
        let mut f = self.clone();
        f.restore_names(names, outer);
        f
    }
}

pub fn declared(t: &Type) -> Option<Interval> {
    int_bounds(t)
}

/// violated when the intervals are disjoint, unproved when they overlap but have is not contained.
pub fn classify(have: Interval, want: Interval) -> &'static str {
    if have.1 < want.0 || have.0 > want.1 {
        "violated"
    } else {
        "unproved"
    }
}
