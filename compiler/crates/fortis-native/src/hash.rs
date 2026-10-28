//! The hasher of native `Map` and `Set` (X-119).
//!
//! Iteration order of a Fortis map or set is first-insertion order (6.7), so the hash function is not
//! observable; it only has to be fast and to resist collision flooding from outside input. This is a
//! folded-multiply hasher (the construction of foldhash and wyhash) keyed by a per-process seed taken
//! from the operating system's randomness through `RandomState`.

use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;

const MULTIPLE: u64 = 0x9E37_79B9_7F4A_7C15;
const FINAL: u64 = 0x2D35_8DCC_AA6C_78A5;

#[inline(always)]
fn folded_multiply(a: u64, b: u64) -> u64 {
    let full = (a as u128).wrapping_mul(b as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| std::collections::hash_map::RandomState::new().hash_one(0x5EED_u64) | 1)
}

/// The keyed builder: every map of one process shares its seed.
#[derive(Clone, Copy, Debug)]
pub struct FastState {
    seed: u64,
}

impl Default for FastState {
    #[inline]
    fn default() -> Self {
        FastState { seed: process_seed() }
    }
}

impl BuildHasher for FastState {
    type Hasher = FastHasher;
    #[inline]
    fn build_hasher(&self) -> FastHasher {
        FastHasher { acc: self.seed }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FastHasher {
    acc: u64,
}

impl FastHasher {
    #[inline(always)]
    fn mix(&mut self, x: u64) {
        self.acc = folded_multiply(self.acc ^ x, MULTIPLE);
    }
}

impl Hasher for FastHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.mix(u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut b = [0u8; 8];
            b[..rest.len()].copy_from_slice(rest);
            self.mix(u64::from_le_bytes(b) ^ ((rest.len() as u64) << 56));
        }
    }
    #[inline]
    fn write_u8(&mut self, x: u8) {
        self.mix(x as u64);
    }
    #[inline]
    fn write_u16(&mut self, x: u16) {
        self.mix(x as u64);
    }
    #[inline]
    fn write_u32(&mut self, x: u32) {
        self.mix(x as u64);
    }
    #[inline]
    fn write_u64(&mut self, x: u64) {
        self.mix(x);
    }
    #[inline]
    fn write_u128(&mut self, x: u128) {
        self.mix(x as u64);
        self.mix((x >> 64) as u64);
    }
    #[inline]
    fn write_usize(&mut self, x: usize) {
        self.mix(x as u64);
    }
    #[inline]
    fn write_i64(&mut self, x: i64) {
        self.mix(x as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        folded_multiply(self.acc, FINAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::hash::Hash;

    fn h<T: Hash>(state: &FastState, v: T) -> u64 {
        state.hash_one(v)
    }

    #[test]
    fn deterministic_within_a_process() {
        let a = FastState::default();
        let b = FastState::default();
        assert_eq!(h(&a, 42i64), h(&b, 42i64));
        assert_eq!(h(&a, "apple"), h(&b, "apple"));
    }

    #[test]
    fn distinguishes_nearby_integers_and_prefix_strings() {
        let s = FastState::default();
        let ints: HashSet<u64> = (0..100_000i64).map(|i| h(&s, i)).collect();
        assert_eq!(ints.len(), 100_000);
        assert_ne!(h(&s, "ab"), h(&s, "abc"));
        assert_ne!(h(&s, ("a", "bc")), h(&s, ("ab", "c")));
        assert_ne!(h(&s, ""), h(&s, "\0"));
    }

    #[test]
    fn low_bits_spread_for_strided_keys() {
        // hashbrown indexes buckets by the low bits: strided keys must not collide there
        let s = FastState::default();
        let buckets: HashSet<u64> = (0..4096i64).map(|i| h(&s, i * 1024) & 4095).collect();
        assert!(buckets.len() > 2400, "{} distinct low-bit buckets", buckets.len());
    }
}
