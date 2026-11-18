//! Go: `chymaera/analysis`: statistics over Argus App captures, from what `capture.list` gives (each record's `meta` JSON;
//! the .nfc/.sub files stay the app's to parse). Semantic grouping needs an embedding model and is the Python helper's.

use crate::goany::Any;
use crate::pyfmt;
use std::collections::BTreeMap;

/// A key with the capture names that carry it, in input order (Go's `NameList`).
struct NameList {
    key: String,
    names: Vec<String>,
}

/// Go's `CaptureAnalysis`.
pub struct Analysis {
    total: usize,
    by_type: BTreeMap<String, i64>,
    duplicate_uids: Vec<NameList>,
    rare_frequencies: Vec<String>,
    pub semantic_groups: Option<Vec<Vec<String>>>,
    semantic_unavailable: String,
}

impl Analysis {
    /// Go's `AsText`.
    pub fn as_text(&self) -> String {
        let mut lines = vec!["# Capture analysis".to_string(), String::new(), format!("total captures: {}", self.total)];
        for (k, n) in &self.by_type {
            lines.push(format!("  {k}: {n}"));
        }
        lines.push(String::new());
        if !self.duplicate_uids.is_empty() {
            lines.push("## Duplicate NFC UIDs".into());
            lines.push("Same UID read more than once - could be a re-read, or a cloned/replayed tag.".into());
            for d in &self.duplicate_uids {
                lines.push(format!("- {}: {}", d.key, d.names.join(", ")));
            }
            lines.push(String::new());
        }
        if !self.rare_frequencies.is_empty() {
            lines.push("## Rare Sub-GHz frequencies (seen once)".into());
            for f in &self.rare_frequencies {
                lines.push(format!("- {f} Hz"));
            }
            lines.push(String::new());
        }
        match &self.semantic_groups {
            Some(groups) if !groups.is_empty() => {
                lines.push("## Semantic groups (similar captures)".into());
                for g in groups {
                    lines.push(format!("- {}", g.join(", ")));
                }
                lines.push(String::new());
            }
            _ if !self.semantic_unavailable.is_empty() => {
                lines.push(format!("Semantic grouping unavailable: {}", self.semantic_unavailable));
            }
            _ => {}
        }
        lines.join("\n").trim_end_matches(crate::gotext::is_space).to_string()
    }
}

/// Go's `parseMeta`: a capture's `meta` read with `UseNumber`; anything unusable (not an object, or followed by another
/// token) is nothing.
fn parse_meta(capture: &BTreeMap<String, Any>) -> Option<BTreeMap<String, Any>> {
    let Some(Any::String(raw)) = capture.get("meta") else { return None };
    if raw.is_empty() {
        return None;
    }
    let b = raw.as_bytes();
    let (first, end) = crate::gojson::parse_first_end(b);
    let v = first.ok()?;
    if next_token(&b[end..]) {
        return None; // trailing data: not valid JSON
    }
    crate::gojson::to_any_use_number(&v).into_object().ok()
}

/// Whether Go's `Decoder.Token` after the first value reads one without an error: an opening bracket, or a whole value
/// (a number ends where its digits do).
fn next_token(rest: &[u8]) -> bool {
    let i = rest.iter().position(|c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'));
    let Some(i) = i else { return false };
    match rest[i] {
        b'[' | b'{' => true,
        _ => crate::gojson::parse_first_end(&rest[i..]).0.is_ok(),
    }
}

fn or_default(capture: &BTreeMap<String, Any>, key: &str, def: &str) -> String {
    capture.get(key).map_or(def.to_string(), pyfmt::str)
}

#[derive(Default)]
struct Ordered {
    list: Vec<NameList>,
}

impl Ordered {
    fn add(&mut self, key: String, name: String) {
        match self.list.iter_mut().find(|l| l.key == key) {
            Some(l) => l.names.push(name),
            None => self.list.push(NameList { key, names: vec![name] }),
        }
    }
}

/// Go's `Analyze`: capture records as `capture.list` returns them; `semantic_reason` is why grouping did not run.
pub fn analyze(captures: &[BTreeMap<String, Any>], semantic_reason: &str) -> Analysis {
    let mut by_type: BTreeMap<String, i64> = BTreeMap::new();
    let (mut uids, mut freqs) = (Ordered::default(), Ordered::default());
    for c in captures {
        *by_type.entry(or_default(c, "type", "unknown")).or_default() += 1;
        let name = or_default(c, "name", "?");
        let meta = parse_meta(c).unwrap_or_default();
        if let Some(uid) = meta.get("uid").filter(|u| pyfmt::truthy(u)) {
            uids.add(pyfmt::str(uid), name.clone());
        }
        if let Some(f) = meta.get("frequency").filter(|f| pyfmt::truthy(f)) {
            freqs.add(pyfmt::str(f), name);
        }
    }
    let duplicate_uids: Vec<NameList> = uids.list.into_iter().filter(|u| u.names.len() > 1).collect();
    let mut rare: Vec<String> = freqs.list.into_iter().filter(|f| f.names.len() == 1).map(|f| f.key).collect();
    rare.sort();
    Analysis {
        total: captures.len(),
        by_type,
        duplicate_uids,
        rare_frequencies: rare,
        semantic_groups: None,
        semantic_unavailable: semantic_reason.to_string(),
    }
}
