//! Go: `cryptotools/identify.go` and `IdentifyReport`: crypto_identify: what a string or file is (PEM, SSH key, JWT,
//! UUID, password hashes, addresses, hex digests, encodings layered or not, XOR with a short key, the classical ciphers it
//! cracks itself with the work shown), ranked, with character statistics. On Go's bytes: a file need not be UTF-8
//! (testdata/parity/identify.json).

use super::classic::{
    self, atbash, crack_affine, crack_autokey, crack_beaufort, crack_caesar, crack_columnar, crack_rail_fence, crack_vigenere, Guess,
};
use super::decode::{base58_decode, decode_one, looks, looks_like_text, truncate_bytes};
use super::english::{chi_squared, entropy, index_of_coincidence, word_ratio};
use super::hash::strip_sep;
use super::{fmt_f, to_upper};
use crate::gosort::slice_stable;
use crate::gotext::{self, lossy};
use regex::Regex;
use std::sync::LazyLock;

/// One guess at what the input is (Go's `Candidate`); its texts are Go's bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    pub what: String,
    pub score: i64,
    pub evidence: Vec<u8>,
    pub preview: Vec<u8>,
    pub work: String,
}

/// Character statistics (Go's `Stats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub length: i64,
    pub letters: i64,
    pub digits: i64,
    pub punct: i64,
    pub spaces: i64,
    pub non_printable: i64,
    pub upper: i64,
    pub lower: i64,
    pub entropy: f64,
    pub zero_width: i64,
}

impl Stats {
    /// Go's `Stats.Describe`.
    pub fn describe(&self) -> String {
        let zw = if self.zero_width > 0 {
            format!("; {} INVISIBLE characters (zero-width or tag characters: hidden text, see stego_inspect)", self.zero_width)
        } else {
            String::new()
        };
        format!(
            "Length {} characters: {} letters ({} upper, {} lower), {} digits, {} punctuation, {} spaces, {} non-printable; entropy {} bits per byte{zw}",
            self.length,
            self.letters,
            self.upper,
            self.lower,
            self.digits,
            self.punct,
            self.spaces,
            self.non_printable,
            fmt_f(self.entropy, 2)
        )
    }
}

fn is_zero_width(r: char) -> bool {
    matches!(
        r as u32,
        0x200B
            | 0x200C
            | 0x200D
            | 0x2060
            | 0x2061
            | 0x2062
            | 0x2063
            | 0x2064
            | 0xFEFF
            | 0x00AD
            | 0x180E
            | 0x200E
            | 0x200F
            | 0x202A
            | 0x202B
            | 0x202C
            | 0x202D
            | 0x202E
    ) || (0xE0000..=0xE007F).contains(&(r as u32))
}

/// Go's `CharStats`.
pub fn char_stats(s: &[u8]) -> Stats {
    let mut st = Stats::default();
    for r in lossy(s).chars() {
        st.length += 1;
        if gotext::is_letter(r) {
            st.letters += 1;
            if r.is_uppercase() {
                st.upper += 1;
            } else {
                st.lower += 1;
            }
        } else if gotext::is_digit(r) {
            st.digits += 1;
        } else if matches!(r, ' ' | '\n' | '\t' | '\r') {
            st.spaces += 1;
        } else if is_zero_width(r) {
            st.zero_width += 1;
            st.non_printable += 1;
        } else if gotext::is_print(r) {
            st.punct += 1;
        } else {
            st.non_printable += 1;
        }
    }
    st.entropy = entropy(s);
    st
}

fn re(p: &str) -> Regex {
    crate::goregexp::compile(p).expect("identify pattern")
}
static PEM_RE: LazyLock<Regex> = LazyLock::new(|| re(r"-----BEGIN ([A-Z0-9 ]+)-----"));
static JWT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{2,}\.[A-Za-z0-9_-]*$"));
static UUID_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"));
static CRYPT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^\$(2[aby]?|argon2(id|i|d)|6|5|1|7|y|pbkdf2[-\w]*|sha1|md5|scrypt)\$"));
static NTLM_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[0-9a-fA-F]{32}:[0-9a-fA-F]{32}$"));
static SSH_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(ssh-(rsa|ed25519|dss)|ecdsa-sha2-nistp(256|384|521)) [A-Za-z0-9+/=]+"));
static IPV4_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(\d{1,3}\.){3}\d{1,3}$"));
static MAC_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^([0-9a-f]{2}[:-]){5}[0-9a-f]{2}$"));
static BTC_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(1|3)[1-9A-HJ-NP-Za-km-z]{25,34}$|^bc1[a-z0-9]{25,60}$"));
static ETH_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^0x[0-9a-fA-F]{40}$"));
static HEX_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:[0-9a-fA-F]{2}[\s:,-]*)+$"));

