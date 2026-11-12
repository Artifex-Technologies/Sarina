//! Go: `go/session/message.go`: the transcript entry, its stored form, the tolerant reader, tool result bodies, pairing
//! repair.

use super::encode_json;
use crate::config::as_int;
use crate::goany::Any;
use crate::gojson::{self, Obj};
use crate::llm::types::{self as llm, usage_from_provider, FunctionCall, Usage, WireToolCall};
use std::collections::BTreeMap;

pub const STATE_FINAL: &str = "final";
pub const STATE_STREAMING: &str = "streaming";
pub const STATE_TOMBSTONED: &str = "tombstoned";

pub const KIND_COMPACT_BOUNDARY: &str = "compact_boundary";
pub const KIND_SNIPPED: &str = "snipped_message";
pub const KIND_CONTINUATION: &str = "continuation_request";
pub const KIND_FILE_HISTORY_REPLAY: &str = "file_history_replay";
pub const KIND_COMPACTION_REPLAY: &str = "compaction_replay";
pub const REMINDER_OPEN: &str = "<system-reminder>";
pub const REMINDER_CLOSE: &str = "</system-reminder>";
const MUTATION_TOTALS_KEY: &str = "mutation_totals";
const LINEAGE_KEY: &str = "lineage_id";
const REVISION_KEY: &str = "revision";

/// A transcript entry: what is sent to the model plus bookkeeping. An empty `metadata` is Go's nil map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub name: String,
    pub tool_call_id: String,
    pub tool_calls: Vec<WireToolCall>,
    pub message_id: String,
    pub state: String,
    pub stop_reason: String,
    pub usage: Usage,
    pub metadata: BTreeMap<String, Any>,
}

impl Message {
    /// The exact shape sent to the backend.
    pub fn wire(&self) -> llm::Message {
        llm::Message {
            role: self.role.clone(),
            content: self.content.clone(),
            name: self.name.clone(),
            tool_call_id: self.tool_call_id.clone(),
            tool_calls: self.tool_calls.clone(),
        }
    }

    /// Metadata "kind", or "".
    pub fn kind(&self) -> &str {
        match self.metadata.get("kind") {
            Some(Any::String(s)) => s,
            _ => "",
        }
    }

    /// Whether the message is a `<system-reminder>` envelope.
    pub fn is_reminder(&self) -> bool {
        self.role == "user" && self.content.starts_with(REMINDER_OPEN)
    }

    pub fn set_meta(&mut self, key: &str, v: Any) {
        self.metadata.insert(key.to_owned(), v);
    }

    /// Go's `Mutate`: records an in-place change (lineage id, revision, per-kind totals). The counts are Go ints.
    pub fn mutate(&mut self, kind: &str) {
        if !self.metadata.contains_key(LINEAGE_KEY) {
            let id = if self.message_id.is_empty() { format!("{}_lineage", self.role) } else { self.message_id.clone() };
            self.metadata.insert(LINEAGE_KEY.into(), Any::String(id));
        }
        let rev = as_int(self.metadata.get(REVISION_KEY)).unwrap_or(0);
        self.metadata.insert(REVISION_KEY.into(), Any::Int(rev.wrapping_add(1)));
        let mut totals = match self.metadata.remove(MUTATION_TOTALS_KEY) {
            Some(t) => t.into_object().unwrap_or_default(),
            None => BTreeMap::new(),
        };
        let n = as_int(totals.get(kind)).unwrap_or(0);
        totals.insert(kind.to_owned(), Any::Int(n.wrapping_add(1)));
        self.metadata.insert(MUTATION_TOTALS_KEY.into(), Any::Object(totals));
    }

    /// The structured mirror of the message as Go's `Blocks` builds it (maps, so keys sorted), written as JSON.
    fn blocks_json(&self) -> String {
        let mut blocks = Vec::new();
        if self.role == "tool" {
            if !self.content.is_empty() {
                blocks.push(
                    Obj::new(false)
                        .str("name", &self.name)
                        .str("text", &self.content)
                        .str("tool_call_id", &self.tool_call_id)
                        .str("type", "tool_result")
                        .end(),
                );
            }
        } else {
            if !self.content.is_empty() {
                blocks.push(Obj::new(false).str("text", &self.content).str("type", "text").end());
            }
            for c in &self.tool_calls {
                blocks.push(
                    Obj::new(false)
                        .str("arguments", &c.function.arguments)
                        .str("id", &c.id)
                        .str("name", &c.function.name)
                        .str("type", "tool_call")
                        .end(),
                );
            }
        }
        let parts: Vec<String> = blocks.into_iter().map(|b| b.unwrap_or_default()).collect();
        format!("[{}]", parts.join(","))
    }

    /// Go's `MarshalJSON`: the stored form (the sent shape plus bookkeeping), without HTML escaping.
    pub fn to_json(&self) -> Result<String, gojson::Error> {
        let o = Obj::new(false)
            .str("role", &self.role)
            .str("content", &self.content)
            .str_omit("name", &self.name)
            .str_omit("tool_call_id", &self.tool_call_id);
        let o = if self.tool_calls.is_empty() { o } else { o.raw("tool_calls", &llm::tool_calls_json(&self.tool_calls, false)) };
        let state = if self.state == STATE_FINAL { "" } else { self.state.as_str() };
        let o = o.raw("blocks", &self.blocks_json()).str_omit("message_id", &self.message_id).str_omit("state", state);
        let o = o.str_omit("stop_reason", &self.stop_reason);
        let o = if self.usage.total() != 0 { o.raw("usage", &self.usage.to_json()) } else { o };
        let o = if self.metadata.is_empty() { o } else { o.with("metadata", gojson::any_json(&Any::Object(self.metadata.clone()), false)) };
        o.end()
    }
}

