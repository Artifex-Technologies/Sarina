//! Go's `strconv.ParseFloat(s, 64)`: Go's float syntax (underscores between digits, hexadecimal mantissas with a `p`
//! exponent, the infinities and NaN by name) and its error texts. Rust's `f64::from_str` takes a different set of strings,
//! and a model sends numbers as text often enough (ValidateArgs coerces them) that the set must be Go's.

use super::quote;

/// A failed parse, with the value Go returns beside the error (0 for bad syntax, ±Inf when out of range).
#[derive(Clone, Debug, PartialEq)]
pub struct ParseFloatError {
    pub value: f64,
    pub message: String,
}

fn syntax(s: &str) -> ParseFloatError {
    ParseFloatError { value: 0.0, message: format!("strconv.ParseFloat: parsing {}: invalid syntax", quote(s)) }
}

fn range(s: &str, value: f64) -> ParseFloatError {
    ParseFloatError { value, message: format!("strconv.ParseFloat: parsing {}: value out of range", quote(s)) }
}

/// `strconv.ParseFloat(s, 64)`.
pub fn parse_float(s: &str) -> Result<f64, ParseFloatError> {
    if let Some(v) = special(s) {
        return Ok(v);
    }
    let r = read_float(s.as_bytes()).ok_or_else(|| syntax(s))?;
    let v = if r.hex {
        hex_value(r.mantissa, r.exp, r.trunc, r.neg)
    } else {
        // the syntax is Go's; the value is the correctly rounded one, which Rust's parser gives as Go's does
        let digits: String = s.chars().filter(|&c| c != '_').collect();
        digits.parse::<f64>().map_err(|_| syntax(s))?
    };
    if v.is_infinite() {
        return Err(range(s, v));
    }
    Ok(v)
}

/// "inf", "infinity" (with a sign) and "nan" (without), in any case, as the whole string.
fn special(s: &str) -> Option<f64> {
    let l = s.to_ascii_lowercase();
    let (sign, body) = match l.as_bytes().first() {
        Some(b'+') => (1.0, &l[1..]),
        Some(b'-') => (-1.0, &l[1..]),
        _ => (1.0, &l[..]),
    };
    if body == "inf" || body == "infinity" {
        return Some(sign * f64::INFINITY);
    }
    if l == "nan" {
        return Some(f64::NAN);
    }
    None
}

struct Read {
    mantissa: u64,
    exp: i64,
    neg: bool,
    trunc: bool,
    hex: bool,
}

