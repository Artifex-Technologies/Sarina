//! Go: `go/session`: the message model, the secret redactor, and the session file Heimdall reads (`session.go`,
//! `message.go`, `migrate.go`, `plan.go`, `redact.go`). The file is written byte for byte as Go writes it and read with
//! Go's tolerance (testdata/parity/session.json). Saving to disk (`Save`, the atomic write, `MigrateDir`) comes with the
//! agent loop.

pub mod message;
pub mod migrate;
pub mod redact;

pub use message::Message;

use crate::config::{self, as_float, as_int, as_map, ModelConfig, RuntimeConfig};
use crate::goany::Any;
use crate::gojson::{self, Obj, Value};
use crate::gotext;
use crate::llm::types::{usage_from_provider, Usage};
use std::collections::BTreeMap;

/// A string map that keeps insertion order across a JSON round trip (Go's `OrderedMap`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrderedMap(pub Vec<(String, String)>);

impl OrderedMap {
    /// Replaces or appends a key.
    pub fn set(&mut self, k: &str, v: &str) {
        match self.0.iter_mut().find(|(key, _)| key == k) {
            Some(kv) => kv.1 = v.to_owned(),
            None => self.0.push((k.to_owned(), v.to_owned())),
        }
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
    }

    /// Go's `MarshalJSON`: an object in insertion order, without HTML escaping.
    pub fn to_json(&self) -> String {
        let mut out = String::from("{");
        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            gojson::write_str(&mut out, k, false);
            out.push(':');
            gojson::write_str(&mut out, v, false);
        }
        out.push('}');
        out
    }

    /// Go's `UnmarshalJSON` (a token stream): anything but an object is empty; each member in order, duplicates kept, a
    /// value that is not a string as `fmt.Sprint` writes it; a value that does not decode (a number out of range) ends the
    /// map there.
    pub fn from_value(v: &Value) -> OrderedMap {
        let mut out = OrderedMap::default();
        let Value::Object(members) = v else { return out };
        for (k, val) in members {
            let (a, err) = gojson::to_any(val);
            if err.is_some() {
                break;
            }
            let s = match a {
                Any::String(ref s) => s.clone(),
                ref other => sprint(other),
            };
            out.0.push((k.clone(), s));
        }
        out
    }
}

/// Go's `fmt.Sprint` of a decoded value.
pub fn sprint(v: &Any) -> String {
    match v {
        Any::Null => "<nil>".into(),
        Any::Bool(b) => b.to_string(),
        Any::Number(f) => gotext::float_v(*f),
        Any::Int(i) => i.to_string(),
        Any::Num(n) => n.clone(),
        Any::String(s) => s.clone(),
        Any::Array(items) => format!("[{}]", items.iter().map(sprint).collect::<Vec<_>>().join(" ")),
        Any::Object(m) => format!("map[{}]", m.iter().map(|(k, x)| format!("{k}:{}", sprint(x))).collect::<Vec<_>>().join(" ")),
    }
}

/// The persisted counters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BudgetState {
    pub model_calls: i64,
    pub session_turns: i64,
    pub tool_calls: i64,
    pub delegated_tasks: i64,
    pub escalations: i64,
}

/// One conversation and everything saved with it. Empty lists and maps stand for Go's nil ones (the file writes them the
/// same way).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub model: ModelConfig,
    pub runtime: RuntimeConfig,
    pub system_prompt_parts: Vec<String>,
    pub user_context: OrderedMap,
    pub system_context: OrderedMap,
    pub messages: Vec<Message>,
    pub turns: i64,
    pub tool_calls: i64,
    pub usage: Usage,
    pub total_cost_usd: f64,
    pub file_history: Vec<BTreeMap<String, Any>>,
    pub budget_state: BudgetState,
    pub plugin_state: BTreeMap<String, Any>,
    /// "" is written as null
    pub scratchpad_directory: String,
}

/// Go's `EncodeJSON` of a value already written compactly without HTML escaping: two-space indent, everything past ASCII
/// as `\u` escapes, no trailing newline.
pub fn encode_json(compact: &str) -> String {
    gojson::ascii_only(&gojson::indent(compact, "  "))
}

impl Session {
    /// "<prefix>_<index of the next message>".
    pub fn next_message_id(&self, prefix: &str) -> String {
        format!("{prefix}_{}", self.messages.len())
    }

