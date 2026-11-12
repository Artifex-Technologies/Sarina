//! Go's `math/rand` with a seeded source (`rand.New(rand.NewSource(seed))`): the additive lagged Fibonacci generator
//! of Go 1 (Mitchell and Reeds; 607 values, tap 273) seeded through Go's 48271 Lehmer step and its table, and the `Rand`
//! methods Sarina's Go uses. A saved vector index's rotation comes from its seed through this generator, so it must be
//! Go's to the bit (testdata/parity/vecindex.json "rand").

use crate::gorand_cooked::RNG_COOKED;

const RNG_LEN: usize = 607;
const RNG_TAP: usize = 273;
const INT32_MAX: i64 = (1 << 31) - 1;

/// `rand.New(rand.NewSource(seed))`.
#[derive(Clone)]
pub struct Rand {
    tap: usize,
    feed: usize,
    vec: Box<[i64; RNG_LEN]>,
}

fn seedrand(x: i32) -> i32 {
    const A: i32 = 48271;
    const Q: i32 = 44488;
    const R: i32 = 3399;
    let hi = x / Q;
    let lo = x % Q;
    let mut x = A.wrapping_mul(lo).wrapping_sub(R.wrapping_mul(hi));
    if x < 0 {
        x = x.wrapping_add(INT32_MAX as i32);
    }
    x
}

impl Rand {
    pub fn new(seed: i64) -> Rand {
        let mut r = Rand { tap: 0, feed: RNG_LEN - RNG_TAP, vec: Box::new([0; RNG_LEN]) };
        let mut seed = seed % INT32_MAX;
        if seed < 0 {
            seed += INT32_MAX;
        }
        if seed == 0 {
            seed = 89482311;
        }
        let mut x = seed as i32;
        for i in -20i32..RNG_LEN as i32 {
            x = seedrand(x);
            if i >= 0 {
                let mut u = (x as i64) << 40;
                x = seedrand(x);
                u ^= (x as i64) << 20;
                x = seedrand(x);
                u ^= x as i64;
                u ^= RNG_COOKED[i as usize];
                r.vec[i as usize] = u;
            }
        }
        r
    }

    pub fn uint64(&mut self) -> u64 {
        self.tap = if self.tap == 0 { RNG_LEN - 1 } else { self.tap - 1 };
        self.feed = if self.feed == 0 { RNG_LEN - 1 } else { self.feed - 1 };
        let x = self.vec[self.feed].wrapping_add(self.vec[self.tap]);
        self.vec[self.feed] = x;
        x as u64
    }

    pub fn int63(&mut self) -> i64 {
        (self.uint64() & ((1 << 63) - 1)) as i64
    }

    pub fn int31(&mut self) -> i32 {
        (self.int63() >> 32) as i32
    }

    /// Go's `Int63n` (panics for n <= 0, as Go's does).
    pub fn int63n(&mut self, n: i64) -> i64 {
        assert!(n > 0, "invalid argument to Int63n");
        if n & (n - 1) == 0 {
            return self.int63() & (n - 1);
        }
        let max = ((1u64 << 63) - 1 - (1u64 << 63) % n as u64) as i64;
        let mut v = self.int63();
        while v > max {
            v = self.int63();
        }
        v % n
    }

    /// Go's `Int31n`.
    pub fn int31n(&mut self, n: i32) -> i32 {
        assert!(n > 0, "invalid argument to Int31n");
        if n & (n - 1) == 0 {
            return self.int31() & (n - 1);
        }
        let max = ((1u32 << 31) - 1 - (1u32 << 31) % n as u32) as i32;
        let mut v = self.int31();
        while v > max {
            v = self.int31();
        }
        v % n
    }

    /// Go's `Intn` (64-bit int).
    pub fn intn(&mut self, n: i64) -> i64 {
        assert!(n > 0, "invalid argument to Intn");
        if n < (1 << 31) {
            return self.int31n(n as i32) as i64;
        }
        self.int63n(n)
    }

    /// Go's `Perm`: a permutation of 0..n (the first, useless swap kept, as Go 1 must).
    pub fn perm(&mut self, n: usize) -> Vec<usize> {
        let mut m = vec![0usize; n];
        for i in 0..n {
            let j = self.intn(i as i64 + 1) as usize;
            m[i] = m[j];
            m[j] = i;
        }
        m
    }
}
