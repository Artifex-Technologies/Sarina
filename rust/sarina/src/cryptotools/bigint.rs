//! Unsigned big numbers for checking RSA and ECDSA signatures and printing serial numbers (no bigint crate is in the
//! registry): little-endian 32-bit limbs, schoolbook multiplication, Knuth's division. Only public values pass through
//! here (keys, signatures, digests), so nothing needs to run in constant time.

use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Nat(Vec<u32>);

impl Nat {
    pub(crate) fn zero() -> Nat {
        Nat(Vec::new())
    }

    pub(crate) fn from_u64(v: u64) -> Nat {
        Nat(vec![v as u32, (v >> 32) as u32]).norm()
    }

    fn norm(mut self) -> Nat {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
        self
    }

    /// A big-endian unsigned number.
    pub(crate) fn from_be(b: &[u8]) -> Nat {
        let mut limbs = vec![0u32; b.len().div_ceil(4)];
        for (i, &x) in b.iter().rev().enumerate() {
            limbs[i / 4] |= u32::from(x) << (8 * (i % 4));
        }
        Nat(limbs).norm()
    }

    /// Big-endian bytes, at least `len` of them (padded with zeros in front).
    pub(crate) fn to_be(&self, len: usize) -> Vec<u8> {
        let mut out: Vec<u8> = self.0.iter().rev().flat_map(|l| l.to_be_bytes()).skip_while(|&b| b == 0).collect();
        if out.len() < len {
            let mut padded = vec![0; len - out.len()];
            padded.append(&mut out);
            return padded;
        }
        out
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn is_odd(&self) -> bool {
        self.0.first().is_some_and(|l| l & 1 == 1)
    }

    pub(crate) fn bit_len(&self) -> usize {
        self.0.last().map_or(0, |l| 32 * self.0.len() - l.leading_zeros() as usize)
    }

    pub(crate) fn bit(&self, i: usize) -> bool {
        self.0.get(i / 32).is_some_and(|l| (l >> (i % 32)) & 1 == 1)
    }

    pub(crate) fn add(&self, o: &Nat) -> Nat {
        let (a, b) = if self.0.len() >= o.0.len() { (self, o) } else { (o, self) };
        let mut out = Vec::with_capacity(a.0.len() + 1);
        let mut carry = 0u64;
        for (i, &x) in a.0.iter().enumerate() {
            let s = u64::from(x) + u64::from(*b.0.get(i).unwrap_or(&0)) + carry;
            out.push(s as u32);
            carry = s >> 32;
        }
        out.push(carry as u32);
        Nat(out).norm()
    }

    /// self - o, for self >= o.
    pub(crate) fn sub(&self, o: &Nat) -> Nat {
        let mut out = Vec::with_capacity(self.0.len());
        let mut borrow = 0i64;
        for (i, &x) in self.0.iter().enumerate() {
            let d = i64::from(x) - i64::from(*o.0.get(i).unwrap_or(&0)) - borrow;
            out.push(d as u32);
            borrow = i64::from(d < 0);
        }
        debug_assert_eq!(borrow, 0, "Nat::sub underflow");
        Nat(out).norm()
    }

    pub(crate) fn mul(&self, o: &Nat) -> Nat {
        if self.is_zero() || o.is_zero() {
            return Nat::zero();
        }
        let mut out = vec![0u32; self.0.len() + o.0.len()];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.0.iter().enumerate() {
                let t = u64::from(a) * u64::from(b) + u64::from(out[i + j]) + carry;
                out[i + j] = t as u32;
                carry = t >> 32;
            }
            out[i + o.0.len()] = carry as u32;
        }
        Nat(out).norm()
    }

