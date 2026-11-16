//! Go: `cryptotools/classic.go`, `howitfell.go` and `Classic` (cryptotools.go): the classical ciphers and their crackers
//! as the crypto_classic tool answers, with the work shown. Step for step Go's, Go's `math/rand` and `sort.SliceStable`
//! included, so every guess and every score is the same (testdata/parity/classic.json).

use super::english::{chi_squared, index_of_coincidence, word_ratio, COMMON_WORDS};
use super::{fmt_f, to_upper, truncate_text};
use crate::gosort::slice_stable;
use crate::gotext;

/// The letters a..z of Go's `strings.ToLower(s)`, in order (Go's `lowerLetters`).
pub fn lower_letters(s: &str) -> String {
    gotext::to_lower(s).chars().filter(char::is_ascii_lowercase).collect()
}

fn shift_char(r: char, shift: i64) -> char {
    match r {
        'a'..='z' => (b'a' + ((r as u8 - b'a') as i64 + shift).rem_euclid(26) as u8) as char,
        'A'..='Z' => (b'A' + ((r as u8 - b'A') as i64 + shift).rem_euclid(26) as u8) as char,
        _ => r,
    }
}

/// Go's `Caesar`.
pub fn caesar(s: &str, shift: i64) -> String {
    let shift = shift.rem_euclid(26);
    s.chars().map(|r| shift_char(r, shift)).collect()
}

/// Go's `Atbash`.
pub fn atbash(s: &str) -> String {
    s.chars()
        .map(|r| match r {
            'a'..='z' => (b'z' - (r as u8 - b'a')) as char,
            'A'..='Z' => (b'Z' - (r as u8 - b'A')) as char,
            _ => r,
        })
        .collect()
}

/// Go's `keyShifts`: the key's letters (after Go's ToLower) as shifts.
fn key_shifts(key: &str) -> Vec<i64> {
    gotext::to_lower(key).chars().filter(char::is_ascii_lowercase).map(|r| (r as u8 - b'a') as i64).collect()
}

/// Go's `Vigenere`.
pub fn vigenere(s: &str, key: &str, decode: bool) -> Result<String, String> {
    let k = key_shifts(key);
    if k.is_empty() {
        return Err("the Vigenère key needs at least one letter".into());
    }
    let mut i = 0;
    Ok(s.chars()
        .map(|r| {
            if !r.is_ascii_alphabetic() {
                return r;
            }
            let mut shift = k[i % k.len()];
            if decode {
                shift = 26 - shift;
            }
            i += 1;
            shift_char(r, shift)
        })
        .collect())
}

fn rail_pattern(n: usize, rails: i64) -> Vec<i64> {
    let mut rows = vec![0; n];
    let (mut row, mut dir) = (0i64, 1i64);
    for slot in rows.iter_mut() {
        *slot = row;
        if rails > 1 {
            if row == 0 {
                dir = 1;
            } else if row == rails - 1 {
                dir = -1;
            }
            row += dir;
        }
    }
    rows
}

