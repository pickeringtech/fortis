// Counting ten million keys over a thousand buckets, with the same work as the Fortis program: a separate lookup
// and insertion per key, an insertion-ordered map (Fortis maps iterate in first-insertion order, 6.7) and the same
// folded-multiply hasher keyed by a per-process seed (X-119). Written independently of the Fortis runtime.
use std::collections::hash_map::{Entry, RandomState};
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};

const MULTIPLE: u64 = 0x9E37_79B9_7F4A_7C15;
const FINAL: u64 = 0x2D35_8DCC_AA6C_78A5;

#[inline(always)]
fn fold(a: u64, b: u64) -> u64 {
    let full = (a as u128).wrapping_mul(b as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

#[derive(Clone, Copy)]
struct Fold {
    seed: u64,
}

struct FoldHasher {
    acc: u64,
}

impl BuildHasher for Fold {
    type Hasher = FoldHasher;
    fn build_hasher(&self) -> FoldHasher {
        FoldHasher { acc: self.seed }
    }
}

impl Hasher for FoldHasher {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.acc = fold(self.acc ^ *b as u64, MULTIPLE);
        }
    }
    fn write_i64(&mut self, x: i64) {
        self.acc = fold(self.acc ^ x as u64, MULTIPLE);
    }
    fn finish(&self) -> u64 {
        fold(self.acc, FINAL)
    }
}

/// An insertion-ordered map: entries in first-insertion order, indexed by key.
struct Ordered {
    index: HashMap<i64, usize, Fold>,
    entries: Vec<(i64, i64)>,
}

impl Ordered {
    fn new(seed: u64) -> Ordered {
        Ordered { index: HashMap::with_hasher(Fold { seed }), entries: Vec::new() }
    }
    fn get(&self, k: i64) -> Option<i64> {
        self.index.get(&k).map(|&i| self.entries[i].1)
    }
    fn set(&mut self, k: i64, v: i64) {
        let n = self.entries.len();
        match self.index.entry(k) {
            Entry::Occupied(e) => self.entries[*e.get()].1 = v,
            Entry::Vacant(e) => {
                self.entries.push((k, v));
                e.insert(n);
            }
        }
    }
}

fn main() {
    let seed = RandomState::new().hash_one(0x5EED_u64) | 1;
    let mut counts = Ordered::new(seed);
    for i in 0..10_000_000i64 {
        let key = i.wrapping_mul(7919) % 1000;
        let seen = counts.get(key).unwrap_or(0);
        counts.set(key, seen.wrapping_add(1));
    }
    println!("{} {}", counts.entries.len(), counts.get(17).unwrap_or(0));
}
