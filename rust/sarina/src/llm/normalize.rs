//! Go: `llm/normalize.go`: a model turn cleaned of local servers' quirks, in place: inline `<think>` blocks move from the
//! content to the reasoning; when the server returned no structured tool calls, calls written as text (`<tool_call>`
//! blocks, `<|python_tag|>`, bare JSON) for a known tool become real calls; arguments cut off by a length stop are
//! refused; tool-call ids are made non-empty and unique.

use super::repair::{parse_arguments, repair_json, strip_fences};
use super::types::{ToolCall, Turn};
use crate::goany::Any;
use crate::gotext;
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

/// Go's `Normalize`.
pub fn normalize(t: &mut Turn, tool_names: &[String]) {
    let (vis, reasoning) = split_think(&t.content);
    if !reasoning.is_empty() || vis != t.content {
        t.content = vis;
        if !reasoning.is_empty() {
            if !t.reasoning.is_empty() {
                t.reasoning.push('\n');
            }
            t.reasoning.push_str(&reasoning);
        }
    }
    if t.tool_calls.is_empty() && !tool_names.is_empty() {
        let known: HashSet<&str> = tool_names.iter().map(String::as_str).collect();
        let (calls, rest) = extract_embedded(&t.content, &known);
        if !calls.is_empty() {
            t.tool_calls = calls;
            t.content = rest;
        }
    }
    let truncated_stop = t.finish_reason == "length" || t.finish_reason == "max_tokens";
    for c in &mut t.tool_calls {
        if c.truncated && truncated_stop && c.parse_error.is_empty() {
            c.parse_error = "arguments were cut off by the output length limit".into();
            c.arguments = Some(BTreeMap::new());
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    for (i, c) in t.tool_calls.iter_mut().enumerate() {
        let mut id = c.id.clone();
        if id.is_empty() || seen.contains(&id) {
            id = format!("call_{i}");
            let mut n = 1;
            while seen.contains(&id) {
                id = format!("call_{i}_{n}");
                n += 1;
            }
        }
        c.id.clone_from(&id);
        seen.insert(id);
    }
}

/// Go's `splitThink`: the visible text and the reasoning (both trimmed); a stray closing tag ends reasoning that started
/// before the text.
pub fn split_think(s: &str) -> (String, String) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    if !s.contains("think>") {
        return (s.to_string(), String::new());
    }
    let (mut vis, mut rea) = (String::new(), String::new());
    let mut s = s;
    loop {
        let o = s.find(OPEN);
        let c = s.find(CLOSE);
        if let Some(c) = c {
            if o.is_none_or(|o| c < o) {
                rea.push_str(&s[..c]);
                s = &s[c + CLOSE.len()..];
                continue;
            }
        }
        let Some(o) = o else { break };
        vis.push_str(&s[..o]);
        let rest = &s[o + OPEN.len()..];
        match rest.find(CLOSE) {
            None => {
                rea.push_str(rest);
                s = "";
                break;
            }
            Some(c2) => {
                rea.push_str(&rest[..c2]);
                s = &rest[c2 + CLOSE.len()..];
            }
        }
    }
    vis.push_str(s);
    (gotext::trim_space(&vis).to_string(), gotext::trim_space(&rea).to_string())
}

fn tool_call_block() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s)<tool_call>(.*?)(?:</tool_call>|$)").expect("the pattern compiles"))
}

fn extract_embedded(content: &str, known: &HashSet<&str>) -> (Vec<ToolCall>, String) {
    let mut calls = Vec::new();
    let mut rest = content.to_string();
    // 1. <tool_call>{...}</tool_call> blocks (Qwen / Hermes style)
    if rest.contains("<tool_call>") {
        let re = tool_call_block();
        rest = re
            .replace_all(&rest, |caps: &regex::Captures| {
                let block = &caps[0];
                // Go matches the block again on its own: the first group of that match
                let inner = re.captures(block).and_then(|c| c.get(1)).map_or("", |m| m.as_str());
                match call_from_text(inner, known) {
                    Some(c) => {
                        calls.push(c);
                        String::new()
                    }
                    None => block.to_string(),
                }
            })
            .into_owned();
    }
    // 2. Llama's python tag: everything after it is the call
    if calls.is_empty() {
        if let Some(idx) = rest.find("<|python_tag|>") {
            let mut body = &rest[idx + "<|python_tag|>".len()..];
            for stop in ["<|eom_id|>", "<|eot_id|>"] {
                let t = gotext::trim_space(body);
                body = t.strip_suffix(stop).unwrap_or(t);
            }
            if let Some(c) = call_from_text(body, known) {
                calls.push(c);
                rest.truncate(idx);
            }
        }
    }
    // 3. the whole message is a JSON call (or an array of calls)
    if calls.is_empty() {
        let trimmed = strip_fences(&rest);
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let (Some(v), _, true) = repair_json(trimmed) {
                let objs = match v.into_array() {
                    Ok(a) => a,
                    Err(v) => vec![v],
                };
                let mut found = Vec::new();
                for o in objs {
                    let Ok(m) = o.into_object() else {
                        found.clear();
                        break;
                    };
                    let Some(c) = call_from_object(&m, known) else {
                        found.clear();
                        break;
                    };
                    found.push(c);
                }
                if !found.is_empty() {
                    calls = found;
                    rest.clear();
                }
            }
        }
    }
    let rest = gotext::trim_space(&rest).to_string();
    (calls, rest)
}

/// A value Go's code sees as not nil (a JSON null is nil).
fn present(x: Option<&Any>) -> Option<&Any> {
    x.filter(|a| !matches!(a, Any::Null))
}

fn call_from_text(text: &str, known: &HashSet<&str>) -> Option<ToolCall> {
    let (v, _, ok) = repair_json(text);
    if !ok {
        return None;
    }
    let m = v?.into_object().ok()?;
    call_from_object(&m, known)
}

fn call_from_object(m: &BTreeMap<String, Any>, known: &HashSet<&str>) -> Option<ToolCall> {
    let src = match m.get("function") {
        Some(Any::Object(f)) => f,
        _ => m,
    };
    let s = |x: Option<&Any>| match x {
        Some(Any::String(v)) => v.clone(),
        _ => String::new(),
    };
    let mut name = s(src.get("name"));
    if name.is_empty() {
        name = s(m.get("name"));
    }
    if !known.contains(name.as_str()) {
        return None;
    }
    let mut c = ToolCall { name, arguments: Some(BTreeMap::new()), ..ToolCall::default() };
    // Go: the first of these that is not nil (a JSON null is nil too)
    let args = present(src.get("arguments"))
        .or_else(|| present(src.get("parameters")))
        .or_else(|| present(m.get("arguments")))
        .or_else(|| present(m.get("parameters")));
    match args {
        None => {}
        Some(Any::Object(a)) => c.arguments = Some(a.clone()),
        Some(Any::String(a)) => {
            let (parsed, trunc, err) = parse_arguments(a);
            c.raw_arguments.clone_from(a);
            if !err.is_empty() {
                c.parse_error = err.into();
            } else {
                c.arguments = parsed;
                c.truncated = trunc;
            }
        }
        Some(_) => c.parse_error = "arguments must be a JSON object".into(),
    }
    Some(c)
}
