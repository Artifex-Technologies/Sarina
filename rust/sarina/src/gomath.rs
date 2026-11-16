//! Go's `math.Exp` as Go runs it on amd64: the assembly version (math/exp_amd64.s, after Naoki Shibata's SLEEF method,
//! public domain), which has two paths: with FMA instructions when the CPU has AVX and FMA, plain multiplies and adds
//! otherwise, and the two can differ in the last bit. The port takes the same path on the same CPU. Rust's `f64::exp`
//! (the C library's) is not Go's, and the vector index's levels are built from it.
//!
//! And Go's `math.Log` on amd64 (math/log_amd64.s, FreeBSD's method in assembly; one path), which the session search ranks
//! with.

// Go's constants as its assembly spells them (each rounds to the same float64 as Go's assembler makes of it)
#![allow(clippy::excessive_precision, clippy::approx_constant)]

const LN2U: f64 = 0.693_147_180_559_662_956_511_601_805_686_950_683_593_75;
const LN2L: f64 = 0.282_352_905_630_315_771_225_884_481_750_134_360_255_254_120_68e-12;
const LOG2E: f64 = 1.442_695_040_888_963_407_359_924_681_001_892_0;
const OVERFLOW: f64 = 7.097_827_128_933_84e2;
const T: [f64; 9] = [
    0.5,
    1.0,
    2.0,
    1.666_666_666_666_666_666_7e-1,
    4.166_666_666_666_666_666_7e-2,
    8.333_333_333_333_333_333_3e-3,
    1.388_888_888_888_888_888_9e-3,
    1.984_126_984_126_984_127_0e-4,
    2.480_158_730_158_730_158_7e-5,
];

/// Whether Go's Exp takes its FMA path on this CPU (Go: `cpu.X86.HasAVX && cpu.X86.HasFMA`).
pub fn use_fma() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx") && std::arch::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// Go's `math.Exp` on amd64.
pub fn exp(x: f64) -> f64 {
    exp_with(x, use_fma())
}

/// Go's `math.Exp` on amd64 with the given path.
pub fn exp_with(x: f64, fma: bool) -> f64 {
    let bits = x.to_bits();
    if bits & !(1 << 63) >= 0x7FF0_0000_0000_0000 {
        // NaN or an infinity: -Inf gives 0, the others themselves
        return if bits == 0xFFF0_0000_0000_0000 { 0.0 } else { x };
    }
    if x > OVERFLOW {
        return f64::INFINITY;
    }
    // CVTSD2SL: rounded to nearest (even), the "integer indefinite" value when out of range
    let t = LOG2E * x;
    let mut e: i32 = if (-2_147_483_648.5..2_147_483_647.5).contains(&t) { t.round_ties_even() as i32 } else { i32::MIN };
    let k = e as f64;
    let mut r;
    if fma {
        r = (-k).mul_add(LN2U, x);
        r = (-k).mul_add(LN2L, r);
        r *= 0.0625;
        let mut p = T[8];
        for c in [T[7], T[6], T[5], T[4], T[3], T[0], T[1]] {
            p = p.mul_add(r, c);
        }
        r *= p;
        for _ in 0..3 {
            let s = r + T[2];
            r *= s;
        }
        let s = r + T[2];
        r = r.mul_add(s, T[1]);
    } else {
        r = x - LN2U * k;
        r -= LN2L * k;
        r *= 0.0625;
        let mut p = T[8];
        for c in [T[7], T[6], T[5], T[4], T[3], T[0], T[1]] {
            p = p * r + c;
        }
        r *= p;
        for _ in 0..4 {
            let s = T[2] + r;
            r *= s;
        }
        r += T[1];
    }
    // fr * 2**exponent
    e = e.wrapping_add(0x3FF);
    if e <= 0 {
        if e < -52 {
            return 0.0;
        }
        r *= f64::from_bits(((e + 0x3FE) as u64) << 52);
        e = 1;
    } else if e >= 0x7FF {
        return f64::INFINITY;
    }
    r * f64::from_bits((e as u64) << 52)
}

