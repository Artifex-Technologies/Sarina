//! Go's `math.Exp` as Go runs it on amd64: the assembly version (math/exp_amd64.s, after Naoki Shibata's SLEEF method,
//! public domain), which has two paths: with FMA instructions when the CPU has AVX and FMA, plain multiplies and adds
//! otherwise, and the two can differ in the last bit. The port takes the same path on the same CPU. Rust's `f64::exp`
//! (the C library's) is not Go's, and the vector index's levels are built from it.

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
