//! Go: `tiers/guard.go`: what a tier's allow_data lets leave the machine. Secrets are redacted (the session redactor's rules,
//! broader ones for text the model or operator wrote, and every literal secret this process holds); a capture dump refuses
//! the tier, because it cannot be reliably cut out of free text. The caller's request is never changed.

use super::config::{AllowData, CLASS_NO_CAPTURES, CLASS_NO_SECRETS};
use crate::goregexp;
use crate::llm::openai::Request;
use crate::llm::types::Message;
use regex::Regex;
use std::sync::OnceLock;

fn pem_block() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        goregexp::compile(r"(?s)-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----.*?(?:-----END (?:[A-Z]+ )*PRIVATE KEY-----|$)").expect("pem")
    })
}

const EXTRA_RULES: &[(&str, &str)] = &[
    (r"(?i)\bBearer\s+[A-Za-z0-9._~+/=\-]{16,}", "Bearer [REDACTED:token]"),
    (
        r#"(?i)\b(api[_-]?key|secret|token|passwd|password|credential)s?(["']?\s*[:=]\s*["']?)[^\s"',;]{6,}"#,
        "${1}${2}[REDACTED:assignment]",
    ),
    (r"\bgh[opsur]_[A-Za-z0-9]{30,}", "[REDACTED:github]"),
    (r"\bglpat-[A-Za-z0-9_\-]{16,}", "[REDACTED:gitlab]"),
    (r"\bsk-[A-Za-z0-9_\-]{20,}", "[REDACTED:key]"),
    (r"\bhf_[A-Za-z0-9]{30,}", "[REDACTED:huggingface]"),
    (r"\bASIA[A-Z0-9]{16}\b", "[REDACTED:aws]"),
];

fn extra_rules() -> &'static [(Regex, &'static str)] {
    static R: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    R.get_or_init(|| EXTRA_RULES.iter().map(|(p, r)| (goregexp::compile(p).expect("secret rule"), *r)).collect())
}

/// Go's `RedactSecrets`: credential-shaped material and every literal in `known` (8 bytes or longer) replaced with
/// `[REDACTED:<kind>]`, and how many replacements were made.
pub fn redact_secrets(text: &str, known: &[String]) -> (String, usize) {
    let mut text = text.to_string();
    let mut count = 0;
    for k in known {
        if k.len() >= 8 && text.contains(k.as_str()) {
            count += text.matches(k.as_str()).count();
            text = text.replace(k.as_str(), "[REDACTED:key]");
        }
    }
    let mut pems = 0;
    text = pem_block()
        .replace_all(&text, |_: &regex::Captures| {
            pems += 1;
            "[REDACTED:pem]"
        })
        .into_owned();
    count += pems;
    let before = text.matches("[REDACTED:").count();
    text = crate::session::redact::redact(&text);
    count += text.matches("[REDACTED:").count() - before;
    for (re, repl) in extra_rules() {
        let mut n = 0;
        text = re
            .replace_all(&text, |c: &regex::Captures| {
                let m = &c[0];
                if m.contains("[REDACTED:") {
                    return m.to_string(); // already redacted; not counted twice
                }
                n += 1;
                re.replace_all(m, *repl).into_owned()
            })
            .into_owned();
        count += n;
    }
    (text, count)
}

const CAPTURE_RULES: &[&str] = &[
    r"(?m)^Filetype:\s*Flipper\b",
    r"(?mi)^RAW_Data:\s*-?\d",
    r"(?m)^(?:ATQA|SAK):\s*[0-9A-Fa-f]{2}",
    r"(?m)^UID:\s*(?:[0-9A-Fa-f]{2}\s){3,}",
    r"(?m)^Block\s+\d+:\s*(?:[0-9A-Fa-f]{2}\s){8,}",
];

fn capture_rules() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    R.get_or_init(|| CAPTURE_RULES.iter().map(|p| goregexp::compile(p).expect("capture rule")).collect())
}

/// Go's `LooksLikeCapture`: the text holds an RF/NFC capture dump.
pub fn looks_like_capture(text: &str) -> bool {
    capture_rules().iter().any(|re| re.is_match(text))
}

/// The refusal when allow_data forbids the request (Go's `RefusedError`); nothing was sent.
pub const CAPTURE_REFUSAL: &str = "the conversation contains capture data (RF/NFC dump) and allow_data has no_captures";

/// Go's `Sanitize`: a copy of `req` that may go to a tier with the given allow_data, and the number of redactions; Err is
/// the refusal.
pub fn sanitize(req: &Request, allow: &AllowData, known: &[String]) -> Result<(Request, usize), String> {
    if allow.all {
        return Ok((req.clone(), 0));
    }
    if allow.has(CLASS_NO_CAPTURES) {
        for m in &req.messages {
            if looks_like_capture(&m.content) || m.tool_calls.iter().any(|tc| looks_like_capture(&tc.function.arguments)) {
                return Err(CAPTURE_REFUSAL.into());
            }
        }
    }
    if !allow.has(CLASS_NO_SECRETS) {
        return Ok((req.clone(), 0));
    }
    let mut out = req.clone();
    let mut reds = 0;
    out.messages = req
        .messages
        .iter()
        .map(|m| {
            let (c, n) = redact_secrets(&m.content, known);
            reds += n;
            let mut m: Message = m.clone();
            m.content = c;
            for tc in &mut m.tool_calls {
                let (a, n) = redact_secrets(&tc.function.arguments, known);
                reds += n;
                tc.function.arguments = a;
            }
            m
        })
        .collect();
    Ok((out, reds))
}

/// Go's `describeGuard`.
pub fn describe_guard(redactions: usize) -> String {
    if redactions == 0 {
        String::new()
    } else {
        format!("{redactions} secret-shaped value(s) redacted before sending")
    }
}
