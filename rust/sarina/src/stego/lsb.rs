//! Go: `stego/lsb.go` and `diff.go`: an image decoded to samples ([`decode_image`], through [`super::images`]), the
//! least-significant-bit statistics with the pairs-of-values chi-square ([`analyse`]), extraction in every order
//! ([`extract_bits`]), the search for a payload ([`search_lsb`]) and the comparison of two pictures ([`diff`])
//! (testdata/parity/stegolsb.json). JPEG decoding is the next part: [`decode_image`] answers None for a JPEG.

use super::images::{self, Decoded, Kind};
use super::wav::{judge, Found, MAX_PIXELS};
use crate::cryptotools::fmt_f;
use crate::gomath;

/// Go's `Pixels`.
pub struct Pixels {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
    pub indices: Option<Vec<u8>>,
    pub format: &'static str,
    pub alpha: bool,
}

/// Go's `DecodeImage`; None for a JPEG (its decoder is not ported yet).
pub fn decode_image(b: &[u8]) -> Option<Result<Pixels, String>> {
    let (w, h, _) = match images::decode_config(b) {
        Ok(Some(c)) => c,
        Ok(None) => return None,
        Err(e) => return Some(Err(format!("not an image I can decode: {e}"))),
    };
    if w * h > MAX_PIXELS {
        return Some(Err(format!("the image is {w}x{h}, more than {MAX_PIXELS} pixels: too big for me to look at bit by bit")));
    }
    let (img, format) = match images::decode(b) {
        Ok(Decoded::Image(m, f)) => (m, f),
        Ok(Decoded::Jpeg) => return None,
        Err(e) => return Some(Err(format!("cannot decode the image: {e}"))),
    };
    let mut p = Pixels { w: img.w, h: img.h, rgba: Vec::with_capacity(4 * img.w * img.h), indices: None, format, alpha: false };
    if img.kind == Kind::Paletted {
        p.indices = Some(img.pix.clone());
    }
    let mut first = 255u8;
    for y in 0..img.h {
        for x in 0..img.w {
            let c = img.nrgba(x, y);
            if x == 0 && y == 0 {
                first = c[3];
            } else if c[3] != first {
                p.alpha = true;
            }
            p.rgba.extend_from_slice(&c);
        }
    }
    Some(Ok(p))
}

/// Go's `ChannelStats`.
#[derive(Clone, Debug)]
pub struct ChannelStats {
    pub name: &'static str,
    pub ones_rate: f64,
    pub povp: f64,
    pub flat: bool,
}

/// Go's `LSBAnalysis`.
pub struct Analysis {
    pub channels: Vec<ChannelStats>,
    pub first_tenth: Vec<ChannelStats>,
    pub verdict: String,
    pub explanation: &'static str,
}

/// Go's `Analyse`.
pub fn analyse(p: &Pixels) -> Analysis {
    let mut channels = vec![(0usize, "red"), (1, "green"), (2, "blue")];
    if p.alpha {
        channels.push((3, "alpha"));
    }
    let stats = |from: usize, to: usize| {
        let mut out = Vec::new();
        if let Some(ix) = &p.indices {
            out.push(channel_stats("palette index", &ix[from..to], 1, 0));
        }
        for &(ch, name) in &channels {
            out.push(channel_stats(name, &p.rgba[from * 4..to * 4], 4, ch));
        }
        out
    };
    let n = p.w * p.h;
    let all = stats(0, n);
    let tenth = n / 10;
    let first_tenth = if tenth >= 256 { stats(0, tenth) } else { Vec::new() };
    let (mut high, mut mid) = (Vec::new(), Vec::new());
    let mut measured = 0;
    for (part, cs) in [("", &all), (" in the first tenth", &first_tenth)] {
        for s in cs.iter().filter(|s| !s.flat) {
            measured += 1;
            if s.povp > 0.95 {
                high.push(format!("{}{part} (p={})", s.name, fmt_f(s.povp, 2)));
            } else if s.povp > 0.5 {
                mid.push(format!("{}{part} (p={})", s.name, fmt_f(s.povp, 2)));
            }
        }
    }
    let (verdict, explanation) = if !high.is_empty() {
        let mut v = format!("LSB values are equalised in {}", high.join(", "));
        if high.len() < measured {
            v += "; not in the other channels";
        }
        (v, "The pairs-of-values test (p near 1: values 2k and 2k+1 occur equally often) is what sequential LSB replacement leaves, but sensor noise, dithering and synthetic noise leave the same; it is a reason to extract, not a finding.")
    } else if !mid.is_empty() {
        (
            format!("LSB values are partly equalised in {}", mid.join(", ")),
            "A short or partial payload could do this; so can ordinary noise.",
        )
    } else {
        (
            "LSB values are not equalised: ordinary image structure or flat colour".to_owned(),
            "No statistical sign of sequential LSB replacement. A payload spread by a key (steghide, randomised LSB) or in JPEG coefficients does not show here.",
        )
    };
    Analysis { channels: all, first_tenth, verdict, explanation }
}