/// Go's `RailFence`.
pub fn rail_fence(s: &str, rails: i64, decode: bool) -> Result<String, String> {
    if !(2..=100).contains(&rails) {
        return Err("rails must be between 2 and 100".into());
    }
    let rs: Vec<char> = s.chars().collect();
    let n = rs.len();
    let rows = rail_pattern(n, rails);
    if !decode {
        let mut out = String::with_capacity(s.len());
        for r in 0..rails {
            for i in 0..n {
                if rows[i] == r {
                    out.push(rs[i]);
                }
            }
        }
        return Ok(out);
    }
    let mut out = vec!['\0'; n];
    let mut pos = 0;
    for r in 0..rails {
        for i in 0..n {
            if rows[i] == r {
                out[i] = rs[pos];
                pos += 1;
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// Go's `applyTable`: each letter through a 26-letter table (capitals stay capitals).
fn apply_table(s: &str, table: &[u8; 26]) -> String {
    s.chars()
        .map(|r| match r {
            'a'..='z' => table[(r as u8 - b'a') as usize] as char,
            'A'..='Z' => (table[(r as u8 - b'A') as usize] as char).to_ascii_uppercase(),
            _ => r,
        })
        .collect()
}

/// Go's `Substitution`: key is the cipher alphabet for plain a..z.
pub fn substitution(s: &str, key: &str, decode: bool) -> Result<String, String> {
    let key = gotext::to_lower(&key.chars().filter(|&r| gotext::is_letter(r)).collect::<String>());
    if key.len() != 26 {
        return Err(format!("a substitution key is the whole alphabet in cipher order, 26 letters (got {})", key.len()));
    }
    let mut seen = [false; 26];
    for r in key.chars() {
        if !r.is_ascii_lowercase() || seen[(r as u8 - b'a') as usize] {
            return Err("a substitution key uses each of a..z exactly once".into());
        }
        seen[(r as u8 - b'a') as usize] = true;
    }
    let mut table = [0u8; 26];
    for (i, r) in key.bytes().enumerate() {
        if decode {
            table[(r - b'a') as usize] = b'a' + i as u8;
        } else {
            table[i] = r;
        }
    }
    Ok(apply_table(s, &table))
}

/// One cracker's guess (Go's `Guess`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Guess {
    pub key: String,
    pub text: String,
    pub score: f64,
    pub note: String,
}

/// Go's `CrackCaesar`: the three best of the 26 shifts.
pub fn crack_caesar(s: &str) -> Vec<Guess> {
    let gs = (0..26)
        .map(|shift| {
            let t = caesar(s, 26 - shift);
            Guess {
                key: format!("shift {shift}"),
                score: chi_squared(&t),
                note: format!("chi-squared {}, {}% common words", fmt_f(chi_squared(&t), 0), fmt_f(100.0 * word_ratio(&t), 0)),
                text: t,
            }
        })
        .collect();
    rank_shift_guesses(gs)
}

fn rank_shift_guesses(mut gs: Vec<Guess>) -> Vec<Guess> {
    let readable = |g: &Guess| word_ratio(&g.text) >= 0.25;
    slice_stable(&mut gs, |a, b| {
        if readable(a) != readable(b) {
            return readable(a);
        }
        if readable(a) && word_ratio(&a.text) != word_ratio(&b.text) {
            return word_ratio(&a.text) > word_ratio(&b.text);
        }
        a.score < b.score
    });
    gs.truncate(3);
    gs
}

/// Go's `vigenereKeyLengths`: the four likeliest key lengths (indexes of coincidence within 0.004 count as equal).
pub fn vigenere_key_lengths(letters: &[u8], max_len: usize) -> Vec<usize> {
    let mut ks: Vec<(usize, f64)> = Vec::new();
    let mut n = 1;
    while n <= max_len && n * 4 <= letters.len() {
        ks.push((n, column_ic(letters, n)));
        n += 1;
    }
    slice_stable(&mut ks, |a, b| if (a.1 - b.1).abs() < 0.004 { a.0 < b.0 } else { a.1 > b.1 });
    ks.iter().take(4).map(|k| k.0).collect()
}

fn column(letters: &[u8], c: usize, n: usize) -> String {
    letters.iter().skip(c).step_by(n).map(|&b| b as char).collect()
}

/// Go's `columnIC`: the mean index of coincidence of every n-th letter.
pub fn column_ic(letters: &[u8], n: usize) -> f64 {
    let mut sum = 0.0;
    for c in 0..n {
        sum += index_of_coincidence(&column(letters, c, n));
    }
    sum / n as f64
}

/// Go's `CrackVigenere`: the key per likely length, each column solved by chi-squared.
pub fn crack_vigenere(s: &str, max_key_len: usize) -> Vec<Guess> {
    let max_key_len = if max_key_len == 0 { 16 } else { max_key_len };
    let ls = lower_letters(s);
    if ls.len() < 12 {
        return Vec::new();
    }
    let lb = ls.as_bytes();
    let mut gs = Vec::new();
    for n in vigenere_key_lengths(lb, max_key_len) {
        let mut key = String::new();
        for c in 0..n {
            let col = column(lb, c, n);
            let (mut best, mut best_chi) = (0u8, f64::INFINITY);
            for shift in 0..26u8 {
                let chi = chi_squared(&caesar(&col, 26 - shift as i64));
                if chi < best_chi {
                    best = shift;
                    best_chi = chi;
                }
            }
            key.push((b'a' + best) as char);
        }
        let t = vigenere(s, &key, true).unwrap_or_default();
        gs.push(Guess {
            note: format!("key length {n}, chi-squared {}, {}% common words", fmt_f(chi_squared(&t), 0), fmt_f(100.0 * word_ratio(&t), 0)),
            score: chi_squared(&t),
            key,
            text: t,
        });
    }
    rank_keyed_guesses(&mut gs);
    gs
}

/// Go's `rankKeyedGuesses`: readable ones (40 % common words) first.
pub fn rank_keyed_guesses(gs: &mut [Guess]) {
    slice_stable(gs, |a, b| {
        let (ri, rj) = (word_ratio(&a.text) >= 0.4, word_ratio(&b.text) >= 0.4);
        if ri != rj {
            return ri;
        }
        if ri {
            return word_ratio(&a.text) > word_ratio(&b.text);
        }
        a.score < b.score
    });
}

/// Go's `CrackRailFence`.
pub fn crack_rail_fence(s: &str, max_rails: i64) -> Vec<Guess> {
    let max_rails = if max_rails <= 0 { 10 } else { max_rails };
    let mut gs = Vec::new();
    for rails in 2..=max_rails {
        let Ok(t) = rail_fence(s, rails, true) else { continue };
        gs.push(Guess {
            key: format!("{rails} rails"),
            score: word_ratio(&t),
            note: format!("{}% common words", fmt_f(100.0 * word_ratio(&t), 0)),
            text: t,
        });
    }
    slice_stable(&mut gs, |a, b| a.score > b.score);
    gs.truncate(3);
    gs
}

const AFFINE_COPRIMES: [i64; 12] = [1, 3, 5, 7, 9, 11, 15, 17, 19, 21, 23, 25];

fn inverse_mod26(a: i64) -> i64 {
    (1..26).find(|x| a * x % 26 == 1).unwrap_or(0)
}

/// Go's `Affine`.
pub fn affine(s: &str, a: i64, b: i64, decode: bool) -> Result<String, String> {
    let (a, b) = (a.rem_euclid(26), b.rem_euclid(26));
    let inv = inverse_mod26(a);
    if inv == 0 {
        return Err(format!("affine needs a coprime with 26 (one of 1 3 5 7 9 11 15 17 19 21 23 25), got {a}"));
    }
    let mut table = [0u8; 26];
    for x in 0..26i64 {
        table[x as usize] = b'a' + if decode { inv * (x - b + 26) % 26 } else { (a * x + b) % 26 } as u8;
    }
    Ok(apply_table(s, &table))
}

/// Go's `numberRE.FindAllString(key, -1)` for `-?\d+` (ASCII digits).
fn numbers(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let j = if b[i] == b'-' { i + 1 } else { i };
        if j < b.len() && b[j].is_ascii_digit() {
            let mut k = j;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            out.push(&s[start..k]);
            i = k;
        } else {
            i += 1;
        }
    }
    out
}

/// Go's `strconv.Atoi` of `-?\d+` with its error dropped: out of range gives the nearest int64.
fn atoi_saturating(n: &str) -> i64 {
    n.parse::<i64>().unwrap_or(if n.starts_with('-') { i64::MIN } else { i64::MAX })
}

fn parse_affine_key(key: &str) -> Result<(i64, i64), String> {
    let ns = numbers(key);
    if ns.len() != 2 {
        return Err("affine needs two numbers as key, a and b (for example 5,8)".into());
    }
    Ok((atoi_saturating(ns[0]), atoi_saturating(ns[1])))
}

/// Go's `CrackAffine`: the three best of the 312 keys.
pub fn crack_affine(s: &str) -> Vec<Guess> {
    let mut gs = Vec::new();
    for a in AFFINE_COPRIMES {
        for b in 0..26 {
            let t = affine(s, a, b, true).unwrap_or_default();
            let chi = chi_squared(&t);
            gs.push(Guess {
                key: format!("a={a}, b={b}"),
                score: chi,
                note: format!("chi-squared {}, {}% common words", fmt_f(chi, 0), fmt_f(100.0 * word_ratio(&t), 0)),
                text: t,
            });
        }
    }
    rank_shift_guesses(gs)
}

/// Go's `Beaufort` (its own inverse).
pub fn beaufort(s: &str, key: &str) -> Result<String, String> {
    let k = key_shifts(key);
    if k.is_empty() {
        return Err("the Beaufort key needs at least one letter".into());
    }
    let mut i = 0;
    Ok(s.chars()
        .map(|r| {
            let base = match r {
                'a'..='z' => b'a',
                'A'..='Z' => b'A',
                _ => return r,
            };
            let out = (base + (k[i % k.len()] - (r as u8 - base) as i64 + 26).rem_euclid(26) as u8) as char;
            i += 1;
            out
        })
        .collect())
}

/// Go's `CrackBeaufort`.
pub fn crack_beaufort(s: &str, max_key_len: usize) -> Vec<Guess> {
    let max_key_len = if max_key_len == 0 { 16 } else { max_key_len };
    let ls = lower_letters(s);
    if ls.len() < 12 {
        return Vec::new();
    }
    let lb = ls.as_bytes();
    let mut gs = Vec::new();
    for n in vigenere_key_lengths(lb, max_key_len) {
        let mut key = String::new();
        for c in 0..n {
            let (mut best, mut best_chi) = (0u8, f64::INFINITY);
            for k in 0..26i64 {
                let col: String =
                    lb.iter().skip(c).step_by(n).map(|&x| (b'a' + (k - (x - b'a') as i64 + 26).rem_euclid(26) as u8) as char).collect();
                let chi = chi_squared(&col);
                if chi < best_chi {
                    best = k as u8;
                    best_chi = chi;
                }
            }
            key.push((b'a' + best) as char);
        }
        let t = beaufort(s, &key).unwrap_or_default();
        gs.push(Guess {
            note: format!("key length {n}, chi-squared {}, {}% common words", fmt_f(chi_squared(&t), 0), fmt_f(100.0 * word_ratio(&t), 0)),
            score: chi_squared(&t),
            key,
            text: t,
        });
    }
    rank_keyed_guesses(&mut gs);
    gs
}

/// Go's `Autokey`: the primer, then the plaintext itself, is the key.
pub fn autokey(s: &str, primer: &str, decode: bool) -> Result<String, String> {
    let mut stream = key_shifts(primer);
    if stream.is_empty() {
        return Err("the autokey primer needs at least one letter".into());
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    for r in s.chars() {
        let base = match r {
            'a'..='z' => b'a',
            'A'..='Z' => b'A',
            _ => {
                out.push(r);
                continue;
            }
        };
        let x = (r as u8 - base) as i64;
        let mut p = x;
        if decode {
            p = (x - stream[i] + 26) % 26;
            out.push((base + p as u8) as char);
        } else {
            out.push((base + ((x + stream[i]) % 26) as u8) as char);
        }
        stream.push(p);
        i += 1;
    }
    Ok(out)
}

/// Go's `crackAutokey`: the guesses and the chi-squared per primer length.
pub fn crack_autokey(s: &str, max_primer: usize) -> (Vec<Guess>, String) {
    let max_primer = if max_primer == 0 { 12 } else { max_primer };
    let ls = lower_letters(s);
    if ls.len() < 12 {
        return (Vec::new(), String::new());
    }
    let lb = ls.as_bytes();
    let mut gs = Vec::new();
    let mut per = Vec::new();
    let mut n = 1;
    while n <= max_primer && n * 3 <= lb.len() {
        let mut key = vec![b'a'; n];
        for (j, slot) in key.iter_mut().enumerate() {
            let (mut best, mut best_chi) = (0u8, f64::INFINITY);
            for k in 0..26i64 {
                let mut chain = String::new();
                let mut prev = k;
                for &x in lb.iter().skip(j).step_by(n) {
                    let p = ((x - b'a') as i64 - prev + 26) % 26;
                    chain.push((b'a' + p as u8) as char);
                    prev = p;
                }
                let chi = chi_squared(&chain);
                if chi < best_chi {
                    best = k as u8;
                    best_chi = chi;
                }
            }
            *slot = b'a' + best;
        }
        let ks = |key: &[u8]| String::from_utf8_lossy(key).into_owned();
        for j in 0..n {
            let t = autokey(s, &ks(&key), true).unwrap_or_default();
            let (mut best_letter, mut best_fit) = (key[j], substitution_fitness(&t));
            for k in b'a'..=b'z' {
                key[j] = k;
                let t = autokey(s, &ks(&key), true).unwrap_or_default();
                let f = substitution_fitness(&t);
                if f > best_fit {
                    best_letter = k;
                    best_fit = f;
                }
            }
            key[j] = best_letter;
        }
        let key = ks(&key);
        let t = autokey(s, &key, true).unwrap_or_default();
        let chi = chi_squared(&t);
        gs.push(Guess {
            note: format!("primer length {n}, chi-squared {}, {}% common words", fmt_f(chi, 0), fmt_f(100.0 * word_ratio(&t), 0)),
            key,
            text: t,
            score: chi,
        });
        per.push(format!("{n} {}", fmt_f(chi, 0)));
        n += 1;
    }
    rank_keyed_guesses(&mut gs);
    gs.truncate(3);
    (gs, per.join(", "))
}

/// Go's `columnOrder`: the column read at each rank, from a word or from the ranks as numbers.
fn column_order(key: &str) -> Result<Vec<usize>, String> {
    let key = gotext::trim_space(key);
    let mut ranks: Vec<i64> = Vec::new();
    let mut ns = numbers(key);
    if !ns.is_empty() && key.trim_matches(|c| "0123456789 ,-".contains(c)).is_empty() {
        let split: Vec<String>;
        if ns.len() == 1 && ns[0].len() > 1 {
            split = ns[0].chars().map(String::from).collect();
            ns = split.iter().map(String::as_str).collect();
        }
        for n in &ns {
            // Go's Atoi of one character ("-" alone is an error: 0)
            ranks.push(n.parse::<i64>().unwrap_or(if n.len() > 1 { atoi_saturating(n) } else { 0 }));
        }
    } else {
        let k = key_shifts(key);
        let mut idx: Vec<usize> = (0..k.len()).collect();
        slice_stable(&mut idx, |&a, &b| k[a] < k[b]);
        ranks = vec![0; k.len()];
        for (r, &c) in idx.iter().enumerate() {
            ranks[c] = r as i64 + 1;
        }
    }
    if ranks.len() < 2 || ranks.len() > 40 {
        return Err("a columnar key is a word or the column order as numbers (3 1 4 2), 2 to 40 columns".into());
    }
    let w = ranks.len() as i64;
    let mut order = vec![0usize; ranks.len()];
    let mut seen = vec![false; ranks.len()];
    for (c, &r) in ranks.iter().enumerate() {
        if r < 1 || r > w || seen[(r - 1) as usize] {
            return Err(format!("a numeric columnar key uses each of 1..{} once", ranks.len()));
        }
        seen[(r - 1) as usize] = true;
        order[(r - 1) as usize] = c;
    }
    Ok(order)
}

fn column_ranks(order: &[usize]) -> String {
    let mut ranks = vec![String::new(); order.len()];
    for (r, &c) in order.iter().enumerate() {
        ranks[c] = (r + 1).to_string();
    }
    ranks.join(" ")
}

/// Go's `Columnar`.
pub fn columnar_cipher(s: &str, key: &str, decode: bool) -> Result<String, String> {
    let order = column_order(key)?;
    Ok(columnar(&s.chars().collect::<Vec<_>>(), &order, decode))
}

fn columnar(rs: &[char], order: &[usize], decode: bool) -> String {
    let (w, n) = (order.len(), rs.len());
    let rows = n.div_ceil(w);
    let full = n % w;
    let mut out = vec!['\0'; n];
    let mut pos = 0;
    for &c in order {
        let mut h = rows;
        if full != 0 && c >= full {
            h -= 1;
        }
        for r in 0..h {
            if decode {
                out[r * w + c] = rs[pos];
            } else {
                out[pos] = rs[r * w + c];
            }
            pos += 1;
        }
    }
    out.into_iter().collect()
}

/// Go's `crackColumnar`: every column order of each width up to 8, the guesses and the best score per width.
pub fn crack_columnar(s: &str, max_width: usize) -> (Vec<Guess>, String) {
    let max_width = if max_width == 0 || max_width > 8 { 8 } else { max_width };
    let rs: Vec<char> = s.chars().collect();
    let spaced = s.contains([' ', '\n']);
    let fit = |t: &str| {
        let mut f = ngram_rate(t);
        if spaced {
            f += word_ratio(t);
        }
        f
    };
    const KEEP: usize = 30;
    let mut gs = Vec::new();
    let mut per = Vec::new();
    let mut w = 2;
    while w <= max_width && w * 2 <= rs.len() {
        let mut perm: Vec<usize> = (0..w).collect();
        let mut top: Vec<(f64, Vec<usize>)> = Vec::new();
        permutations(&mut perm, &mut |p| {
            let f = fit(&columnar(&rs, p, true));
            if top.len() == KEEP && f <= top[KEEP - 1].0 {
                return;
            }
            let i = top.iter().position(|t| t.0 < f).unwrap_or(top.len());
            top.insert(i, (f, p.to_vec()));
            top.truncate(KEEP);
        });
        let mut best: (f64, Vec<usize>) = (f64::NEG_INFINITY, Vec::new());
        for c in &top {
            let f = c.0 + word_cover(&lower_letters(&columnar(&rs, &c.1, true)));
            if f > best.0 {
                best = (f, c.1.clone());
            }
        }
        let t = columnar(&rs, &best.1, true);
        let mut note = format!(
            "key length {w}, {} common letter pairs per letter, {}% of the letters in common words",
            fmt_f(ngram_rate(&t), 2),
            fmt_f(100.0 * word_cover(&lower_letters(&t)), 0)
        );
        if spaced {
            note += &format!(", {}% common words", fmt_f(100.0 * word_ratio(&t), 0));
        }
        gs.push(Guess { key: column_ranks(&best.1), text: t, score: best.0, note });
        per.push(format!("{w} {}", fmt_f(best.0, 2)));
        w += 1;
    }
    slice_stable(&mut gs, |a, b| a.score > b.score);
    gs.truncate(3);
    (gs, per.join(", "))
}

/// Go's `permutations` (Heap's algorithm, the same order).
fn permutations(p: &mut [usize], f: &mut dyn FnMut(&[usize])) {
    let mut c = vec![0usize; p.len()];
    f(p);
    let mut i = 0;
    while i < p.len() {
        if c[i] < i {
            if i % 2 == 0 {
                p.swap(0, i);
            } else {
                p.swap(c[i], i);
            }
            f(p);
            c[i] += 1;
            i = 0;
        } else {
            c[i] = 0;
            i += 1;
        }
    }
}

const COMMON_BIGRAMS: &str = "th he in er an re on at en nd ti es or te of ed is it al ar st to nt ng se ha as ou io le ve co me de hi ri ro ic ne ea ra ce li ch ll be ma si om ur";
const COMMON_TRIGRAMS: &str =
    "the and ing ent ion her for tha nth int ere tio ter est ers ati hat ate all eth hes ver his oft ith fth sth oth res ont";

/// Go's `substitutionFitness`.
pub fn substitution_fitness(t: &str) -> f64 {
    let t = gotext::to_lower(t);
    let mut score = -chi_squared(&t) / 10.0;
    for bg in COMMON_BIGRAMS.split(' ') {
        score += t.matches(bg).count() as f64;
    }
    for tg in COMMON_TRIGRAMS.split(' ') {
        score += 3.0 * t.matches(tg).count() as f64;
    }
    score += 120.0 * word_ratio(&t) * gotext::fields(&t).len() as f64;
    score
}

fn ngram_tables() -> &'static ([bool; 676], Vec<bool>) {
    static T: std::sync::OnceLock<([bool; 676], Vec<bool>)> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut pair = [false; 676];
        let mut triple = vec![false; 26 * 26 * 26];
        for bg in COMMON_BIGRAMS.split(' ').map(str::as_bytes) {
            pair[(bg[0] - b'a') as usize * 26 + (bg[1] - b'a') as usize] = true;
        }
        for tg in COMMON_TRIGRAMS.split(' ').map(str::as_bytes) {
            triple[((tg[0] - b'a') as usize * 26 + (tg[1] - b'a') as usize) * 26 + (tg[2] - b'a') as usize] = true;
        }
        (pair, triple)
    })
}

