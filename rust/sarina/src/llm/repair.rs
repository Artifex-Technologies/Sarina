//! Go: `llm/repair.go`: JSON as small local models write it, read anyway: code fences, trailing commas, single-quoted
//! strings, raw newlines inside strings, and output cut off mid-structure (closed again, and reported as truncated).

use crate::goany::Any;
use crate::gojson;
use crate::gotext;
use std::collections::BTreeMap;

/// Go's `decodeFirst`: the first JSON value in `s` (one `json.Decoder.Decode`), what follows ignored.
fn decode_first(s: &str) -> Option<Any> {
    match gojson::parse_first(s.as_bytes()).0 {
        Ok(v) => match gojson::to_any(&v) {
            (a, None) => Some(a),
            _ => None,
        },
        Err(_) => None,
    }
}

/// Go's `stripFences`: a Markdown code fence around the text taken off (its first line, and a closing fence).
pub fn strip_fences(s: &str) -> &str {
    let mut s = gotext::trim_space(s);
    if s.starts_with("```") {
        s = match s.find('\n') {
            Some(nl) => &s[nl + 1..],
            None => &s[3..],
        };
        let t = gotext::trim_space(s);
        s = gotext::trim_space(t.strip_suffix("```").unwrap_or(t));
    }
    s
}

/// Go's `RepairJSON`: the value, whether closing characters had to be added (the input was cut off), and whether it read.
pub fn repair_json(s: &str) -> (Option<Any>, bool, bool) {
    let s = strip_fences(s);
    if let Some(v) = decode_first(s) {
        return (Some(v), false, true);
    }
    let (fixed, trunc) = repair_text(s);
    if let Some(v) = decode_first(&fixed) {
        return (Some(v), trunc, true);
    }
    (None, false, false)
}

fn trim_comma(out: &mut Vec<u8>) {
    let mut i = out.len();
    while i > 0 && matches!(out[i - 1], b' ' | b'\n' | b'\t' | b'\r') {
        i -= 1;
    }
    if i > 0 && out[i - 1] == b',' {
        out.truncate(i - 1);
    }
}

fn esc_ctl(out: &mut Vec<u8>, c: u8) -> bool {
    match c {
        b'\n' => out.extend_from_slice(b"\\n"),
        b'\r' => out.extend_from_slice(b"\\r"),
        b'\t' => out.extend_from_slice(b"\\t"),
        _ => return false,
    }
    true
}

/// Go's `repairText`: one pass over the bytes that fixes quotes, control characters and trailing commas and closes what
/// is open.
pub fn repair_text(s: &str) -> (String, bool) {
    let s = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(s.len() + 8);
    let mut stack: Vec<u8> = Vec::new();
    let (mut in_double, mut in_single) = (false, false);
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if in_double {
            if c == b'\\' && i + 1 < s.len() {
                out.extend_from_slice(&[c, s[i + 1]]);
                i += 1;
            } else if c == b'"' {
                in_double = false;
                out.push(c);
            } else if !esc_ctl(&mut out, c) {
                out.push(c);
            }
        } else if in_single {
            if c == b'\\' && i + 1 < s.len() {
                if s[i + 1] == b'\'' {
                    out.push(b'\'');
                } else {
                    out.extend_from_slice(&[c, s[i + 1]]);
                }
                i += 1;
            } else if c == b'\'' {
                in_single = false;
                out.push(b'"');
            } else if c == b'"' {
                out.extend_from_slice(b"\\\"");
            } else if !esc_ctl(&mut out, c) {
                out.push(c);
            }
        } else {
            match c {
                b'"' => {
                    in_double = true;
                    out.push(c);
                }
                b'\'' => {
                    in_single = true;
                    out.push(b'"');
                }
                b'{' => {
                    stack.push(b'}');
                    out.push(c);
                }
                b'[' => {
                    stack.push(b']');
                    out.push(c);
                }
                b'}' | b']' => {
                    trim_comma(&mut out);
                    if stack.last() == Some(&c) {
                        stack.pop();
                    }
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
        i += 1;
    }
    let mut truncated = false;
    if in_double || in_single {
        out.push(b'"');
        truncated = true;
    }
    if !stack.is_empty() {
        truncated = true;
        while out.last().is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n')) {
            out.pop();
        }
        trim_comma(&mut out);
        if out.last() == Some(&b':') {
            out.extend_from_slice(b"null");
        }
        out.extend(stack.iter().rev());
    }
    // only ASCII was added or taken away whole, so the text stays UTF-8
    (String::from_utf8(out).unwrap_or_else(|e| gotext::lossy(e.as_bytes())), truncated)
}

/// Go's `ParseArguments`: an arguments string as an object (empty is `{}`); the error text when it is not one.
pub fn parse_arguments(raw: &str) -> (Option<BTreeMap<String, Any>>, bool, &'static str) {
    if gotext::trim_space(raw).is_empty() {
        return (Some(BTreeMap::new()), false, "");
    }
    let (v, trunc, ok) = repair_json(raw);
    if !ok {
        return (None, false, "arguments are not valid JSON");
    }
    match v.map(Any::into_object) {
        Some(Ok(m)) => (Some(m), trunc, ""),
        _ => (None, false, "arguments must be a JSON object"),
    }
}