fn channel_stats(name: &'static str, data: &[u8], stride: usize, offset: usize) -> ChannelStats {
    let mut hist = [0f64; 256];
    let (mut ones, mut n) = (0.0, 0.0);
    let mut i = offset;
    while i < data.len() {
        let v = data[i];
        hist[usize::from(v)] += 1.0;
        n += 1.0;
        if v & 1 == 1 {
            ones += 1.0;
        }
        i += stride;
    }
    let mut st = ChannelStats { name, ones_rate: 0.0, povp: 0.0, flat: false };
    if n == 0.0 {
        return st;
    }
    st.ones_rate = ones / n;
    if hist.iter().filter(|&&h| h > 0.0).count() <= 1 {
        st.flat = true;
        return st;
    }
    let (mut chi, mut df) = (0.0, 0);
    for k in 0..128 {
        let e = (hist[2 * k] + hist[2 * k + 1]) / 2.0;
        if e < 5.0 {
            continue;
        }
        let d = hist[2 * k] - e;
        chi += d * d / e;
        df += 1;
    }
    if df == 0 {
        st.flat = true;
        return st;
    }
    st.povp = 1.0 - chi_square_cdf(chi, f64::from(df));
    st
}

fn chi_square_cdf(x: f64, k: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    lower_gamma(k / 2.0, x / 2.0)
}