/// Go's `math.Log` as amd64 runs it (math/log_amd64.s): the same operations in the same order, including its reading of the
/// exponent (no special case for subnormals) and its `f1 <= sqrt(2)/2` test.
pub fn log(x: f64) -> f64 {
    const HSQRT2: f64 = 7.07106781186547524401e-01;
    const LN2HI: f64 = 6.93147180369123816490e-01;
    const LN2LO: f64 = 1.90821492927058770002e-10;
    const L1: f64 = 6.666666666666735130e-01;
    const L2: f64 = 3.999999999940941908e-01;
    const L3: f64 = 2.857142874366239149e-01;
    const L4: f64 = 2.222219843214978396e-01;
    const L5: f64 = 1.818357216161805012e-01;
    const L6: f64 = 1.531383769920937332e-01;
    const L7: f64 = 1.479819860511658591e-01;
    let bits = x.to_bits();
    if bits & !(1u64 << 63) == 0 {
        return f64::NEG_INFINITY;
    }
    if (bits as i64) < 0 {
        return f64::from_bits(0x7FF8000000000001);
    }
    if bits as i64 >= 0x7FF0000000000000 {
        return x;
    }
    let mut f1 = f64::from_bits((bits & 0x000F_FFFF_FFFF_FFFF) | 0.5f64.to_bits());
    let mut k = (((bits >> 52) & 0x7FF) as i32 - 0x3FE) as f64;
    // CMPSD's "not less than": HSqrt2 < f1 is false
    let adj = if HSQRT2 < f1 { 0.0 } else { 1.0 };
    k -= adj;
    f1 *= adj + 1.0;
    let f = f1 - 1.0;
    let s = f / (2.0 + f);
    let s2 = s * s;
    let s4 = s2 * s2;
    let t1 = s2 * ((((L7 * s4) + L5) * s4 + L3) * s4 + L1);
    let t2 = s4 * (((L6 * s4) + L4) * s4 + L2);
    let r = t1 + t2;
    let hfsq = 0.5 * f * f;
    let a = s * (hfsq + r) + k * LN2LO;
    k * LN2HI - ((hfsq - a) - f)
}

/// Go's `math.Frexp`: a fraction in [0.5, 1) and a power of two.
pub fn frexp(f: f64) -> (f64, i64) {
    if f == 0.0 || f.is_infinite() || f.is_nan() {
        return (f, 0);
    }
    let (f, mut exp) = if f.abs() < f64::MIN_POSITIVE { (f * (1u64 << 52) as f64, -52) } else { (f, 0) };
    let mut x = f.to_bits();
    exp += ((x >> 52) & 0x7ff) as i64 - 1023 + 1;
    x &= !(0x7ffu64 << 52);
    x |= 1022u64 << 52;
    (f64::from_bits(x), exp)
}

/// Go's `math.Log2` (pure Go on amd64, on its assembly `Log`): exact for powers of two.
pub fn log2(x: f64) -> f64 {
    let (frac, exp) = frexp(x);
    if frac == 0.5 {
        return (exp - 1) as f64;
    }
    log(frac) * std::f64::consts::LOG2_E + exp as f64
}

// ---- Go's math.Sin, Cos and Sincos (math/sin.go, sincos.go, trig_reduce.go: Cephes' method, pure Go on amd64) ------------

