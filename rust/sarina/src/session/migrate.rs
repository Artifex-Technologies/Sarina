//! Go: `go/session/migrate.go`: seven retired tool names (folded into task_list, task_update and update_plan) rewritten in
//! a loaded history, so the model does not call them again. Idempotent; anything it does not recognise is left alone.

use super::Session;
use crate::goany::Any;
use crate::gojson;
use crate::gotext;
use std::collections::{BTreeMap, HashSet};

/// The tool that replaced a retired one.
pub fn retired_result_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "task_get" | "task_next" => "task_list",
        "task_start" | "task_complete" | "task_cancel" | "task_block" => "task_update",
        "plan_clear" => "update_plan",
        _ => return None,
    })
}

/// Go's `IsRetiredTool`.
pub fn is_retired_tool(name: &str) -> bool {
    retired_result_name(name).is_some()
}

/// What a migration changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Migration {
    /// assistant tool calls rewritten
    pub calls: i64,
    /// tool result messages renamed
    pub results: i64,
}

impl Migration {
    pub fn changed(&self) -> bool {
        self.calls > 0 || self.results > 0
    }

    pub fn add(&mut self, o: Migration) {
        self.calls += o.calls;
        self.results += o.results;
    }
}

/// The replacement name and arguments for a retired call, or None when it is not retired or its arguments do not read as
/// an object.
pub fn rewrite_retired_call(name: &str, arguments: &str) -> Option<(String, String)> {
    let new_name = retired_result_name(name)?;
    let mut old = BTreeMap::new();
    if !gotext::trim_space(arguments).is_empty() {
        let (v, err) = gojson::unmarshal_any(arguments.as_bytes());
        if err.is_some() {
            return None;
        }
        // null makes Go's map nil, and anything but an object does not read into a map
        old = v.into_object().ok()?;
    }
    let mut out = BTreeMap::new();
    let copy_keys = |out: &mut BTreeMap<String, Any>, keys: &[&str]| {
        for k in keys {
            if let Some(v) = old.get(*k) {
                out.insert((*k).to_owned(), v.clone());
            }
        }
    };
    let task_id = |out: &mut BTreeMap<String, Any>| {
        if let Some(v) = old.get("task_id").or_else(|| old.get("id")) {
            out.insert("task_id".to_owned(), v.clone());
        }
    };
    let status = |out: &mut BTreeMap<String, Any>, s: &str| {
        out.insert("status".to_owned(), Any::String(s.into()));
    };
    match name {
        "task_get" => task_id(&mut out),
        "task_next" => {
            out.insert("actionable_only".into(), Any::Bool(true));
            copy_keys(&mut out, &["max_tasks"]);
        }
        "task_start" => {
            task_id(&mut out);
            status(&mut out, "in_progress");
            copy_keys(&mut out, &["owner", "active_form"]);
        }
        "task_complete" => {
            task_id(&mut out);
            status(&mut out, "completed");
        }
        "task_cancel" => {
            task_id(&mut out);
            status(&mut out, "cancelled");
            copy_keys(&mut out, &["reason"]);
        }
        "task_block" => {
            task_id(&mut out);
            status(&mut out, "blocked");
            copy_keys(&mut out, &["reason"]);
            if let Some(v) = old.get("blocked_by") {
                out.insert("add_blocked_by".into(), v.clone());
            }
        }
        _ => {
            // plan_clear
            out.insert("clear".into(), Any::Bool(true));
            if let Some(Any::Bool(true)) = old.get("sync_tasks") {
                out.insert("prune_tasks".into(), Any::Bool(true));
            }
        }
    }
    let args = gojson::any_json(&Any::Object(out), false).ok()?;
    Some((new_name.to_owned(), args))
}

/// Renames the "tool" field at the head of a serialised tool result body; other content is returned unchanged.
pub fn rename_result_body(content: &str, from: &str, to: &str) -> String {
    for sep in ["{\n  \"tool\": \"", "{\"tool\": \"", "{\"tool\":\""] {
        let prefix = format!("{sep}{from}\"");
        if let Some(rest) = content.strip_prefix(&prefix) {
            return format!("{sep}{to}\"{rest}");
        }
    }
    content.to_owned()
}

fn str_at<'a>(m: &'a BTreeMap<String, Any>, k: &str) -> &'a str {
    match m.get(k) {
        Some(Any::String(s)) => s,
        _ => "",
    }
}

impl Session {
    /// Go's `MigrateRetiredTools`, in place.
    pub fn migrate_retired_tools(&mut self) -> Migration {
        let mut m = Migration::default();
        let mut skipped = HashSet::new(); // ids of retired calls that could not be rewritten
        for msg in &mut self.messages {
            for c in &mut msg.tool_calls {
                if !is_retired_tool(&c.function.name) {
                    continue;
                }
                match rewrite_retired_call(&c.function.name, &c.function.arguments) {
                    Some((n, a)) => {
                        c.function.name = n;
                        c.function.arguments = a;
                        m.calls += 1;
                    }
                    None => {
                        skipped.insert(c.id.clone());
                    }
                }
            }
        }
        for msg in &mut self.messages {
            if msg.role != "tool" || skipped.contains(&msg.tool_call_id) {
                continue;
            }
            let mut changed = false;
            if let Some(to) = retired_result_name(&msg.name) {
                msg.content = rename_result_body(&msg.content, &msg.name, to);
                msg.name = to.to_owned();
                changed = true;
            }
            // Go: a nil metadata map is skipped; an empty one has neither key
            if let Some(to) = retired_result_name(str_at(&msg.metadata, "tool_name")) {
                msg.metadata.insert("tool_name".into(), Any::String(to.into()));
                changed = true;
            }
            let action = str_at(&msg.metadata, "action").to_owned();
            if matches!(action.as_str(), "task_get" | "task_next" | "task_start" | "task_complete" | "task_cancel" | "task_block") {
                let to = retired_result_name(&action).unwrap_or_default();
                msg.metadata.insert("action".into(), Any::String(to.into()));
                changed = true;
            }
            if changed {
                m.results += 1;
            }
        }
        m
    }
}
