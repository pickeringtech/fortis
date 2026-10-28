//! Python's `random.Random` (MT19937, seeded from an integer by `init_by_array`), so that property tests draw the
//! same cases as the definitional interpreter (interpreter/eval.py `run_tests`, `generator_for`).

pub struct PyRandom {
    mt: [u32; 624],
    index: usize,
}

impl PyRandom {
    pub fn new(seed: u128) -> PyRandom {
        let mut key: Vec<u32> = Vec::new();
        let mut s = seed;
        while s > 0 {
            key.push(s as u32);
            s >>= 32;
        }
        if key.is_empty() {
            key.push(0);
        }
        let mut r = PyRandom { mt: [0; 624], index: 624 };
        r.init_genrand(19_650_218);
        let (mut i, mut j) = (1usize, 0usize);
        let n = 624usize;
        let mut k = n.max(key.len());
        while k > 0 {
            let prev = r.mt[i - 1];
            r.mt[i] = (r.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_664_525))).wrapping_add(key[j]).wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= n {
                r.mt[0] = r.mt[n - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        k = n - 1;
        while k > 0 {
            let prev = r.mt[i - 1];
            r.mt[i] = (r.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_566_083_941))).wrapping_sub(i as u32);
            i += 1;
            if i >= n {
                r.mt[0] = r.mt[n - 1];
                i = 1;
            }
            k -= 1;
        }
        r.mt[0] = 0x8000_0000;
        r
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..624 {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32.wrapping_mul(prev ^ (prev >> 30)).wrapping_add(i as u32);
        }
        self.index = 624;
    }

    pub fn genrand_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7FFF_FFFF);
                let mut v = self.mt[(i + 397) % 624] ^ (y >> 1);
                if y & 1 == 1 {
                    v ^= 0x9908_B0DF;
                }
                self.mt[i] = v;
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^= y >> 18;
        y
    }

    /// `random()`: 53 random bits as a float in [0, 1).
    pub fn random(&mut self) -> f64 {
        let a = (self.genrand_u32() >> 5) as f64;
        let b = (self.genrand_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    pub fn getrandbits(&mut self, k: u32) -> u128 {
        if k <= 32 {
            return (self.genrand_u32() >> (32 - k)) as u128;
        }
        let mut out: u128 = 0;
        let mut got = 0u32;
        while got < k {
            let take = (k - got).min(32);
            let r = self.genrand_u32() >> (32 - take);
            out |= (r as u128) << got;
            got += take;
        }
        out
    }

    /// `_randbelow(n)` by rejection sampling on `n.bit_length()` bits.
    pub fn below(&mut self, n: u128) -> u128 {
        if n == 0 {
            return 0;
        }
        let k = 128 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }

    /// `randint(a, b)`, both ends included.
    pub fn randint(&mut self, a: i128, b: i128) -> i128 {
        a + self.below((b - a + 1) as u128) as i128
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_cpython() {
        // python3 -c "import random; r=random.Random(42); print([r.randint(0,100) for _ in range(5)], r.random())"
        let mut r = PyRandom::new(42);
        let xs: Vec<i128> = (0..5).map(|_| r.randint(0, 100)).collect();
        assert_eq!(xs, vec![81, 14, 3, 94, 35]);
        let mut r = PyRandom::new(1);
        assert_eq!(r.random(), 0.13436424411240122);
    }
}
