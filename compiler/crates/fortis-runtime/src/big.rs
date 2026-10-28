//! A minimal unsigned big integer: enough exact arithmetic for Decimal (96-bit coefficients scaled by powers of
//! ten up to 10^56) without a dependency.

use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big {
    /// little-endian base-2^32 limbs, no trailing zero limbs
    limbs: Vec<u32>,
}

impl Big {
    pub fn zero() -> Big {
        Big { limbs: vec![] }
    }
    pub fn from_u128(mut v: u128) -> Big {
        let mut limbs = Vec::new();
        while v > 0 {
            limbs.push(v as u32);
            v >>= 32;
        }
        Big { limbs }
    }
    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }
    fn trim(mut self) -> Big {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
        self
    }
    pub fn to_u128(&self) -> Option<u128> {
        if self.limbs.len() > 4 {
            return None;
        }
        let mut v: u128 = 0;
        for (i, l) in self.limbs.iter().enumerate() {
            v |= (*l as u128) << (32 * i);
        }
        Some(v)
    }
    pub fn pow10(n: u32) -> Big {
        let mut r = Big::from_u128(1);
        for _ in 0..n {
            r = r.mul_small(10);
        }
        r
    }
    pub fn mul_small(&self, m: u32) -> Big {
        let mut out = Vec::with_capacity(self.limbs.len() + 1);
        let mut carry: u64 = 0;
        for l in &self.limbs {
            let v = *l as u64 * m as u64 + carry;
            out.push(v as u32);
            carry = v >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        Big { limbs: out }.trim()
    }
    pub fn mul(&self, o: &Big) -> Big {
        if self.is_zero() || o.is_zero() {
            return Big::zero();
        }
        let mut out = vec![0u64; self.limbs.len() + o.limbs.len() + 1];
        for (i, a) in self.limbs.iter().enumerate() {
            let mut carry = 0u64;
            for (j, b) in o.limbs.iter().enumerate() {
                let cur = out[i + j] + (*a as u64) * (*b as u64) + carry;
                out[i + j] = cur & 0xFFFF_FFFF;
                carry = cur >> 32;
            }
            let mut k = i + o.limbs.len();
            while carry > 0 {
                let cur = out[k] + carry;
                out[k] = cur & 0xFFFF_FFFF;
                carry = cur >> 32;
                k += 1;
            }
        }
        Big { limbs: out.into_iter().map(|x| x as u32).collect() }.trim()
    }
    pub fn add(&self, o: &Big) -> Big {
        let n = self.limbs.len().max(o.limbs.len());
        let mut out = Vec::with_capacity(n + 1);
        let mut carry = 0u64;
        for i in 0..n {
            let v = *self.limbs.get(i).unwrap_or(&0) as u64 + *o.limbs.get(i).unwrap_or(&0) as u64 + carry;
            out.push(v as u32);
            carry = v >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        Big { limbs: out }.trim()
    }
    /// self - o, requiring self >= o
    pub fn sub(&self, o: &Big) -> Big {
        let mut out = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0i64;
        for i in 0..self.limbs.len() {
            let mut v = self.limbs[i] as i64 - *o.limbs.get(i).unwrap_or(&0) as i64 - borrow;
            if v < 0 {
                v += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(v as u32);
        }
        Big { limbs: out }.trim()
    }
    pub fn cmp(&self, o: &Big) -> Ordering {
        if self.limbs.len() != o.limbs.len() {
            return self.limbs.len().cmp(&o.limbs.len());
        }
        for i in (0..self.limbs.len()).rev() {
            match self.limbs[i].cmp(&o.limbs[i]) {
                Ordering::Equal => continue,
                x => return x,
            }
        }
        Ordering::Equal
    }
    fn bits(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(l) => 32 * (self.limbs.len() - 1) + (32 - l.leading_zeros() as usize),
        }
    }
    fn bit(&self, i: usize) -> bool {
        self.limbs.get(i / 32).map_or(false, |l| (l >> (i % 32)) & 1 == 1)
    }
    fn shl1_or(&self, b: bool) -> Big {
        let mut out = Vec::with_capacity(self.limbs.len() + 1);
        let mut carry = b as u32;
        for l in &self.limbs {
            out.push((l << 1) | carry);
            carry = l >> 31;
        }
        if carry > 0 {
            out.push(carry);
        }
        Big { limbs: out }.trim()
    }
    /// (quotient, remainder); `o` must be non-zero
    pub fn divmod(&self, o: &Big) -> (Big, Big) {
        assert!(!o.is_zero(), "division by zero");
        if self.cmp(o) == Ordering::Less {
            return (Big::zero(), self.clone());
        }
        let mut q = vec![0u32; self.limbs.len()];
        let mut r = Big::zero();
        for i in (0..self.bits()).rev() {
            r = r.shl1_or(self.bit(i));
            if r.cmp(o) != Ordering::Less {
                r = r.sub(o);
                q[i / 32] |= 1 << (i % 32);
            }
        }
        (Big { limbs: q }.trim(), r)
    }
    pub fn to_decimal_string(&self) -> String {
        if self.is_zero() {
            return "0".into();
        }
        let mut digits = Vec::new();
        let mut cur = self.clone();
        let ten = Big::from_u128(10);
        while !cur.is_zero() {
            let (q, r) = cur.divmod(&ten);
            digits.push(b'0' + r.to_u128().unwrap() as u8);
            cur = q;
        }
        digits.reverse();
        String::from_utf8(digits).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_agrees_with_u128() {
        let cases: [(u128, u128); 5] = [(0, 1), (7, 3), (u64::MAX as u128, 12345), (10u128.pow(30), 10u128.pow(12) + 7), (u128::MAX / 3, 99)];
        for (a, b) in cases {
            let (ba, bb) = (Big::from_u128(a), Big::from_u128(b));
            assert_eq!(ba.add(&bb).to_decimal_string(), a.checked_add(b).map(|x| x.to_string()).unwrap_or_else(|| ba.add(&bb).to_decimal_string()));
            let (q, r) = ba.divmod(&bb);
            assert_eq!(q.to_u128(), Some(a / b));
            assert_eq!(r.to_u128(), Some(a % b));
            if a >= b {
                assert_eq!(ba.sub(&bb).to_u128(), Some(a - b));
            }
        }
        assert_eq!(Big::pow10(40).to_decimal_string(), format!("1{}", "0".repeat(40)));
        let x = Big::pow10(30).mul(&Big::pow10(26));
        assert_eq!(x, Big::pow10(56));
    }
}