fn hash_kind(n: usize) -> Option<&'static str> {
    Some(match n {
        8 => "CRC32 (or a short checksum)",
        16 => "a 64-bit hash (CRC64, xxHash64, SipHash) or half an MD5",
        32 => "MD5, NTLM, LM or MD4 (all 128 bits)",
        40 => "SHA-1, RIPEMD-160 or a Git object id",
        56 => "SHA-224, SHA3-224",
        64 => "SHA-256, SHA3-256, BLAKE2s-256 or a Keccak-256 (Ethereum) hash",
        96 => "SHA-384 or SHA3-384",
        128 => "SHA-512, SHA3-512, BLAKE2b-512 or Whirlpool",
        _ => return None,
    })
}

fn crypt_kind(prefix: &str) -> &'static str {
    if prefix.starts_with("$2") {
        "bcrypt password hash"
    } else if prefix.starts_with("$argon2") {
        "Argon2 password hash"
    } else if prefix == "$6" {
        "sha512crypt password hash (Linux /etc/shadow)"
    } else if prefix == "$5" {
        "sha256crypt password hash"
    } else if prefix == "$1" {
        "md5crypt password hash"
    } else if prefix == "$y" || prefix == "$7" {
        "yescrypt or scrypt password hash"
    } else if prefix.starts_with("$pbkdf2") {
        "PBKDF2 password hash"
    } else {
        "a modular-crypt password hash"
    }
}

/// Go's `readable`: common words when there are spaces, letter pairs and word cover when there are none.
fn readable(t: &str, words: f64) -> bool {
    if gotext::trim_space(t).contains([' ', '\n']) {
        return word_ratio(t) >= words;
    }
    classic::reads_as_english(t)
}

/// Go's `xorFit`.
fn xor_fit(b: &[u8]) -> f64 {
    const FREQ: [f64; 26] = [
        0.08167, 0.01492, 0.02782, 0.04253, 0.12702, 0.02228, 0.02015, 0.06094, 0.06966, 0.00153, 0.00772, 0.04025, 0.02406, 0.06749,
        0.07507, 0.01929, 0.00095, 0.05987, 0.06327, 0.09056, 0.02758, 0.00978, 0.02360, 0.00150, 0.01974, 0.00074,
    ];
    let mut score = 0.0;
    for &c in b {
        score += match c {
            b' ' => 13.0,
            b'a'..=b'z' => FREQ[(c - b'a') as usize] * 100.0,
            b'A'..=b'Z' => FREQ[(c - b'A') as usize] * 50.0,
            0x20..=0x7e | b'\n' => 1.0,
            _ => -20.0,
        };
    }
    score
}

/// Go's `crackXOR`: the best single-byte or repeating-key (2-8 bytes) XOR that reads as text: (score, text, note).
fn crack_xor(b: &[u8]) -> Option<(i64, Vec<u8>, String)> {
    let mut best: Option<(i64, Vec<u8>, String)> = None;
    let mut try_key = |key: &[u8], how: &str| {
        let out: Vec<u8> = b.iter().enumerate().map(|(i, x)| x ^ key[i % key.len()]).collect();
        let t = lossy(&out);
        if !looks_like_text(&out) || word_ratio(&t) < 0.15 {
            return;
        }
        let score = (70.0 + 30.0 * word_ratio(&t)) as i64;
        if best.as_ref().is_none_or(|g| score > g.0) {
            let k: String = key.iter().map(|x| format!("{x:02x}")).collect();
            let note = format!("{how}: key {k} (hex), {}% common words", fmt_f(100.0 * word_ratio(&t), 0));
            best = Some((score, out, note));
        }
    };
    for k in 1..=255u8 {
        try_key(&[k], "single-byte XOR");
    }
    if b.len() < 24 {
        return best;
    }
    let mut key_len = 2;
    while key_len <= 8 && key_len * 3 <= b.len() {
        let mut key = vec![0u8; key_len];
        for (c, slot) in key.iter_mut().enumerate() {
            let col: Vec<u8> = b.iter().skip(c).step_by(key_len).copied().collect();
            let (mut best_k, mut best_fit) = (0u8, -1.0);
            for k in 0..=255u8 {
                let out: Vec<u8> = col.iter().map(|x| x ^ k).collect();
                let fit = xor_fit(&out);
                if fit > best_fit {
                    best_k = k;
                    best_fit = fit;
                }
            }
            *slot = best_k;
        }
        try_key(&key, &format!("repeating-key XOR (key length {key_len})"));
        key_len += 1;
    }
    best
}

