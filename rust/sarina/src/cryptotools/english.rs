//! Go: `cryptotools/english.go`: how much a text looks like English (letter frequencies, common words, printable
//! characters, entropy, index of coincidence).

use crate::gotext;
use std::collections::HashSet;
use std::sync::LazyLock;

const ENGLISH_FREQ: [f64; 26] = [
    0.08167, 0.01492, 0.02782, 0.04253, 0.12702, 0.02228, 0.02015, 0.06094, 0.06966, 0.00153, 0.00772, 0.04025, 0.02406, 0.06749, 0.07507,
    0.01929, 0.00095, 0.05987, 0.06327, 0.09056, 0.02758, 0.00978, 0.02360, 0.00150, 0.01974, 0.00074,
];

const WORDS: &str = "the be to of and a in that have i it for not on with he as you do at this but his by from they we say
her she or an will my one all would there their what so up out if about who get which go me when make can like time no just him know
take people into year your good some could them see other than then now look only come its over think also back after use two how our
work first well way even new want because any these give day most us is are was were been has had did does said says very much more
than many where why here each both own same too under again further once during before above below between through off down little
much such other still should those while must may might shall find found call called made need keep let put ask tell seem try leave
help start show hear play run move live believe hold bring happen write provide sit stand lose pay meet include continue set learn
change lead understand watch follow stop create speak read allow add spend grow open walk win offer remember love consider appear buy
wait serve die send expect build stay fall cut reach kill remain suggest raise pass sell require report decide pull return explain
hope develop carry break receive agree support hit produce eat cover catch draw choose cause point attack secret message key cipher
code flag password hello world test text plain data file name user system computer network security hidden meet midnight tonight
north south east west enemy money gold treasure map letter dear love friend yours truly sincerely never always";

/// Go's `commonWords`.
pub static COMMON_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| WORDS.split_whitespace().collect());

/// The letters a..z of Go's `strings.ToLower(s)` counted (Go: `for _, r := range strings.ToLower(s)`).
fn letter_counts(s: &str) -> ([f64; 26], f64) {
    let mut counts = [0f64; 26];
    let mut n = 0.0;
    for r in gotext::to_lower(s).chars() {
        if r.is_ascii_lowercase() {
            counts[(r as u8 - b'a') as usize] += 1.0;
            n += 1.0;
        }
    }
    (counts, n)
}

/// Go's `ChiSquared`: how far the letter counts are from English (lower is more English; +Inf without letters).
pub fn chi_squared(s: &str) -> f64 {
    let (counts, n) = letter_counts(s);
    if n == 0.0 {
        return f64::INFINITY;
    }
    let mut chi = 0.0;
    for i in 0..26 {
        let expected = ENGLISH_FREQ[i] * n;
        let d = counts[i] - expected;
        chi += d * d / expected;
    }
    chi
}

/// Go's `WordRatio`: the share of words that are common English words.
pub fn word_ratio(s: &str) -> f64 {
    let lower = gotext::to_lower(s);
    let words: Vec<&str> = lower.split(|r: char| !gotext::is_letter(r) && r != '\'').filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return 0.0;
    }
    let hits = words
        .iter()
        .filter(|w| {
            let w = w.trim_matches('\'');
            w.len() >= 2 && COMMON_WORDS.contains(w)
        })
        .count();
    hits as f64 / words.len() as f64
}

/// Go's `Printable`: the share of characters that are printable (or newline, return, tab).
pub fn printable(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let (mut ok, mut n) = (0, 0);
    for r in s.chars() {
        n += 1;
        if r == '\n' || r == '\r' || r == '\t' || (gotext::is_print(r) && r != '\u{FFFD}') {
            ok += 1;
        }
    }
    ok as f64 / n as f64
}

/// Go's `LooksLikeText`.
pub fn looks_like_text(s: &str) -> bool {
    if s.len() < 2 || printable(s) < 0.95 {
        return false;
    }
    if word_ratio(s) >= 0.25 {
        return true;
    }
    let (mut letters, mut spaces, mut digits) = (0usize, 0usize, 0usize);
    for r in s.chars() {
        if gotext::is_letter(r) {
            letters += 1;
        } else if r == ' ' || r == '\n' {
            spaces += 1;
        } else if gotext::is_digit(r) {
            digits += 1;
        }
    }
    let n = s.chars().count();
    let t = gotext::trim_space(s);
    if t.starts_with('{') || t.starts_with('[') || t.starts_with('<') {
        return true;
    }
    letters > n / 2 && spaces > 0 && (digits as f64) < 0.3 * n as f64
}

/// Go's `Entropy`: Shannon entropy in bits per byte.
pub fn entropy(b: &[u8]) -> f64 {
    if b.is_empty() {
        return 0.0;
    }
    let mut counts = [0f64; 256];
    for &c in b {
        counts[c as usize] += 1.0;
    }
    let n = b.len() as f64;
    let mut h = 0.0;
    for c in counts {
        if c > 0.0 {
            let p = c / n;
            h -= p * crate::gomath::log2(p);
        }
    }
    h
}

/// Go's `IndexOfCoincidence` (0 for fewer than two letters).
pub fn index_of_coincidence(s: &str) -> f64 {
    let (counts, n) = letter_counts(s);
    if n < 2.0 {
        return 0.0;
    }
    let sum: f64 = counts.iter().fold(0.0, |a, c| a + c * (c - 1.0));
    sum / (n * (n - 1.0))
}
