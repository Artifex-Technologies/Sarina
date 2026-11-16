//! Go: `go/cryptotools`: cryptography Sarina does in code instead of in her head. Ported part by part, each proved
//! against Go's own answers (testdata/parity/classic.json, ...); the tools are offered by the Go build until the whole
//! set (with `stego`) is ported, so `needs_go` still hands SARINA_CRYPTO=on to Go (docs/RUST_PORT.md "Resume here").

pub mod classic;
pub mod english;

/// Go's `TruncateText`: the first n characters of s, and how many more there were.
pub fn truncate_text(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_owned();
    }
    let cut: String = s.chars().take(n).collect();
    format!("{cut}\n[... {} more characters]", count - n)
}

/// Go's `%.Nf` of a float64: Rust's digits, Go's names for the infinities and NaN.
pub fn fmt_f(x: f64, prec: usize) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x.is_infinite() {
        if x > 0.0 { "+Inf" } else { "-Inf" }.into()
    } else {
        format!("{x:.prec$}")
    }
}

/// Go's `strings.ToUpper`, character by character (a character whose upper case is more than one stays as it is).
pub fn to_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut u = c.to_uppercase();
            match (u.next(), u.next()) {
                (Some(one), None) => one,
                _ => c,
            }
        })
        .collect()
}