fn lower_gamma(a: f64, x: f64) -> f64 {
    if x < a + 1.0 {
        let (mut sum, mut term) = (1.0 / a, 1.0 / a);
        for n in 1..500 {
            term *= x / (a + f64::from(n));
            sum += term;
            if term.abs() < sum.abs() * 1e-14 {
                break;
            }
        }
        return sum * gomath::exp(-x + a * gomath::log(x) - gomath::lgamma(a));
    }
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / 1e-300;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..500 {
        let an = -f64::from(i) * (f64::from(i) - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < 1e-300 {
            d = 1e-300;
        }
        c = b + an / c;
        if c.abs() < 1e-300 {
            c = 1e-300;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-14 {
            break;
        }
    }
    1.0 - gomath::exp(-x + a * gomath::log(x) - gomath::lgamma(a)) * h
}

/// Go's `Describe`.
pub fn describe(a: &Analysis) -> String {
    let line = |s: &ChannelStats| {
        if s.flat {
            format!("{}: flat (one value)", s.name)
        } else {
            format!("{}: LSB ones {}%, pairs-of-values p={}", s.name, fmt_f(100.0 * s.ones_rate, 1), fmt_f(s.povp, 2))
        }
    };
    let mut b = format!("Whole image: {}", a.channels.iter().map(line).collect::<Vec<_>>().join("; "));
    if !a.first_tenth.is_empty() {
        b += &format!("\nFirst tenth: {}", a.first_tenth.iter().map(line).collect::<Vec<_>>().join("; "));
    }
    b + &format!("\nStatistic: {}. {}", a.verdict, a.explanation)
}

/// Go's `Orders`: which samples carry the bits (-1 the palette index), the common ones first.
pub const ORDERS: [(&str, &[i32]); 7] =
    [("rgb", &[0, 1, 2]), ("r", &[0]), ("g", &[1]), ("b", &[2]), ("rgba", &[0, 1, 2, 3]), ("bgr", &[2, 1, 0]), ("a", &[3])];

/// Go's `ExtractBits`.
pub fn extract_bits(p: &Pixels, channels: &[i32], bits: i64, msb_first: bool, max: usize) -> Vec<u8> {
    let bits = if (1..=4).contains(&bits) { bits } else { 1 };
    let mut out = Vec::with_capacity(max.min(1 << 20));
    let (mut cur, mut have) = (0u32, 0u32);
    for i in 0..p.w * p.h {
        for &ch in channels {
            let v = if ch < 0 {
                match &p.indices {
                    Some(ix) => ix[i],
                    None => return out,
                }
            } else {
                p.rgba[i * 4 + ch as usize]
            };
            for k in (0..bits).rev() {
                let bit = u32::from(v >> k) & 1;
                if msb_first {
                    cur = cur << 1 | bit;
                } else {
                    cur |= bit << have;
                }
                have += 1;
                if have == 8 {
                    out.push(cur as u8);
                    cur = 0;
                    have = 0;
                    if out.len() >= max {
                        return out;
                    }
                }
            }
        }
    }
    out
}

/// Go's `SearchLSB`.
pub fn search_lsb(p: &Pixels, max_bytes: usize) -> Vec<Found> {
    let mut orders: Vec<(&str, &[i32])> = Vec::new();
    if p.indices.is_some() {
        orders.push(("palette index", &[-1]));
    }
    orders.extend(ORDERS);
    let mut found = Vec::new();
    for (name, chans) in orders {
        if chans.contains(&3) && !p.alpha {
            continue;
        }
        for msb in [true, false] {
            if let Some(mut f) = judge(&extract_bits(p, chans, 1, msb, max_bytes)) {
                f.order = name.to_owned();
                f.msb_first = msb;
                f.bits = 1;
                found.push(f);
            }
        }
    }
    found
}

/// Go's `Diff`.
pub fn diff(a: &Pixels, b: &Pixels) -> Result<String, String> {
    if a.w != b.w || a.h != b.h {
        return Err(format!(
            "the images differ in size ({}x{} and {}x{}), so they are not the same picture with something hidden in it",
            a.w, a.h, b.w, b.h
        ));
    }
    let n = a.w * a.h;
    let (mut changed, mut lsb_only, mut max_diff) = (0usize, 0usize, 0i32);
    let mut chan = [0usize; 4];
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (a.w as i64, a.h as i64, -1i64, -1i64);
    for i in 0..n {
        let (mut pixel_changed, mut only_lsb) = (false, true);
        for c in 0..4 {
            let d = (i32::from(a.rgba[i * 4 + c]) - i32::from(b.rgba[i * 4 + c])).abs();
            if d > 0 {
                pixel_changed = true;
                chan[c] += 1;
                max_diff = max_diff.max(d);
                if d > 1 {
                    only_lsb = false;
                }
            }
        }
        if pixel_changed {
            changed += 1;
            if only_lsb {
                lsb_only += 1;
            }
            let (x, y) = ((i % a.w) as i64, (i / a.w) as i64);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if changed == 0 {
        return Ok(format!("The two images are identical pixel for pixel ({}x{}).", a.w, a.h));
    }
    let mut out = format!(
        "{changed} of {n} pixels differ ({}%); largest change {max_diff} of 255; changes per channel: red {}, green {}, blue {}, alpha {}; within x {min_x}-{max_x}, y {min_y}-{max_y}.",
        fmt_f(100.0 * changed as f64 / n as f64, 2),
        chan[0],
        chan[1],
        chan[2],
        chan[3]
    );
    out += if lsb_only == changed {
        " Every change is in the lowest bit only: exactly what LSB steganography leaves (invisible to the eye). Extract the LSBs of the changed image (action lsb) to read it."
    } else if lsb_only * 2 > changed {
        " Most changes are in the lowest bit only: LSB steganography, or a lossy re-save."
    } else {
        " The changes are larger than the lowest bit: a re-encode, a resize, or visible editing, not plain LSB hiding."
    };
    Ok(out)
}