/// Go's `ngramRate`: common letter pairs (and triples, counted twice) per letter.
pub fn ngram_rate(s: &str) -> f64 {
    let (pair, triple) = ngram_tables();
    let (mut n, mut hits, mut p1, mut p2) = (0usize, 0usize, -1i64, -1i64);
    for r in s.chars() {
        let r = r.to_ascii_lowercase();
        if !r.is_ascii_lowercase() {
            continue;
        }
        let x = (r as u8 - b'a') as i64;
        n += 1;
        if p1 >= 0 && pair[(p1 * 26 + x) as usize] {
            hits += 1;
        }
        if p2 >= 0 && triple[((p2 * 26 + p1) * 26 + x) as usize] {
            hits += 2;
        }
        p2 = p1;
        p1 = x;
    }
    if n == 0 {
        return 0.0;
    }
    hits as f64 / n as f64
}

/// Go's `wordCover`: the share of the letters that common words (2 to 12 letters) can cover.
pub fn word_cover(ls: &str) -> f64 {
    if ls.is_empty() {
        return 0.0;
    }
    let b = ls.as_bytes();
    let mut best = vec![0usize; b.len() + 1];
    for i in 1..=b.len() {
        best[i] = best[i - 1];
        for l in 2..=12.min(i) {
            if let Ok(w) = std::str::from_utf8(&b[i - l..i]) {
                if COMMON_WORDS.contains(w) && best[i - l] + l > best[i] {
                    best[i] = best[i - l] + l;
                }
            }
        }
    }
    best[b.len()] as f64 / b.len() as f64
}