    /// The quotient and remainder by a non-zero divisor (Knuth's algorithm D).
    pub(crate) fn divrem(&self, v: &Nat) -> (Nat, Nat) {
        assert!(!v.is_zero(), "Nat division by zero");
        if self.cmp(v) == Ordering::Less {
            return (Nat::zero(), self.clone());
        }
        let n = v.0.len();
        if n == 1 {
            let d = u64::from(v.0[0]);
            let mut q = vec![0u32; self.0.len()];
            let mut r = 0u64;
            for i in (0..self.0.len()).rev() {
                let cur = (r << 32) | u64::from(self.0[i]);
                q[i] = (cur / d) as u32;
                r = cur % d;
            }
            return (Nat(q).norm(), Nat::from_u64(r));
        }
        let m = self.0.len() - n;
        let s = v.0[n - 1].leading_zeros();
        let shl = |x: &[u32], extra: usize| -> Vec<u32> {
            let mut out = vec![0u32; x.len() + extra];
            for i in 0..x.len() {
                out[i] |= x[i] << s;
                if s > 0 {
                    out[i + 1] |= x[i] >> (32 - s);
                }
            }
            out
        };
        let vn = {
            let mut w = shl(&v.0, 1);
            w.truncate(n);
            w
        };
        let mut un = shl(&self.0, 1);
        let mut q = vec![0u32; m + 1];
        let b = 1u64 << 32;
        for j in (0..=m).rev() {
            let num = (u64::from(un[j + n]) << 32) | u64::from(un[j + n - 1]);
            let mut qhat = num / u64::from(vn[n - 1]);
            let mut rhat = num % u64::from(vn[n - 1]);
            while qhat >= b || qhat * u64::from(vn[n - 2]) > ((rhat << 32) | u64::from(un[j + n - 2])) {
                qhat -= 1;
                rhat += u64::from(vn[n - 1]);
                if rhat >= b {
                    break;
                }
            }
            let mut k = 0i64;
            for i in 0..n {
                let p = qhat * u64::from(vn[i]);
                let t = i64::from(un[i + j]) - k - (p & 0xffff_ffff) as i64;
                un[i + j] = t as u32;
                k = (p >> 32) as i64 - (t >> 32);
            }
            let t = i64::from(un[j + n]) - k;
            un[j + n] = t as u32;
            q[j] = qhat as u32;
            if t < 0 {
                q[j] = q[j].wrapping_sub(1);
                let mut c = 0u64;
                for i in 0..n {
                    let t = u64::from(un[i + j]) + u64::from(vn[i]) + c;
                    un[i + j] = t as u32;
                    c = t >> 32;
                }
                un[j + n] = un[j + n].wrapping_add(c as u32);
            }
        }
        let mut r = vec![0u32; n];
        for i in 0..n {
            r[i] = un[i] >> s;
            if s > 0 {
                r[i] |= un[i + 1] << (32 - s);
            }
        }
        (Nat(q).norm(), Nat(r).norm())
    }

    pub(crate) fn rem(&self, m: &Nat) -> Nat {
        self.divrem(m).1
    }

    pub(crate) fn mul_mod(&self, o: &Nat, m: &Nat) -> Nat {
        self.mul(o).rem(m)
    }

    /// self^e mod m, e as big-endian bits of `e`.
    pub(crate) fn pow_mod(&self, e: &Nat, m: &Nat) -> Nat {
        let mut r = Nat::from_u64(1).rem(m);
        let base = self.rem(m);
        for i in (0..e.bit_len()).rev() {
            r = r.mul_mod(&r, m);
            if e.bit(i) {
                r = r.mul_mod(&base, m);
            }
        }
        r
    }

    /// The decimal digits (Go's big.Int String for a non-negative number).
    pub(crate) fn decimal(&self) -> String {
        if self.is_zero() {
            return "0".into();
        }
        let mut parts = Vec::new();
        let mut x = self.clone();
        let chunk = Nat::from_u64(1_000_000_000);
        while !x.is_zero() {
            let (q, r) = x.divrem(&chunk);
            parts.push(r.0.first().copied().unwrap_or(0));
            x = q;
        }
        let mut s = parts.pop().unwrap_or(0).to_string();
        for p in parts.iter().rev() {
            s += &format!("{p:09}");
        }
        s
    }
}

impl PartialOrd for Nat {
    fn partial_cmp(&self, o: &Nat) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Nat {
    fn cmp(&self, o: &Nat) -> Ordering {
        self.0.len().cmp(&o.0.len()).then_with(|| self.0.iter().rev().cmp(o.0.iter().rev()))
    }
}

#[cfg(test)]
mod tests {
    use super::Nat;

    #[test]
    fn division_and_powers_agree_with_small_numbers() {
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..2000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let a = x >> (x % 64);
            let b = (x.rotate_left(17) >> (x % 61)).max(1);
            let (q, r) = Nat::from_u64(a).divrem(&Nat::from_u64(b));
            assert_eq!((q, r), (Nat::from_u64(a / b), Nat::from_u64(a % b)), "{a} / {b}");
            assert_eq!(Nat::from_u64(a).decimal(), a.to_string());
        }
        // a multi-limb check: (2^200 + 12345) mod (2^97 + 3) by q*d + r == n
        let n = Nat::from_be(&{
            let mut v = vec![0u8; 26];
            v[0] = 1;
            v[24] = 0x30;
            v[25] = 0x39;
            v
        });
        let d = Nat::from_be(&{
            let mut v = vec![0u8; 13];
            v[0] = 2;
            v[12] = 3;
            v
        });
        let (q, r) = n.divrem(&d);
        assert!(r < d);
        assert_eq!(q.mul(&d).add(&r), n);
        // Knuth's "add back" step (Hacker's Delight's test case, in 32-bit limbs): the estimated digit is one too big
        let n = Nat(vec![0, 0, 0x8000_0000, 0x7fff_ffff]);
        let d = Nat(vec![1, 0, 0x8000_0000]);
        let (q, r) = n.divrem(&d);
        assert!(r < d);
        assert_eq!(q.mul(&d).add(&r), n);
        assert_eq!(
            Nat::from_u64(3).pow_mod(&Nat::from_u64(200), &Nat::from_u64(1_000_000_007)),
            Nat::from_u64(3u64.pow(20) % 1_000_000_007).pow_mod(&Nat::from_u64(10), &Nat::from_u64(1_000_000_007))
        );
    }
}
