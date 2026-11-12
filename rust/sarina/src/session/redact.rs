//! Go: `go/session/redact.go`: high-confidence credentials in tool output replaced with `[REDACTED:<kind>]`.

use crate::goregexp;
use regex::Regex;
use std::sync::OnceLock;

/// The patterns in Go's order, each with its kind (None: the kind is the word after "sk-").
const RULES: &[(&str, Option<&str>)] = &[
    (r"sk-(ant|proj|or|live|test)-[A-Za-z0-9_\-]{8,}", None),
    (r"\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{16,}", Some("stripe")),
    (r"\bghp_[A-Za-z0-9]{20,}", Some("github")),
    (r"\bAKIA[A-Z0-9]{16,}", Some("aws")),
    (r"\bxoxb-[A-Za-z0-9\-]{20,}", Some("slack")),
    (r"\bAIza[A-Za-z0-9_\-]{35}", Some("google")),
    (r"\beyJ[A-Za-z0-9_\-]{5,}\.eyJ[A-Za-z0-9_\-]{5,}\.[A-Za-z0-9_\-]{5,}", Some("jwt")),
    (r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----", Some("pem")),
];

fn rules() -> &'static [(Regex, Option<&'static str>)] {
    static R: OnceLock<Vec<(Regex, Option<&'static str>)>> = OnceLock::new();
    R.get_or_init(|| RULES.iter().map(|(p, k)| (goregexp::compile(p).expect("redact pattern"), *k)).collect())
}

/// Go's `Redact` (applied to tool results only).
pub fn redact(text: &str) -> String {
    let mut text = text.to_owned();
    for (re, kind) in rules() {
        text = re
            .replace_all(&text, |c: &regex::Captures| {
                let m = &c[0];
                let k = match kind {
                    Some(k) => *k,
                    None => {
                        let rest = &m[3..];
                        &rest[..rest.find('-').unwrap_or(rest.len())]
                    }
                };
                format!("[REDACTED:{k}]")
            })
            .into_owned();
    }
    text
}