/// Go's readFloat, which must consume the whole string.
fn read_float(s: &[u8]) -> Option<Read> {
    let mut i = 0;
    let mut neg = false;
    if i >= s.len() {
        return None;
    }
    if s[i] == b'+' {
        i += 1;
    } else if s[i] == b'-' {
        neg = true;
        i += 1;
    }
    let (mut base, mut max_mant, mut exp_char, mut hex) = (10u64, 19, b'e', false);
    if i + 2 < s.len() && s[i] == b'0' && s[i + 1].eq_ignore_ascii_case(&b'x') {
        base = 16;
        max_mant = 16;
        i += 2;
        exp_char = b'p';
        hex = true;
    }
    let (mut underscores, mut sawdot, mut sawdigits, mut trunc) = (false, false, false, false);
    let (mut nd, mut nd_mant, mut dp) = (0i64, 0i64, 0i64);
    let mut mantissa = 0u64;
    while i < s.len() {
        let c = s[i];
        let lc = c.to_ascii_lowercase();
        if c == b'_' {
            underscores = true;
            i += 1;
            continue;
        }
        if c == b'.' {
            if sawdot {
                break;
            }
            sawdot = true;
            dp = nd;
            i += 1;
            continue;
        }
        let d = if c.is_ascii_digit() {
            u64::from(c - b'0')
        } else if base == 16 && (b'a'..=b'f').contains(&lc) {
            u64::from(lc - b'a' + 10)
        } else {
            break;
        };
        sawdigits = true;
        if d == 0 && nd == 0 {
            // leading zeros
            dp -= 1;
            i += 1;
            continue;
        }
        nd += 1;
        if nd_mant < max_mant {
            mantissa = mantissa * base + d;
            nd_mant += 1;
        } else if d != 0 {
            trunc = true;
        }
        i += 1;
    }
    if !sawdigits {
        return None;
    }
    if !sawdot {
        dp = nd;
    }
    if base == 16 {
        dp *= 4;
        nd_mant *= 4;
    }
    if i < s.len() && s[i].to_ascii_lowercase() == exp_char {
        i += 1;
        if i >= s.len() {
            return None;
        }
        let mut esign = 1;
        if s[i] == b'+' {
            i += 1;
        } else if s[i] == b'-' {
            i += 1;
            esign = -1;
        }
        if i >= s.len() || !s[i].is_ascii_digit() {
            return None;
        }
        let mut e = 0i64;
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            if s[i] == b'_' {
                underscores = true;
            } else if e < 10000 {
                e = e * 10 + i64::from(s[i] - b'0');
            }
            i += 1;
        }
        dp += e * esign;
    } else if base == 16 {
        // a hexadecimal mantissa needs its p exponent
        return None;
    }
    if i != s.len() {
        return None;
    }
    if underscores && !underscore_ok(s) {
        return None;
    }
    let exp = if mantissa != 0 { dp - nd_mant } else { 0 };
    Some(Read { mantissa, exp, neg, trunc, hex })
}

/// Go's underscoreOK: underscores only between digits, or between a base prefix and a digit (strconv's ParseFloat and
/// ParseInt share it).
pub fn underscore_ok(mut s: &[u8]) -> bool {
    let mut saw = b'^';
    let mut i = 0;
    if !s.is_empty() && (s[0] == b'-' || s[0] == b'+') {
        s = &s[1..];
    }
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(s[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        saw = b'0';
        hex = s[1].eq_ignore_ascii_case(&b'x');
    }
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() || hex && (b'a'..=b'f').contains(&c.to_ascii_lowercase()) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
        i += 1;
    }
    saw != b'_'
}

/// Go's atofHex for float64: mantissa * 2^exp, rounded to nearest even (trunc: a non-zero digit was dropped).
fn hex_value(mut mantissa: u64, mut exp: i64, trunc: bool, neg: bool) -> f64 {
    const MANTBITS: u32 = 52;
    const MIN_EXP: i64 = -1022;
    const MAX_EXP: i64 = 1023;
    const BIAS: i64 = -1023;
    exp += i64::from(MANTBITS);
    while mantissa != 0 && mantissa >> (MANTBITS + 2) == 0 {
        mantissa <<= 1;
        exp -= 1;
    }
    if trunc {
        mantissa |= 1;
    }
    while mantissa >> (1 + MANTBITS + 2) != 0 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    // denormalize when the exponent is too small (the -2 is for the rounding bits)
    while mantissa > 1 && exp < MIN_EXP - 2 {
        mantissa = (mantissa >> 1) | (mantissa & 1);
        exp += 1;
    }
    let mut round = mantissa & 3;
    mantissa >>= 2;
    round |= mantissa & 1; // round to even
    exp += 2;
    if round == 3 {
        mantissa += 1;
        if mantissa == 1 << (1 + MANTBITS) {
            mantissa >>= 1;
            exp += 1;
        }
    }
    if mantissa >> MANTBITS == 0 {
        // denormal or zero
        exp = BIAS;
    }
    let sign = if neg { 1u64 << 63 } else { 0 };
    if exp > MAX_EXP {
        return f64::from_bits(sign | (0x7ff << 52));
    }
    let bits = sign | (((exp - BIAS) as u64) << MANTBITS) | (mantissa & ((1 << MANTBITS) - 1));
    f64::from_bits(bits)
}