const SIN: [f64; 6] = [
    1.58962301576546568060e-10,
    -2.50507477628578072866e-8,
    2.75573136213857245213e-6,
    -1.98412698295895385996e-4,
    8.33333333332211858878e-3,
    -1.66666666666666307295e-1,
];
const COS: [f64; 6] = [
    -1.13585365213876817300e-11,
    2.08757008419747316778e-9,
    -2.75573141792967388112e-7,
    2.48015872888517045348e-5,
    -1.38888888888730564116e-3,
    4.16666666666665929218e-2,
];
// Pi/4 split into three parts
const PI4A: f64 = 7.85398125648498535156e-1;
const PI4B: f64 = 3.77489470793079817668e-8;
const PI4C: f64 = 2.69515142907905952645e-15;
const REDUCE_THRESHOLD: f64 = (1u64 << 29) as f64;
// 4/Pi as Go's constant arithmetic rounds it
const FOUR_OVER_PI: f64 = 1.273_239_544_735_162_686_151_070_106_980_114_896_275_677_165_923_651_589_981_338_752_471_174_381_073_8;
const M_PI4: [u64; 20] = [
    0x0000000000000001,
    0x45f306dc9c882a53,
    0xf84eafa3ea69bb81,
    0xb6c52b3278872083,
    0xfca2c757bd778ac3,
    0x6e48dc74849ba5c0,
    0x0c925dd413a32439,
    0xfc3bd63962534e7d,
    0xd1046bea5d768909,
    0xd338e04d68befc82,
    0x7323ac7306a673e9,
    0x3908bf177bf25076,
    0x3ff12fffbc0b301f,
    0xde5e2316b414da3e,
    0xda6cfd9e4f96136e,
    0x9e8c7ecd3cbfd45a,
    0xea4f758fd7cbe2f6,
    0x7a0e73ef14a525d4,
    0xd7f6bf623f1aba10,
    0xac06608df8f6d757,
];

/// Go's shifts: by 64 or more gives 0.
fn shl(x: u64, n: u32) -> u64 {
    x.checked_shl(n).unwrap_or(0)
}
fn shr(x: u64, n: u32) -> u64 {
    x.checked_shr(n).unwrap_or(0)
}

/// Go's `trigReduce` (Payne and Hanek): x (at least 2^29) modulo Pi/4, its octant and the remainder.
fn trig_reduce(x: f64) -> (u64, f64) {
    const PI4: f64 = std::f64::consts::FRAC_PI_4;
    if x < PI4 {
        return (0, x);
    }
    let mut ix = x.to_bits();
    let exp = ((ix >> 52) & 0x7ff) as i64 - 1023 - 52;
    ix &= !(0x7ffu64 << 52);
    ix |= 1 << 52;
    let (digit, bitshift) = (((exp + 61) as u64 / 64) as usize, ((exp + 61) as u64 % 64) as u32);
    let z0 = shl(M_PI4[digit], bitshift) | shr(M_PI4[digit + 1], 64 - bitshift);
    let z1 = shl(M_PI4[digit + 1], bitshift) | shr(M_PI4[digit + 2], 64 - bitshift);
    let z2 = shl(M_PI4[digit + 2], bitshift) | shr(M_PI4[digit + 3], 64 - bitshift);
    let z2hi = ((z2 as u128 * ix as u128) >> 64) as u64;
    let z1p = z1 as u128 * ix as u128;
    let (z1hi, z1lo) = ((z1p >> 64) as u64, z1p as u64);
    let z0lo = z0.wrapping_mul(ix);
    let (lo, c) = z1lo.overflowing_add(z2hi);
    let mut hi = z0lo.wrapping_add(z1hi).wrapping_add(c as u64);
    let mut j = hi >> 61;
    hi = hi << 3 | lo >> 61;
    let lz = hi.leading_zeros();
    let e = 1023 - (lz as u64 + 1);
    hi = shl(hi, lz + 1) | shr(lo, 64 - (lz + 1));
    hi >>= 64 - 52;
    hi |= e << 52;
    let mut z = f64::from_bits(hi);
    if j & 1 == 1 {
        j += 1;
        j &= 7;
        z -= 1.0;
    }
    (j, z * PI4)
}

/// The octant and the remainder, as sin, cos and Sincos all reduce.
fn reduce(x: f64) -> (u64, f64) {
    if x >= REDUCE_THRESHOLD {
        return trig_reduce(x);
    }
    let mut j = (x * FOUR_OVER_PI) as u64;
    let mut y = j as f64;
    if j & 1 == 1 {
        j += 1;
        y += 1.0;
    }
    j &= 7;
    (j, ((x - y * PI4A) - y * PI4B) - y * PI4C)
}

