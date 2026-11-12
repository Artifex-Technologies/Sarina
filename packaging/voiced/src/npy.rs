//! The parts of NumPy (2.5) the voice's sampling depends on, written so that the same seed gives the same speech tokens as the
//! Python engine did:
//!
//! - `Generator`: `np.random.default_rng(seed)`, a PCG64 generator seeded through a SeedSequence, with `random()`,
//!   `choice(list)` and `choice(n, p=q)` exactly as NumPy draws them;
//! - `exp_f32`: NumPy's own single-precision `exp` (the AVX2/FMA3 one this processor runs: a Cody-Waite reduction and a
//!   rational polynomial), which is not the C library's `expf` and differs from it in about a third of all inputs;
//! - `sum_f32`: NumPy's pairwise summation, the order `ndarray.sum()` adds a float32 array in.
//!
//! Each is checked against NumPy itself (`sarina-voiced-parity numpy`, tests/parity.py).

// ---------------------------------------------------------------------------------------------------------------------------
// SeedSequence and PCG64

const INIT_A: u32 = 0x43b0_d7e5;
const MULT_A: u32 = 0x931e_8875;
const INIT_B: u32 = 0x8b51_f9dd;
const MULT_B: u32 = 0x58f3_8ded;
const MIX_MULT_L: u32 = 0xca01_f9dd;
const MIX_MULT_R: u32 = 0x4973_f715;
const XSHIFT: u32 = 16;
const POOL: usize = 4;

fn hashmix(value: u32, hash_const: &mut u32) -> u32 {
    let mut v = value ^ *hash_const;
    *hash_const = hash_const.wrapping_mul(MULT_A);
    v = v.wrapping_mul(*hash_const);
    v ^ (v >> XSHIFT)
}

fn mix(x: u32, y: u32) -> u32 {
    let r = MIX_MULT_L.wrapping_mul(x).wrapping_sub(MIX_MULT_R.wrapping_mul(y));
    r ^ (r >> XSHIFT)
}

/// `SeedSequence(seed).generate_state(n_words, np.uint32)` for a seed that is a non-negative integer.
pub fn seed_sequence_state(seed: u128, n_words: usize) -> Vec<u32> {
    // the entropy as 32-bit words, least significant first (0 is one word, [0])
    let mut entropy = Vec::new();
    let mut s = seed;
    loop {
        entropy.push(s as u32);
        s >>= 32;
        if s == 0 {
            break;
        }
    }
    let mut pool = [0u32; POOL];
    let mut hc = INIT_A;
    for (i, slot) in pool.iter_mut().enumerate() {
        *slot = hashmix(if i < entropy.len() { entropy[i] } else { 0 }, &mut hc);
    }
    for i_src in 0..POOL {
        for i_dst in 0..POOL {
            if i_src != i_dst {
                let h = hashmix(pool[i_src], &mut hc);
                pool[i_dst] = mix(pool[i_dst], h);
            }
        }
    }
    for &e in entropy.iter().skip(POOL) {
        for slot in pool.iter_mut() {
            let h = hashmix(e, &mut hc);
            *slot = mix(*slot, h);
        }
    }
    let mut hash_const = INIT_B;
    let mut out = Vec::with_capacity(n_words);
    for i in 0..n_words {
        let mut v = pool[i % POOL];
        v ^= hash_const;
        hash_const = hash_const.wrapping_mul(MULT_B);
        v = v.wrapping_mul(hash_const);
        v ^= v >> XSHIFT;
        out.push(v);
    }
    out
}

const PCG_MULT: u128 = (0x2360_ed05_1fc6_5da4u128 << 64) | 0x4385_df64_9fcc_f645u128;

/// NumPy's `Generator(PCG64(seed))`, which is what `np.random.default_rng(seed)` makes. Cloning it keeps the state, so a
/// draw can be taken back by keeping a copy from before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generator {
    state: u128,
    inc: u128,
    has_uint32: bool,
    uinteger: u32,
}