/// Go's `Identify`: the candidates, most likely first, and the input's statistics.
pub fn identify(s: &[u8]) -> (Vec<Candidate>, Stats) {
    let st = char_stats(s);
    let tb = gotext::trim_space_bytes(s);
    let t = lossy(tb);
    let mut cs: Vec<Candidate> = Vec::new();
    fn add(cs: &mut Vec<Candidate>, what: &str, score: i64, evidence: impl Into<Vec<u8>>, preview: impl Into<Vec<u8>>) {
        cs.push(Candidate { what: what.into(), score, evidence: evidence.into(), preview: preview.into(), work: String::new() });
    }
    let worked = |cs: &mut Vec<Candidate>, cipher: &str, per: &str| {
        if let Some(last) = cs.last_mut() {
            last.work = classic::how_it_fell(&t, cipher, per);
        }
    };
    if tb.is_empty() {
        return (cs, st);
    }
    if let Some(m) = PEM_RE.captures(&t) {
        let label = &m[1];
        if label.starts_with("PGP") {
            add(
                &mut cs,
                &format!("PGP armored block: {label}"),
                100,
                format!("the -----BEGIN {label}----- header"),
                "crypto_modern reads its packets (what kind, which key id) without decrypting anything",
            );
        } else if label.contains("PRIVATE KEY") {
            add(
                &mut cs,
                &format!("a PRIVATE key ({label})"),
                100,
                "the PEM header",
                "a secret: I do not decode, print or load private keys; keep it out of the conversation",
            );
        } else {
            add(
                &mut cs,
                &format!("PEM block: {label}"),
                100,
                format!("the -----BEGIN {label}----- header"),
                "crypto_modern parses certificates and public keys",
            );
        }
        return (cs, st);
    }
    if SSH_RE.is_match(&t) {
        add(&mut cs, "an SSH public key", 100, "the key type prefix", "crypto_modern gives its type, size and fingerprint");
        return (cs, st);
    }
    if JWT_RE.is_match(&t) {
        let first = t.split('.').next().unwrap_or_default().trim_end_matches('=');
        let url64 = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        if let Ok(hdr) = super::decode::base64_raw(first.as_bytes(), url64) {
            if crate::gojson::unmarshal_any(&hdr).1.is_none() && hdr.windows(3).any(|w| w == b"alg") {
                let mut ev = b"three base64url parts separated by dots; the first decodes to a JSON header: ".to_vec();
                ev.extend_from_slice(&hdr);
                add(&mut cs, "a JWT (JSON Web Token)", 98, ev, "crypto_modern decodes it; the signature is NOT verified without the key");
                return (cs, st);
            }
        }
    }
    if UUID_RE.is_match(&t) {
        add(&mut cs, &format!("a UUID (version {})", &t[14..15]), 95, "8-4-4-4-12 hex groups with a version digit", "");
        return (cs, st);
    }
    if NTLM_RE.is_match(&t) {
        add(
            &mut cs,
            "an LM:NTLM hash pair (Windows)",
            90,
            "two 32-hex-digit halves separated by a colon",
            "a hash: not reversible; it can only be compared against candidates",
        );
        return (cs, st);
    }
    if let Some(m) = CRYPT_RE.captures(&t) {
        let p = format!("${}", &m[1]);
        add(
            &mut cs,
            crypt_kind(&p),
            95,
            format!("the {p}$ prefix of the modular crypt format"),
            "a salted, slow password hash: not reversible; only a guess can be checked against it",
        );
        return (cs, st);
    }
    if ETH_RE.is_match(&t) {
        add(&mut cs, "an Ethereum address (or a 160-bit hash with 0x)", 70, "0x and 40 hex digits", "");
    }
    if BTC_RE.is_match(&t) {
        add(&mut cs, "a Bitcoin address", 60, "base58 with a 1 or 3 prefix (or bech32 bc1)", "");
    }
    if IPV4_RE.is_match(&t) {
        add(&mut cs, "an IPv4 address", 90, "four dotted numbers", "");
        return (cs, st);
    }
    if MAC_RE.is_match(&t) {
        add(&mut cs, "a MAC address", 85, "six hex pairs", "");
        return (cs, st);
    }
    let compact = strip_sep(&t);
    if HEX_RE.is_match(&t) {
        if let Some(kind) = hash_kind(compact.len()).filter(|_| compact.len() >= 32) {
            add(&mut cs, &format!("a hex digest: {kind}"), 85, format!("{} hex digits = {} bits", compact.len(), compact.len() * 4), "a hash is one-way: it says nothing about its input; it can be compared against the hash of a candidate (crypto_hash with expected)");
        }
        if let Ok(b) = super::decode::hex_decode(compact.as_bytes()) {
            if looks_like_text(&b) {
                add(&mut cs, "hex-encoded text", 92, "every character is a hex digit and the bytes read as text", truncate_bytes(&b, 200));
            } else if compact.len() < 32 || !hash_kind(compact.len()).unwrap_or_default().contains("SHA") {
                add(
                    &mut cs,
                    "hex-encoded bytes",
                    60,
                    format!("hex digits; the {} bytes do not read as text (entropy {})", b.len(), fmt_f(entropy(&b), 2)),
                    "",
                );
            }
            if b.len() >= 8 && !looks_like_text(&b) {
                if let Some((score, text, note)) = crack_xor(&b) {
                    add(&mut cs, "bytes XORed with a short key", score, note, truncate_bytes(&text, 200));
                }
            }
        }
    }
    let fields = gotext::fields_bytes(tb);
    let short_words = fields.iter().any(|f| f.len() < 12);
    let long_enough = tb.iter().filter(|&&c| c != b' ').count() >= 16;
    let already_english = word_ratio(&t) >= 0.3 || (fields.len() == 1 && looks_like_text(tb) && word_ratio(&t) > 0.0);
    if looks("base64", tb) && !HEX_RE.is_match(&t) && !(short_words && tb.contains(&b' ')) && long_enough && !already_english {
        if let Ok(b) = decode_one("base64", tb) {
            if looks_like_text(&b) {
                add(&mut cs, "base64-encoded text", 90, "base64 characters, and the bytes read as text", truncate_bytes(&b, 200));
            } else if looks("hex", &b) {
                let mut preview = b"hex: ".to_vec();
                preview.extend_from_slice(&truncate_bytes(&b, 120));
                if let Ok(inner) = decode_one("hex", &b) {
                    if looks_like_text(&inner) {
                        preview = b"text (base64 -> hex -> text): ".to_vec();
                        preview.extend_from_slice(&truncate_bytes(&inner, 200));
                    }
                }
                add(&mut cs, "base64 of hex", 80, "base64 whose bytes are hex digits", preview);
            } else {
                add(&mut cs, "base64-encoded bytes", 55, format!("base64 characters; the {} bytes do not read as text (entropy {}): could be ciphertext, a key, compressed or binary data", b.len(), fmt_f(entropy(&b), 2)), "");
                if b.len() >= 8 {
                    if let Some((score, text, note)) = crack_xor(&b) {
                        add(&mut cs, "base64 of bytes XORed with a short key", score, note, truncate_bytes(&text, 200));
                    }
                }
            }
        }
    }
    let decoded_text = |enc: &str| decode_one(enc, tb).ok().filter(|b| looks_like_text(b));
    if looks("base32", tb) {
        if let Some(b) = decoded_text("base32") {
            add(&mut cs, "base32-encoded text", 88, "A-Z and 2-7 only, and the bytes read as text", truncate_bytes(&b, 200));
        }
    }
    if looks("base58", tb) && cs.is_empty() {
        if let Some(b) = base58_decode(&t).ok().filter(|b| looks_like_text(b)) {
            add(&mut cs, "base58-encoded text", 85, "base58 alphabet (no 0, O, I, l), and the bytes read as text", truncate_bytes(&b, 200));
        }
    }
    if looks("ascii85", tb) {
        if let Some(b) = decoded_text("ascii85") {
            add(&mut cs, "Ascii85-encoded text", 90, "the <~ ~> frame, and the bytes read as text", truncate_bytes(&b, 200));
        }
    }
    if looks("binary", tb) {
        if let Some(b) = decoded_text("binary") {
            add(&mut cs, "binary (0/1) encoded text", 90, "only 0 and 1, eight bits per character", truncate_bytes(&b, 200));
        }
    }
    if looks("decimal", tb) {
        if let Some(b) = decoded_text("decimal") {
            add(&mut cs, "decimal byte values (ASCII codes)", 85, "numbers 0-255 separated by spaces or commas", truncate_bytes(&b, 200));
        }
    }
    if looks("morse", tb) {
        if let Ok(b) = decode_one("morse", tb) {
            add(&mut cs, "Morse code", 90, "dots and dashes", truncate_bytes(&b, 200));
        }
    }
    if looks("url", tb) {
        if let Ok(b) = decode_one("url", tb) {
            if b != tb {
                add(&mut cs, "URL (percent) encoded text", 80, "%XX escapes", truncate_bytes(&b, 200));
            }
        }
    }
    let decoded = cs.iter().any(|c| c.score >= 80);
    if st.letters >= 8 && st.letters * 2 > st.length && !decoded {
        if word_ratio(&t) >= 0.3 {
            add(
                &mut cs,
                "plain English text",
                90,
                format!("{}% of its words are common English words", fmt_f(100.0 * word_ratio(&t), 0)),
                "",
            );
        } else {
            let gs = crack_caesar(&t);
            let at = atbash(&t);
            if !gs.is_empty() && word_ratio(&gs[0].text) >= 0.25 && gs[0].key != "shift 0" {
                add(
                    &mut cs,
                    &format!("a shifted alphabet (Caesar/ROT), {}", gs[0].key),
                    85,
                    gs[0].note.clone(),
                    super::truncate_text(&gs[0].text, 200),
                );
            } else if word_ratio(&at) >= 0.3 {
                add(
                    &mut cs,
                    "Atbash (mirrored alphabet)",
                    85,
                    format!("{}% common words after mirroring", fmt_f(100.0 * word_ratio(&at), 0)),
                    super::truncate_text(&at, 200),
                );
            } else {
                cipher_guesses(&mut cs, &t, tb, &st, &worked);
            }
        }
    }
    if cs.is_empty() {
        let how = if st.entropy > 7.2 {
            "high, like encrypted, compressed or random data"
        } else {
            "low: structured data or text in a form I do not know"
        };
        add(&mut cs, "nothing I recognise", 10, format!("entropy {} bits per byte: {how}", fmt_f(st.entropy, 2)), "");
    }
    slice_stable(&mut cs, |a, b| a.score > b.score);
    (cs, st)
}