fn sin_poly(z: f64, zz: f64) -> f64 {
    z + z * zz * (((((SIN[0] * zz + SIN[1]) * zz + SIN[2]) * zz + SIN[3]) * zz + SIN[4]) * zz + SIN[5])
}
fn cos_poly(zz: f64) -> f64 {
    1.0 - 0.5 * zz + zz * zz * (((((COS[0] * zz + COS[1]) * zz + COS[2]) * zz + COS[3]) * zz + COS[4]) * zz + COS[5])
}

/// Go's `math.Sin`.
pub fn sin(x: f64) -> f64 {
    if x == 0.0 || x.is_nan() {
        return x;
    }
    if x.is_infinite() {
        return f64::NAN;
    }
    let (mut sign, x) = if x < 0.0 { (true, -x) } else { (false, x) };
    let (mut j, z) = reduce(x);
    if j > 3 {
        sign = !sign;
        j -= 4;
    }
    let zz = z * z;
    let y = if j == 1 || j == 2 { cos_poly(zz) } else { sin_poly(z, zz) };
    if sign {
        -y
    } else {
        y
    }
}

/// Go's `math.Cos`.
pub fn cos(x: f64) -> f64 {
    if x.is_nan() || x.is_infinite() {
        return f64::NAN;
    }
    let mut sign = false;
    let (mut j, z) = reduce(x.abs());
    if j > 3 {
        j -= 4;
        sign = !sign;
    }
    if j > 1 {
        sign = !sign;
    }
    let zz = z * z;
    let y = if j == 1 || j == 2 { sin_poly(z, zz) } else { cos_poly(zz) };
    if sign {
        -y
    } else {
        y
    }
}

/// Go's `math.Sincos`.
pub fn sincos(x: f64) -> (f64, f64) {
    if x == 0.0 {
        return (x, 1.0);
    }
    if x.is_nan() || x.is_infinite() {
        return (f64::NAN, f64::NAN);
    }
    let (mut sin_sign, mut cos_sign) = (false, false);
    let x = if x < 0.0 {
        sin_sign = true;
        -x
    } else {
        x
    };
    let (mut j, z) = reduce(x);
    if j > 3 {
        j -= 4;
        sin_sign = !sin_sign;
        cos_sign = !cos_sign;
    }
    if j > 1 {
        cos_sign = !cos_sign;
    }
    let zz = z * z;
    let (mut c, mut s) = (cos_poly(zz), sin_poly(z, zz));
    if j == 1 || j == 2 {
        std::mem::swap(&mut s, &mut c);
    }
    if cos_sign {
        c = -c;
    }
    if sin_sign {
        s = -s;
    }
    (s, c)
}

#[cfg(test)]
mod tests {
    use super::exp_with;

    /// Go's two paths (with and without FMA) stay within one unit in the last place of each other, and within two of the
    /// C library's exp (which is why Rust's own exp cannot stand in), over the normal range (below about -708 the results are subnormal and Go's two-step scaling rounds
    /// twice); the fixture holds the path of the machine it was made on.
    #[test]
    fn the_two_paths_agree_to_one_ulp() {
        let mut x = -700.0;
        while x < 709.0 {
            let (a, b, c) = (exp_with(x, true), exp_with(x, false), x.exp());
            for (p, q, most) in [(a, b, 1), (a, c, 2), (b, c, 2)] {
                assert!(
                    (p.to_bits() as i64 - q.to_bits() as i64).abs() <= most,
                    "{x}: {:x} {:x} {:x}",
                    a.to_bits(),
                    b.to_bits(),
                    c.to_bits()
                );
            }
            x += 0.0137;
        }
        assert_eq!(exp_with(f64::NEG_INFINITY, false), 0.0);
        assert_eq!(exp_with(710.0, false), f64::INFINITY);
        assert!(exp_with(f64::NAN, true).is_nan());
    }
}