/// Go's `MessageFromMap`: a stored message read tolerantly.
pub fn message_from_map(m: &BTreeMap<String, Any>) -> Message {
    let s = |k: &str| match m.get(k) {
        Some(Any::String(s)) => s.clone(),
        _ => String::new(),
    };
    let mut msg = Message {
        role: s("role"),
        content: s("content"),
        name: s("name"),
        tool_call_id: s("tool_call_id"),
        message_id: s("message_id"),
        state: s("state"),
        stop_reason: s("stop_reason"),
        ..Message::default()
    };
    if msg.state.is_empty() {
        msg.state = STATE_FINAL.into();
    }
    if let Some(Any::Array(list)) = m.get("tool_calls") {
        for item in list {
            let Any::Object(tm) = item else { continue };
            let fs = |map: Option<&Any>, k: &str| match map {
                Some(Any::Object(f)) => match f.get(k) {
                    Some(Any::String(s)) => s.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            let id = match tm.get("id") {
                Some(Any::String(s)) => s.clone(),
                _ => String::new(),
            };
            msg.tool_calls.push(WireToolCall {
                id,
                kind: "function".into(),
                function: FunctionCall { name: fs(tm.get("function"), "name"), arguments: fs(tm.get("function"), "arguments") },
            });
        }
    }
    if let Some(u @ Any::Object(_)) = m.get("usage") {
        msg.usage = usage_from_provider(Some(u));
    }
    if let Some(Any::Object(md)) = m.get("metadata") {
        msg.metadata = md.clone();
    }
    msg
}

/// Go's `ToolResultBody`: a tool result the way the model sees it; metadata that cannot be written is dropped.
pub fn tool_result_body(tool: &str, ok: bool, content: &str, metadata: &BTreeMap<String, Any>) -> String {
    let body = |md: Option<&BTreeMap<String, Any>>| {
        let o = Obj::new(false).str("tool", tool).bool("ok", ok).str("content", content);
        let o = match md {
            Some(m) if !m.is_empty() => o.with("metadata", gojson::any_json(&Any::Object(m.clone()), false)),
            _ => o,
        };
        o.end()
    };
    match body(Some(metadata)) {
        Ok(s) => encode_json(&s),
        Err(_) => encode_json(&body(None).unwrap_or_default()),
    }
}

/// Go's `NotRunMessage`: the answer to a tool call that was deliberately not run.
pub fn not_run_message(id: &str, name: &str, reason: &str, message_id: &str) -> Message {
    let s = |v: &str| Any::String(v.to_owned());
    Message {
        role: "tool".into(),
        name: name.into(),
        tool_call_id: id.into(),
        message_id: message_id.into(),
        state: STATE_FINAL.into(),
        stop_reason: "tool_not_run".into(),
        content: tool_result_body(name, false, reason, &BTreeMap::from([("error_kind".to_owned(), s("not_run"))])),
        metadata: BTreeMap::from([
            ("error_kind".to_owned(), s("not_run")),
            ("tool_name".to_owned(), s(name)),
            ("tool_call_id".to_owned(), s(id)),
            ("phase".to_owned(), s("completed")),
        ]),
        ..Message::default()
    }
}

/// Go's `RepairPairing`: every assistant tool call gets a result right after its siblings (a "not run" one if missing),
/// tool messages with no matching call are dropped.
pub fn repair_pairing(msgs: &[Message]) -> Vec<Message> {
    let mut out = Vec::with_capacity(msgs.len());
    let mut i = 0;
    while i < msgs.len() {
        let m = &msgs[i];
        if m.role == "tool" {
            i += 1;
            continue;
        }
        out.push(m.clone());
        if m.role != "assistant" || m.tool_calls.is_empty() {
            i += 1;
            continue;
        }
        let want: std::collections::HashSet<&str> = m.tool_calls.iter().map(|c| c.id.as_str()).collect();
        let mut answered = std::collections::HashSet::new();
        let mut j = i + 1;
        while j < msgs.len() && msgs[j].role == "tool" {
            let id = msgs[j].tool_call_id.as_str();
            if want.contains(id) && answered.insert(id) {
                out.push(msgs[j].clone());
            }
            j += 1;
        }
        for c in &m.tool_calls {
            if !answered.contains(c.id.as_str()) {
                out.push(not_run_message(
                    &c.id,
                    &c.function.name,
                    "this call has no recorded result (the session stopped before it ran)",
                    &format!("tool_repair_{}", c.id),
                ));
            }
        }
        i = j;
    }
    out
}

/// A list of messages as Go's `EncodeJSON(msgs)` writes it.
pub fn messages_json(msgs: &[Message]) -> Result<String, gojson::Error> {
    let mut parts = Vec::with_capacity(msgs.len());
    for m in msgs {
        parts.push(m.to_json()?);
    }
    Ok(format!("[{}]", parts.join(",")))
}
