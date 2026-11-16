//! Go: `go/stego`: read-only checks for data hidden in files and text. Ported part by part, each proved against Go's own
//! answers (testdata/parity/stegotext.json, ...); the tools are offered by the Go build until the whole set (with
//! `cryptotools`) is ported, so `needs_go` still hands SARINA_CRYPTO=on to Go (docs/RUST_PORT.md "Resume here").

pub mod deflate;
mod enc_tables;
pub mod encode;
pub mod flate;
pub mod gif;
pub mod goio;
pub mod images;
pub mod jpeg;
pub mod lsb;
pub mod magic;
pub mod meta;
pub mod png;
pub mod text;
pub mod tool;
pub mod watermark;
pub mod wav;

/// Go's `strings.ToValidUTF8(s, "\u{FFFD}")` on bytes: each run of bytes that are not UTF-8 becomes one U+FFFD.
pub fn to_valid_utf8(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut in_bad = false;
    while i < b.len() {
        match crate::gotext::decode_rune(&b[i..]) {
            (Some(c), n) => {
                let mut tmp = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                in_bad = false;
                i += n;
            }
            (None, _) => {
                if !in_bad {
                    out.extend_from_slice("\u{FFFD}".as_bytes());
                }
                in_bad = true;
                i += 1;
            }
        }
    }
    out
}

/// Go's `clipText`: trimmed, made valid UTF-8, cut to n bytes (inside a character too, as Go's s[:n]) with "…".
pub fn clip_text(s: &[u8], n: usize) -> Vec<u8> {
    let mut v = to_valid_utf8(crate::gotext::trim_space_bytes(s));
    if v.len() > n {
        v.truncate(n);
        v.extend_from_slice("…".as_bytes());
    }
    v
}
