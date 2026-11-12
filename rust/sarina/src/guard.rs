//! Go: `guard`: the one text gate. Anything written where a future prompt reads from (curated memory, skills, session
//! notes, compaction summaries) goes through it: invisible and bidirectional Unicode, instruction-override phrases and
//! credential shapes are refused (`scan_local`); a gate with a Heimdall kind then asks Heimdall for an out-of-process
//! second opinion (`scan`), which fails open when Heimdall is absent and closed on its "deny".
//!
//! The patterns are Go's, read with Go's meaning (`goregexp`); Go's regexp sees a byte that is not UTF-8 as U+FFFD, and so
//! does this (the match is mapped back to the original bytes for the message).

use crate::gojson::quote_bytes;
use crate::goregexp;
use crate::gotext;
use regex::Regex;
use std::sync::OnceLock;

/// A scan's verdict (Go's `Result`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verdict {
    pub blocked: bool,
    pub reason: String,
}

/// The shared gate (Go's `Gate`).
#[derive(Clone, Debug, Default)]
pub struct Gate {
    /// when set, the full scan asks Heimdall with this kind after the local checks pass ("memory_proposal")
    pub heimdall_kind: String,
    pub session_id: String,
    /// a byte-order mark at the very start is allowed (whole files read from disk)
    pub allow_leading_bom: bool,
}

impl Gate {
    /// Go's `Default`: all local checks, no Heimdall call.
    pub fn default_gate() -> Gate {
        Gate { allow_leading_bom: true, ..Gate::default() }
    }

    /// Go's `Memory`: local checks, then Heimdall.
    pub fn memory() -> Gate {
        Gate { heimdall_kind: "memory_proposal".into(), ..Gate::default() }
    }
}

const INVISIBLE: [u32; 6] = [0x200B, 0x200C, 0x200D, 0x2060, 0xFEFF, 0x00AD];

const INJECTION: [&str; 15] = [
    r"(?i)ignore (all|any|the)?\s*(previous|prior|above)\s*instructions",
    r"(?i)disregard (all|any|the)?\s*(previous|prior|above)",
    r"(?i)you are now\b",
    r"(?i)new (system )?instructions?\s*:",
    r"(?i)act as if\b",
    r"(?i)override (your|the|all) instructions",
    r"(?i)do not (tell|inform|notify) the user",
    r"(?i)without (informing|telling|notifying) the user",
    r"(?i)system prompt\s*:",
    r"(?i)\b(ignore|disregard|forget|override)\b[^.\n]{0,40}\b(previous|prior|above|earlier|all|any|your)\b[^.\n]{0,40}\b(instructions?|rules|prompts?|guidelines|constraints)\b",
    r"(?i)\byou are no longer\b",
    r"(?i)\b(reveal|print|show|output)\b[^.\n]{0,30}\b(system prompt|hidden instructions)\b",
    r"(?i)\b(system|important|urgent|admin(istrator)?|security) (notice|message|instructions?|directive)s? (to|for) (the )?(ai|assistant|agent|model|llm)\b",
    r"(?i)\b(notes?|messages?|instructions?) (to|for) (the )?(ai|assistant|agent|llm|language model)\s*:",
    r"(?i)\b(bypass|disable|turn off)\b[^.\n]{0,30}\b(safety|permission|approval|guardrails?)\b",
];

const CREDENTIAL: [&str; 12] = [
    r"-----BEGIN [A-Z ]*PRIVATE KEY",
    r"ssh-(rsa|ed25519|dss) AAAA[0-9A-Za-z+/]{10,}",
    r"\b(AKIA|ASIA)[0-9A-Z]{16}\b",
    r"ghp_[0-9A-Za-z]{36}",
    r"\bgh[pousr]_[A-Za-z0-9]{30,}",
    r"\bsk-[A-Za-z0-9_-]{20,}",
    r"xox[baprs]-[0-9A-Za-z-]{10,}",
    r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    r#"(?i)api[_-]?key\s*[:=]\s*['"]?[0-9A-Za-z_\-]{16,}"#,
    r"(?i)password\s*[:=]\s*\S+",
    r#"(?i)secret[_-]?(key|token)?\s*[:=]\s*['"]?[0-9A-Za-z_\-]{12,}"#,
    r#"(?i)\b(token|passw(or)?d|private[_-]?key)\b["']?\s*[:=]\s*["']?[A-Za-z0-9_\-+/=.]{12,}"#,
];

fn compiled(which: &'static OnceLock<Vec<Regex>>, pats: &[&str]) -> &'static [Regex] {
    which.get_or_init(|| pats.iter().map(|p| goregexp::compile(p).expect("guard's patterns compile")).collect())
}

fn injection() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    compiled(&R, &INJECTION)
}

fn credential() -> &'static [Regex] {
    static R: OnceLock<Vec<Regex>> = OnceLock::new();
    compiled(&R, &CREDENTIAL)
}

impl Gate {
    /// Go's `Scan`: the local checks, then, for a gate with a Heimdall kind, Heimdall's verdict (HEIMDALL_REVIEW_URL read
    /// through `getenv`).
    pub fn scan(&self, text: &[u8], getenv: &dyn Fn(&str) -> String) -> Verdict {
        let v = self.scan_local(text);
        if v.blocked || self.heimdall_kind.is_empty() {
            return v;
        }
        match crate::heimdall::consult(&gotext::lossy(text), &self.heimdall_kind, &self.session_id, getenv) {
            Some(h) if h.denied => Verdict {
                blocked: true,
                reason: format!("Heimdall flagged this independently: {}", h.reasons.unwrap_or_default().join("; ")),
            },
            _ => Verdict::default(),
        }
    }

    /// Go's `Scan` without the Heimdall call: the first invisible or format character, else the first instruction-like
    /// phrase (quoted), else a credential shape.
    pub fn scan_local(&self, text: &[u8]) -> Verdict {
        let mut i = 0;
        while i < text.len() {
            let (c, n) = gotext::decode_rune(&text[i..]);
            let r = c.unwrap_or('\u{FFFD}');
            let at = i;
            i += n.max(1);
            if self.allow_leading_bom && at == 0 && r == '\u{FEFF}' {
                continue;
            }
            let u = r as u32;
            if INVISIBLE.contains(&u) || gotext::is_format(r) || (0xE0000..0xE0080).contains(&u) {
                return Verdict {
                    blocked: true,
                    reason: format!("contains an invisible/formatting Unicode character (U+{u:04X}), which is not permitted here"),
                };
            }
        }
        let (s, at) = gotext::lossy_with_offsets(text);
        for p in injection() {
            if let Some(m) = p.find(&s) {
                if !m.as_str().is_empty() {
                    let found = &text[at[m.start()]..at[m.end()]];
                    return Verdict {
                        blocked: true,
                        reason: format!("looks like an instruction rather than a fact ({})", quote_bytes(found)),
                    };
                }
            }
        }
        if credential().iter().any(|p| p.is_match(&s)) {
            return Verdict { blocked: true, reason: "looks like it contains a credential or secret; those belong outside memory".into() };
        }
        Verdict::default()
    }
}