/// Go's `readsAsEnglish`.
pub fn reads_as_english(s: &str) -> bool {
    if gotext::trim_space(s).contains([' ', '\n']) {
        return word_ratio(s) >= 0.2;
    }
    ngram_rate(s) >= 0.7 || word_cover(&lower_letters(s)) >= 0.5
}

/// Go's `CrackSubstitution`: hill-climbing from the frequency order, restarts shuffled by Go's math/rand seeded with 1.
pub fn crack_substitution(s: &str, restarts: usize) -> Guess {
    let restarts = if restarts == 0 { 10 } else { restarts };
    let letters = s.chars().filter(|&r| gotext::is_letter(r)).count();
    let mut rng = crate::gorand::Rand::new(1);
    let mut best = Guess { score: f64::NEG_INFINITY, ..Guess::default() };
    let lower = gotext::to_lower(s);
    for rs in 0..restarts {
        let mut counts = [0i64; 26];
        for r in lower.chars() {
            if r.is_ascii_lowercase() {
                counts[(r as u8 - b'a') as usize] += 1;
            }
        }
        let mut order: Vec<usize> = (0..26).collect();
        slice_stable(&mut order, |&a, &b| counts[a] > counts[b]);
        let by_freq = b"etaoinshrdlcumwfgypbvkjxqz";
        let mut table = [0u8; 26];
        for (rank, &c) in order.iter().enumerate() {
            table[c] = by_freq[rank];
        }
        if rs > 0 {
            for _ in 0..10 {
                let (a, b) = (rng.intn(26) as usize, rng.intn(26) as usize);
                table.swap(a, b);
            }
        }
        let mut cur = substitution_fitness(&apply_table(s, &table));
        for _ in 0..4000 {
            let (a, b) = (rng.intn(26) as usize, rng.intn(26) as usize);
            if a == b {
                continue;
            }
            table.swap(a, b);
            let f = substitution_fitness(&apply_table(s, &table));
            if f >= cur {
                cur = f;
            } else {
                table.swap(a, b);
            }
        }
        if cur > best.score {
            let mut key = [b'a'; 26];
            for (c, &p) in table.iter().enumerate() {
                key[(p - b'a') as usize] = b'a' + c as u8;
            }
            best = Guess { key: String::from_utf8_lossy(&key).into_owned(), text: apply_table(s, &table), score: cur, note: String::new() };
        }
    }
    best.note = format!("{}% common words", fmt_f(100.0 * word_ratio(&best.text), 0));
    if letters < 150 {
        best.note += "; the text is short for this (under 150 letters), so the key is a guess: check it against the words that came out";
    }
    best
}