    /// Adds a message (state "" becomes final) and returns its index.
    pub fn append(&mut self, mut m: Message) -> usize {
        if m.state.is_empty() {
            m.state = message::STATE_FINAL.into();
        }
        self.messages.push(m);
        self.messages.len() - 1
    }

    /// Go's `Marshal`: the session file. The API key is never written. A float Go cannot write (a cost of NaN read from a
    /// damaged file) is Go's error, and nothing is written.
    pub fn marshal(&self) -> Result<String, gojson::Error> {
        let rt = &self.runtime; // Normalized: the lists are never nil here
        let mut msgs = Vec::with_capacity(self.messages.len());
        for m in &self.messages {
            msgs.push(m.to_json()?);
        }
        let mut history = Vec::with_capacity(self.file_history.len());
        for h in &self.file_history {
            history.push(gojson::any_json(&Any::Object(h.clone()), false)?);
        }
        let b = &self.budget_state;
        let budget = Obj::new(false)
            .int("model_calls", b.model_calls)
            .int("session_turns", b.session_turns)
            .int("tool_calls", b.tool_calls)
            .int("delegated_tasks", b.delegated_tasks)
            .int("escalations", b.escalations)
            .end();
        let scratch = if self.scratchpad_directory.is_empty() {
            "null".to_owned()
        } else {
            let mut s = String::new();
            gojson::write_str(&mut s, &self.scratchpad_directory, false);
            s
        };
        let compact = Obj::new(false)
            .str("session_id", &self.id)
            .with("model_config", self.model.to_json(false))
            .with("runtime_config", rt.to_json(false))
            .strs("system_prompt_parts", &self.system_prompt_parts)
            .raw("user_context", &self.user_context.to_json())
            .raw("system_context", &self.system_context.to_json())
            .raw("messages", &format!("[{}]", msgs.join(",")))
            .int("turns", self.turns)
            .int("tool_calls", self.tool_calls)
            .raw("usage", &self.usage.to_json())
            .f64("total_cost_usd", self.total_cost_usd)
            .raw("file_history", &format!("[{}]", history.join(",")))
            .with("budget_state", budget)
            .with("plugin_state", gojson::any_json(&Any::Object(self.plugin_state.clone()), false))
            .raw("scratchpad_directory", &scratch)
            .end()?;
        Ok(encode_json(&compact))
    }

    /// The plan-mode state; a missing or malformed entry is normal mode.
    pub fn plan(&self) -> PlanState {
        let Some(Any::Object(m)) = self.plugin_state.get(PLAN_KEY) else {
            return PlanState { mode: PLAN_NORMAL.into(), ..PlanState::default() };
        };
        let s = |k: &str| match m.get(k) {
            Some(Any::String(s)) => s.clone(),
            _ => String::new(),
        };
        let mode = if s("mode") == PLAN_ACTIVE { PLAN_ACTIVE } else { PLAN_NORMAL };
        PlanState { mode: mode.into(), pending: s("pending"), note: s("note") }
    }

    /// Stores the plan state (normal mode with nothing pending leaves no entry).
    pub fn set_plan(&mut self, p: &PlanState) {
        if p.mode != PLAN_ACTIVE && p.pending.is_empty() && p.note.is_empty() {
            self.plugin_state.remove(PLAN_KEY);
            return;
        }
        let mode = if p.mode == PLAN_ACTIVE { PLAN_ACTIVE } else { PLAN_NORMAL };
        let mut m = BTreeMap::from([("mode".to_owned(), Any::String(mode.into()))]);
        if !p.pending.is_empty() {
            m.insert("pending".into(), Any::String(p.pending.clone()));
        }
        if !p.note.is_empty() {
            m.insert("note".into(), Any::String(p.note.clone()));
        }
        self.plugin_state.insert(PLAN_KEY.into(), Any::Object(m));
    }
}

pub const PLAN_NORMAL: &str = "normal";
pub const PLAN_ACTIVE: &str = "plan";
const PLAN_KEY: &str = "plan_mode";

/// A session's plan mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanState {
    pub mode: String,
    pub pending: String,
    pub note: String,
}

impl PlanState {
    pub fn active(&self) -> bool {
        self.mode == PLAN_ACTIVE
    }
}