impl Generator {
    pub fn new(seed: u128) -> Generator {
        let w = seed_sequence_state(seed, 8);
        let v = |i: usize| (w[2 * i] as u64) | ((w[2 * i + 1] as u64) << 32);
        let initstate = ((v(0) as u128) << 64) | v(1) as u128;
        let initseq = ((v(2) as u128) << 64) | v(3) as u128;
        let mut g = Generator { state: 0, inc: (initseq << 1) | 1, has_uint32: false, uinteger: 0 };
        g.step();
        g.state = g.state.wrapping_add(initstate);
        g.step();
        g
    }

    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let s = self.state;
        let rot = (s >> 122) as u32;
        (((s >> 64) as u64) ^ (s as u64)).rotate_right(rot)
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.has_uint32 {
            self.has_uint32 = false;
            return self.uinteger;
        }
        let n = self.next_u64();
        self.has_uint32 = true;
        self.uinteger = (n >> 32) as u32;
        n as u32
    }

    /// `rng.random()`: a double in [0, 1).
    pub fn random(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9007199254740992.0)
    }

    /// `rng.integers(0, n)` for 1 <= n <= 2**32 (Lemire's method on 32-bit draws, as NumPy does for such a range).
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n >= 1 && n <= 1 << 32);
        let rng = (n - 1) as u32;
        if rng == 0 {
            return 0;
        }
        if rng == u32::MAX {
            return self.next_u32() as u64;
        }
        let excl = rng + 1;
        let mut m = self.next_u32() as u64 * excl as u64;
        let mut leftover = m as u32;
        if leftover < excl {
            let threshold = (u32::MAX - rng) % excl;
            while leftover < threshold {
                m = self.next_u32() as u64 * excl as u64;
                leftover = m as u32;
            }
        }
        m >> 32
    }

    /// `rng.choice(items)` for a list.
    pub fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }

    /// `rng.choice(len(p), p=p)` for a float32 array `p`: the checks NumPy makes first (each failure is a ValueError there),
    /// then one double drawn and looked up in the cumulative sums.
    pub fn choice_p(&mut self, p: &[f32]) -> Result<usize, &'static str> {
        if p.is_empty() {
            return Err("a cannot be empty unless no samples are taken");
        }
        let atol = f64::EPSILON.sqrt().max((f32::EPSILON as f64).sqrt());
        let pd: Vec<f64> = p.iter().map(|&x| x as f64).collect();
        // kahan_sum
        let mut sum = pd[0];
        let mut c = 0.0f64;
        for &x in &pd[1..] {
            let y = x - c;
            let t = sum + y;
            c = (t - sum) - y;
            sum = t;
        }
        if sum.is_nan() {
            return Err("probabilities contain NaN");
        }
        if pd.iter().any(|&x| x < 0.0) {
            return Err("probabilities are not non-negative");
        }
        if (sum - 1.0).abs() > atol {
            return Err("probabilities do not sum to 1");
        }
        let mut cdf = Vec::with_capacity(pd.len());
        let mut acc = 0.0f64;
        for (i, &x) in pd.iter().enumerate() {
            acc = if i == 0 { x } else { acc + x };
            cdf.push(acc);
        }
        let last = *cdf.last().unwrap();
        for v in cdf.iter_mut() {
            *v /= last;
        }
        let u = self.random();
        // searchsorted(u, side="right"): the first index whose value is greater than u
        Ok(cdf.partition_point(|&v| v <= u))
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// exp for float32, as NumPy computes it on an AVX2/FMA3 processor (simd_exp_f32 in loops_exponent_log.dispatch.c.src)

const XMAX: f32 = 88.72283935546875;
const XMIN: f32 = -103.97208404541015625;
const CVT_MAGIC: f32 = 12582912.0; // 0x1.8p+23
const LOG2E: f32 = std::f32::consts::LOG2_E;
const LN2_HI: f32 = -6.93145752e-1;
const LN2_LO: f32 = -1.42860677e-6;
const P0: f32 = 9.999999999980870924916e-01;
const P1: f32 = 7.257664613233124478488e-01;
const P2: f32 = 2.473615434895520810817e-01;
const P3: f32 = 5.114512081637298353406e-02;
const P4: f32 = 6.757896990527504603057e-03;
const P5: f32 = 5.082762527590693718096e-04;
const Q0: f32 = 1.000000000000000000000e+00;
const Q1: f32 = -2.742335390411667452936e-01;
const Q2: f32 = 2.159509375685829852307e-02;

/// `np.exp` of one float32, bit for bit what NumPy's vector loop gives.
pub fn exp_f32(x: f32) -> f32 {
    if x.is_nan() {
        return f32::NAN;
    }
    if x >= XMAX {
        return f32::INFINITY;
    }
    if x <= XMIN {
        return 0.0;
    }
    let quadrant = ((x * LOG2E) + CVT_MAGIC) - CVT_MAGIC;
    let mut r = quadrant.mul_add(LN2_HI, x);
    r = quadrant.mul_add(LN2_LO, r);
    r = quadrant.mul_add(0.0, r);
    let mut num = P5.mul_add(r, P4);
    num = num.mul_add(r, P3);
    num = num.mul_add(r, P2);
    num = num.mul_add(r, P1);
    num = num.mul_add(r, P0);
    let mut den = Q2.mul_add(r, Q1);
    den = den.mul_add(r, Q0);
    let poly = num / den;
    scalef(poly, quadrant)
}

/// poly * 2**quadrant the way avx2_scalef_ps does it: adding to the exponent bits, and for quadrant <= -125 (a subnormal
/// result) through 2**-125 and a division.
fn scalef(poly: f32, quadrant: f32) -> f32 {
    let q = quadrant as i32;
    if quadrant <= -125.0 {
        let diff = -(quadrant - -125.0); // >= 0
        let two_power_diff = 1i32.wrapping_shl(diff as i32 as u32);
        let p = f32::from_bits((poly.to_bits() as i32).wrapping_add((-125i32) << 23) as u32);
        p / two_power_diff as f32
    } else {
        f32::from_bits((poly.to_bits() as i32).wrapping_add(q << 23) as u32)
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// sums

/// `a.sum()` of a contiguous float32 array: NumPy's pairwise summation (blocks of up to 128, eight running sums in each).
pub fn sum_f32(a: &[f32]) -> f32 {
    0.0f32 + pairwise(a)
}

fn pairwise(a: &[f32]) -> f32 {
    let n = a.len();
    if n < 8 {
        let mut res = 0.0f32;
        for &x in a {
            res += x;
        }
        // NumPy starts this case from -0.0 so that a sum of negative zeros stays -0.0; for any other input it is the same.
        res
    } else if n <= 128 {
        let mut r = [0f32; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - (n % 8) {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise(&a[..n2]) + pairwise(&a[n2..])
    }
}