// ---- how it fell ------------------------------------------------------------------------------------------------------

/// Go's `letterOrder`: the ten most common letters with their counts.
pub fn letter_order(s: &str) -> String {
    let mut counts = [0i64; 26];
    for r in gotext::to_lower(s).chars() {
        if r.is_ascii_lowercase() {
            counts[(r as u8 - b'a') as usize] += 1;
        }
    }
    let mut order: Vec<usize> = (0..26).collect();
    for i in 1..26 {
        let mut j = i;
        while j > 0 && counts[order[j]] > counts[order[j - 1]] {
            order.swap(j, j - 1);
            j -= 1;
        }
    }
    let parts: Vec<String> =
        order[..10].iter().filter(|&&i| counts[i] > 0).map(|&i| format!("{} {}", (b'a' + i as u8) as char, counts[i])).collect();
    parts.join(", ") + " (English: e t a o i n s h r d)"
}

/// Go's `icMeaning`.
pub fn ic_meaning(ic: f64) -> &'static str {
    if ic >= 0.058 {
        "English-like, so one alphabet is used (a shift, affine or substitution) or the letters were only moved (a transposition)"
    } else if ic < 0.05 {
        "flat, so the letters were spread over several alphabets (a key that changes from letter to letter)"
    } else {
        "between English and random"
    }
}