/// Go's `Path`: `<dir>/<id>.json`.
pub fn path(dir: &str, id: &str) -> String {
    gotext::lossy(&crate::gopath::join(&[dir.as_bytes(), format!("{id}.json").as_bytes()]))
}

/// Go's `FileMode`: 0600 unless SARINA_SESSION_FILE_MODE says 0640 or 0644.
pub fn file_mode(getenv: &dyn Fn(&str) -> String) -> u32 {
    match gotext::trim_space(&getenv("SARINA_SESSION_FILE_MODE")) {
        "0640" | "640" => 0o640,
        "0644" | "644" => 0o644,
        _ => 0o600,
    }
}

/// Go's `Parse`: a session file's bytes, with retired tool names in the history rewritten (in memory).
pub fn parse(b: &[u8]) -> Result<Session, String> {
    let mut s = parse_raw(b)?;
    s.migrate_retired_tools();
    Ok(s)
}

/// Go's `parse`: the file read without migrating anything.
pub fn parse_raw(b: &[u8]) -> Result<Session, String> {
    let doc = gojson::parse(b).map_err(|e| format!("session file is not a JSON object: {e}"))?;
    let members: &[(String, Value)] = match &doc {
        Value::Object(m) => m,
        Value::Null => &[],
        other => {
            let kind = match other {
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                _ => "array",
            };
            return Err(format!(
                "session file is not a JSON object: json: cannot unmarshal {kind} into Go value of type map[string]jsontext.Value"
            ));
        }
    };
    // a map: the last of duplicate keys wins
    let raw = |key: &str| members.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v);
    for key in [
        "session_id",
        "model_config",
        "runtime_config",
        "system_prompt_parts",
        "user_context",
        "system_context",
        "messages",
        "turns",
        "tool_calls",
    ] {
        if raw(key).is_none() {
            return Err(format!("session file is missing required key {}", gotext::quote(key)));
        }
    }
    let generic = |key: &str| raw(key).map_or(Any::Null, |v| gojson::to_any(v).0);
    let mut s = Session::default();
    match &generic("session_id") {
        Any::String(id) if !id.is_empty() => s.id.clone_from(id),
        _ => return Err("session_id must be a non-empty string".into()),
    }
    s.model = config::model_from_map(as_map(Some(&generic("model_config")))).ok_or("model_config.model is required")?;
    s.runtime = config::runtime_from_map(as_map(Some(&generic("runtime_config")))).ok_or("runtime_config.cwd is required")?;
    match &generic("system_prompt_parts") {
        Any::Array(parts) => {
            s.system_prompt_parts = parts
                .iter()
                .filter_map(|p| match p {
                    Any::String(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        }
        _ => return Err("system_prompt_parts must be a list".into()),
    }
    s.user_context = raw("user_context").map(OrderedMap::from_value).unwrap_or_default();
    s.system_context = raw("system_context").map(OrderedMap::from_value).unwrap_or_default();
    match &generic("messages") {
        Any::Array(msgs) => {
            for m in msgs {
                if let Any::Object(mm) = m {
                    s.messages.push(message::message_from_map(mm));
                }
            }
        }
        _ => return Err("messages must be a list".into()),
    }
    s.turns = as_int(Some(&generic("turns"))).unwrap_or(0);
    s.tool_calls = as_int(Some(&generic("tool_calls"))).unwrap_or(0);
    s.usage = usage_from_provider(Some(&generic("usage")));
    s.total_cost_usd = as_float(Some(&generic("total_cost_usd"))).unwrap_or(0.0);
    if let Any::Array(fh) = &generic("file_history") {
        for e in fh {
            if let Any::Object(em) = e {
                s.file_history.push(em.clone());
            }
        }
    }
    let bs_any = generic("budget_state");
    let bs = as_map(Some(&bs_any));
    let geti = |k: &str| as_int(bs.get(k)).unwrap_or(0);
    s.budget_state = BudgetState {
        model_calls: geti("model_calls"),
        session_turns: geti("session_turns"),
        tool_calls: geti("tool_calls"),
        delegated_tasks: geti("delegated_tasks"),
        escalations: geti("escalations"),
    };
    if let Ok(ps) = generic("plugin_state").into_object() {
        s.plugin_state = ps;
    }
    if let Any::String(sd) = &generic("scratchpad_directory") {
        s.scratchpad_directory.clone_from(sd);
    }
    Ok(s)
}