/// The letters that are not English as they stand: Vigenère, Beaufort, autokey, affine, transpositions, or what they
/// may be when nothing reads.
fn cipher_guesses(cs: &mut Vec<Candidate>, t: &str, tb: &[u8], st: &Stats, worked: &dyn Fn(&mut Vec<Candidate>, &str, &str)) {
    let ic = index_of_coincidence(t);
    let small = st.letters <= 3000;
    let (mut aff, mut bf, mut ak, mut tr) = (Vec::new(), Vec::new(), Vec::new(), Vec::<Guess>::new());
    let (mut ak_per, mut col_per) = (String::new(), String::new());
    if small && st.letters >= 20 {
        aff = crack_affine(t);
    }
    if small && ic < 0.05 && st.letters >= 40 {
        bf = crack_beaufort(t, 16);
        (ak, ak_per) = crack_autokey(t, 12);
    }
    let chi = chi_squared(t);
    let transposed = ic >= 0.055 && st.letters >= 30 && chi < 100.0;
    if small && transposed {
        let (col, per) = crack_columnar(t, 8);
        let mut rails = crack_rail_fence(t, 10);
        for r in rails.iter_mut() {
            r.note += " (rail fence)";
        }
        rails.extend(col);
        tr = rails;
        col_per = per;
        slice_stable(&mut tr, |a, b| readable(&a.text, 0.4) && !readable(&b.text, 0.4));
    }
    let add = |cs: &mut Vec<Candidate>, what: String, score: i64, evidence: String, preview: String| {
        cs.push(Candidate { what, score, evidence: evidence.into_bytes(), preview: preview.into_bytes(), work: String::new() });
    };
    let icf = fmt_f(ic, 3);
    let gs = crack_vigenere(t, 16);
    if !gs.is_empty() && word_ratio(&gs[0].text) >= 0.25 {
        add(
            cs,
            format!("a Vigenère cipher, key {}", to_upper(&gs[0].key)),
            80,
            format!("letters with index of coincidence {icf} (English 0.066, random 0.038); {}", gs[0].note),
            super::truncate_text(&gs[0].text, 200),
        );
    } else if !bf.is_empty() && readable(&bf[0].text, 0.4) {
        add(
            cs,
            format!("a Beaufort cipher, key {}", to_upper(&bf[0].key)),
            80,
            format!("letters with index of coincidence {icf} (English 0.066, random 0.038); {}", bf[0].note),
            super::truncate_text(&bf[0].text, 200),
        );
        worked(cs, "beaufort", "");
    } else if !ak.is_empty() && readable(&ak[0].text, 0.4) {
        add(
            cs,
            format!("an autokey cipher, primer {}", to_upper(&ak[0].key)),
            80,
            format!("letters with index of coincidence {icf} (English 0.066, random 0.038), and no repeating key; {}", ak[0].note),
            super::truncate_text(&ak[0].text, 200),
        );
        worked(cs, "autokey", &ak_per);
    } else if !aff.is_empty() && aff[0].key != "a=1, b=0" && readable(&aff[0].text, 0.4) {
        add(
            cs,
            format!("an affine cipher, key {}", aff[0].key),
            85,
            format!("all 312 keys tried, each decoding's letter frequencies compared with English; the best: {}", aff[0].note),
            super::truncate_text(&aff[0].text, 200),
        );
        worked(cs, "affine", "");
    } else if !tr.is_empty() && tr[0].text.as_bytes() != tb && readable(&tr[0].text, 0.4) {
        add(
            cs,
            format!("a transposition, key {}", tr[0].key),
            80,
            format!(
                "the letters' own frequencies are English (chi-squared {}), only their order is changed; {}",
                fmt_f(chi, 0),
                tr[0].note
            ),
            super::truncate_text(&tr[0].text, 200),
        );
        if tr[0].note.ends_with("(rail fence)") {
            worked(cs, "railfence", "");
        } else {
            worked(cs, "columnar", &col_per);
        }
    } else if ic < 0.05 && st.letters >= 40 {
        add(cs, "a polyalphabetic cipher (Vigenère-like) or random letters".into(), 50, format!("flat letter frequencies: index of coincidence {icf} (English 0.066); the Vigenère, Beaufort and autokey crackers found no readable key"), String::new());
    } else if transposed && st.letters >= 60 && !t.contains([' ', '\n']) {
        add(cs, "a transposition (letters moved, not replaced)".into(), 55, format!("the letters' own frequencies are English (chi-squared {}, index of coincidence {icf}) but no English words; the rail fence and columnar crackers (key lengths up to 8) found no readable order", fmt_f(chi, 0)), String::new());
    } else if ic >= 0.055 && st.letters >= 60 {
        add(cs, "a monoalphabetic substitution".into(), 55, format!("English-like letter spread (index of coincidence {icf}) but no English words, and no affine key reads: each letter replaced by another; crypto_classic with cipher substitution and crack tries it"), String::new());
    } else if cs.iter().map(|c| c.score).max().unwrap_or(0) < 50 {
        add(
            cs,
            "letters, not English: a cipher, another language or a name".into(),
            30,
            format!("{}% common English words, index of coincidence {icf}", fmt_f(100.0 * word_ratio(t), 0)),
            String::new(),
        );
    }
}

/// Go's `IdentifyReport`: crypto_identify's answer, as Go's bytes.
pub fn identify_report(s: &[u8], src: &str) -> Vec<u8> {
    let (cs, st) = identify(s);
    let mut b = format!("What the {src} looks like, most likely first:\n").into_bytes();
    let indent = |x: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(x.len());
        for &c in x {
            out.push(c);
            if c == b'\n' {
                out.extend_from_slice(b"   ");
            }
        }
        out
    };
    for (i, c) in cs.iter().enumerate() {
        b.extend_from_slice(format!("{}. {} (fit {}/100). Evidence: ", i + 1, c.what, c.score).as_bytes());
        b.extend_from_slice(&c.evidence);
        b.push(b'.');
        if !c.preview.is_empty() {
            b.extend_from_slice(b"\n   -> ");
            b.extend_from_slice(&indent(&c.preview));
        }
        if !c.work.is_empty() {
            b.extend_from_slice(b"\n   ");
            b.extend_from_slice(&indent(c.work.trim_end_matches('\n').as_bytes()));
        }
        b.push(b'\n');
    }
    b.extend_from_slice(format!("{}.\n", st.describe()).as_bytes());
    b
}