fn kasiski(ls: &[u8], max_len: i64) -> String {
    let mut pos: std::collections::BTreeMap<&[u8], Vec<i64>> = std::collections::BTreeMap::new();
    for i in 0..ls.len().saturating_sub(2) {
        pos.entry(&ls[i..i + 3]).or_default().push(i as i64);
    }
    let mut reps: Vec<(&[u8], Vec<i64>)> = Vec::new();
    let mut all = Vec::new();
    for (tri, ps) in pos {
        if ps.len() < 2 {
            continue;
        }
        let dists: Vec<i64> = ps.windows(2).map(|w| w[1] - w[0]).collect();
        all.extend(dists.iter().copied());
        reps.push((tri, dists));
    }
    if reps.is_empty() {
        return "Kasiski: no trigram repeats, so there are no distances to factor (the text is short, or the key is long or does not repeat).".into();
    }
    reps.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let ex: Vec<String> = reps
        .iter()
        .take(4)
        .map(|(tri, ds)| {
            format!(
                "{} at distances {}",
                String::from_utf8_lossy(tri).to_ascii_uppercase(),
                ds.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    let mut fs: Vec<(i64, usize)> = Vec::new();
    for f in 2..=max_len {
        let n = all.iter().filter(|&&d| d % f == 0).count();
        if n > 0 {
            fs.push((f, n));
        }
    }
    slice_stable(&mut fs, |a, b| a.1 > b.1);
    let top: Vec<String> = fs.iter().take(3).map(|(f, n)| format!("{f} ({n} of {})", all.len())).collect();
    let mut out = format!("Kasiski: {} repeated trigram(s), e.g. {}.", reps.len(), ex.join("; "));
    if !top.is_empty() {
        out += &format!(" Factors the distances share most: {}.", top.join(", "));
    }
    out
}

pub(crate) fn how_it_fell(text: &str, cipher: &str, per_length: &str) -> String {
    let ls = lower_letters(text);
    if ls.is_empty() {
        return String::new();
    }
    let ic = index_of_coincidence(&ls);
    let mut b = String::from("How it fell (computed from the ciphertext):\n");
    b += &format!("- Letter frequencies, most common first: {}.\n", letter_order(&ls));
    b += &format!("- Index of coincidence {} (English 0.066, random letters 0.038): {}.\n", fmt_f(ic, 3), ic_meaning(ic));
    match cipher {
        "vigenere" | "beaufort" => {
            b += &format!("- {}\n", kasiski(ls.as_bytes(), 16));
            let mut ics = Vec::new();
            let mut n = 1;
            while n <= 12 && n * 4 <= ls.len() {
                ics.push(format!("{n} {}", fmt_f(column_ic(ls.as_bytes(), n), 3)));
                n += 1;
            }
            b += &format!(
                "- Index of coincidence per key length (every n-th letter; English-like at the key length and its multiples): {}.\n",
                ics.join(", ")
            );
        }
        "autokey" => {
            b += "- The key does not repeat (the plaintext continues it), so Kasiski and the index of coincidence per key length do not apply. With a primer of n letters every n-th letter forms a chain that one primer letter decides; each chain was solved by chi-squared over the 26 letters.\n";
            if !per_length.is_empty() {
                b += &format!("- Chi-squared of the decoding per primer length (lower is more like English): {per_length}.\n");
            }
        }
        "columnar" | "railfence" => {
            b += &format!("- Chi-squared of the ciphertext's own letters against English: {}. A transposition only moves letters, so their counts stay English.\n", fmt_f(chi_squared(&ls), 0));
            if !per_length.is_empty() {
                b += &format!("- Every column order of each key length was tried and scored by common letter pairs per letter plus the share of letters in common words (and of common words, when there are spaces); the best score per key length: {per_length}.\n");
            }
        }
        _ => {}
    }
    b
}

// ---- the tool's answer --------------------------------------------------------------------------------------------------

fn dir(decode: bool) -> &'static str {
    if decode {
        "decoded"
    } else {
        "encoded"
    }
}

/// Go's `Classic`: crypto_classic's answer (encode, decode with the key, or crack with the work shown).
pub fn classic(text: &str, cipher: &str, key: &str, decode: bool, crack: bool) -> Result<String, String> {
    if gotext::trim_space(text).is_empty() {
        return Err("no text".into());
    }
    let guesses = |gs: &[Guess], what: &str| -> String {
        if gs.is_empty() {
            return format!("Could not crack it: the text is too short, or it is not a {what}.");
        }
        let mut sb = format!("Best {what} guesses, best first:\n");
        for (i, g) in gs.iter().enumerate() {
            sb += &format!("{}. key {} ({}):\n   {}\n", i + 1, g.key, g.note, truncate_text(&g.text, 600).replace('\n', "\n   "));
        }
        if !reads_as_english(&gs[0].text) {
            sb += "None of these reads as English (few common words): the cipher is probably something else, or the language is not English.\n";
        }
        sb
    };
    let work = |per: &str| how_it_fell(text, cipher, per);
    let shift_key = |what: &str| gotext::sscan_int(key).ok_or_else(|| format!("{what} needs a numeric key"));
    let out = match cipher {
        "caesar" => {
            if crack {
                return Ok(guesses(&crack_caesar(text), "Caesar") + &work(""));
            }
            let mut n = shift_key("caesar").map_err(|e| e + " (the shift)")?;
            if decode {
                n = n.wrapping_neg();
            }
            format!("Caesar shift {n}:\n{}", caesar(text, n))
        }
        "rot13" => format!("ROT13:\n{}", caesar(text, 13)),
        "atbash" => format!("Atbash:\n{}", atbash(text)),
        "vigenere" => {
            if crack {
                return Ok(guesses(&crack_vigenere(text, 16), "Vigenère") + &work(""));
            }
            let out = vigenere(text, key, decode)?;
            format!("Vigenère with key {} ({}):\n{out}", to_upper(key), dir(decode))
        }
        "affine" => {
            if crack {
                return Ok(guesses(&crack_affine(text), "affine") + &work(""));
            }
            let (a, k) = parse_affine_key(key)?;
            let out = affine(text, a, k, decode)?;
            format!("Affine with a={a}, b={k} ({}):\n{out}", dir(decode))
        }
        "beaufort" => {
            if crack {
                return Ok(guesses(&crack_beaufort(text, 16), "Beaufort") + &work(""));
            }
            let out = beaufort(text, key)?;
            format!("Beaufort with key {} (encoding and decoding are the same operation):\n{out}", to_upper(key))
        }
        "autokey" => {
            if crack {
                let (gs, per) = crack_autokey(text, 12);
                return Ok(guesses(&gs, "autokey") + &work(&per));
            }
            let out = autokey(text, key, decode)?;
            format!("Autokey with primer {} ({}):\n{out}", to_upper(key), dir(decode))
        }
        "columnar" => {
            if crack {
                let (gs, per) = crack_columnar(text, 8);
                let mut out = guesses(&gs, "columnar transposition (key = the rank of each column)");
                if !gotext::trim_space(text).contains([' ', '\n']) && lower_letters(text).len() < 80 {
                    out += "The text is short and has no spaces: an order that shows the message rotated (a few letters of its end first) can score as well as the right one; if the best guess starts or ends mid-word, read it as a circle.\n";
                }
                return Ok(out + &work(&per));
            }
            let order = column_order(key)?;
            let out = columnar_cipher(text, key, decode)?;
            format!("Columnar transposition with key {key}, column ranks {} ({}):\n{out}", column_ranks(&order), dir(decode))
        }
        "railfence" => {
            if crack {
                return Ok(guesses(&crack_rail_fence(text, 10), "rail fence") + &work(""));
            }
            let n = shift_key("railfence").map_err(|e| e + " (the number of rails)")?;
            let out = rail_fence(text, n, decode)?;
            format!("Rail fence with {n} rails ({}):\n{out}", dir(decode))
        }
        "substitution" => {
            if crack {
                let g = crack_substitution(text, 6);
                let mut b = format!(
                    "Substitution guess ({}). Cipher alphabet for plain a..z: {}\n{}",
                    g.note,
                    to_upper(&g.key),
                    truncate_text(&g.text, 1500)
                );
                if word_ratio(&g.text) < 0.2 {
                    b += "\nThis does not read as English: the hill-climb did not find the key (too little text, or not a simple substitution).";
                }
                return Ok(b + "\n" + &work(""));
            }
            let out = substitution(text, key, decode)?;
            format!("Substitution ({}):\n{out}", dir(decode))
        }
        _ => return Err(format!("unknown cipher {}", gotext::quote(cipher))),
    };
    Ok(out)
}
