//! testdata/parity/session.json: Go's session file (go/session with go/config's loaders and go/llm's types): made-up files
//! read and written back, before and after the retired-tool migration, with the plan state, the repaired history and each
//! message's wire form; Redact, ToolResultBody, NotRunMessage, SetPlan, Mutate, FileMode.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::gotext::lossy;
use sarina::session::{self, message, migrate, redact, PlanState, Session};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn fnv(b: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn digest(b: &[u8]) -> Value {
    json!({"fnv": fnv(b), "len": b.len()})
}

/// A text as the fixture keeps it: whole, or its hash when long (the base file's answers are kept whole).
fn text(s: &str, full: bool) -> Value {
    if full || s.len() < 1200 {
        common::text(s.as_bytes())
    } else {
        digest(s.as_bytes())
    }
}

/// The fixture's typed value (parity.Typed) back to an `any`.
fn untyped(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Array(a) => Any::Array(a.iter().map(untyped).collect()),
        Value::Object(m) if m.contains_key("f") => Any::Number(f64::from_bits(u64::from_str_radix(m["f"].as_str().unwrap(), 16).unwrap())),
        Value::Object(m) if m.contains_key("i") => Any::Int(m["i"].as_str().unwrap().parse().unwrap()),
        Value::Object(m) if m.contains_key("s") => Any::String(lossy(&common::bytes(&m["s"]))),
        Value::Object(m) => Any::Object(m["o"].as_object().unwrap().iter().map(|(k, x)| (k.clone(), untyped(x))).collect()),
        other => panic!("{other}"),
    }
}

/// Go's json.Marshal of a json.RawMessage: compacted, with HTML escaping.
fn compact_raw(raw: &str) -> String {
    let mut out = String::new();
    let mut in_str = false;
    let mut escaped = false;
    for c in raw.chars() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            out.push(c);
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if !c.is_ascii_whitespace() {
            out.push(c);
        }
    }
    gojson::escape_html(&out)
}

/// The test's edit: the made-up file with the value at `path` set to `raw` (removed when empty), as Go's edit writes it.
fn edit(base: &Any, path: &[String], raw: &str) -> String {
    let mut doc = base.clone();
    let mut m = match &mut doc {
        Any::Object(m) => m,
        _ => unreachable!(),
    };
    for k in &path[..path.len() - 1] {
        if !matches!(m.get(k), Some(Any::Object(_))) {
            m.insert(k.clone(), Any::Object(BTreeMap::new()));
        }
        m = match m.get_mut(k) {
            Some(Any::Object(next)) => next,
            _ => unreachable!(),
        };
    }
    let last = path.last().unwrap();
    if raw.is_empty() {
        m.remove(last);
    } else {
        m.insert(last.clone(), Any::Num(compact_raw(raw))); // written as it stands
    }
    gojson::marshal_any(&doc).unwrap()
}

fn session_out(b: &[u8], full: bool) -> Value {
    let mut s: Session = match session::parse_raw(b) {
        Ok(s) => s,
        Err(e) => return json!({"err": e}),
    };
    let marshal = |s: &Session| match s.marshal() {
        Ok(t) => text(&t, full),
        Err(e) => json!({"err": e.0}),
    };
    let mut out = serde_json::Map::new();
    out.insert("file".into(), marshal(&s));
    let mig = s.migrate_retired_tools();
    out.insert("migration".into(), json!([mig.calls, mig.results]));
    if mig.changed() {
        out.insert("migrated".into(), marshal(&s));
    }
    let p = s.plan();
    out.insert("plan".into(), json!([p.mode, common::text(p.pending.as_bytes()), common::text(p.note.as_bytes()), p.active()]));
    let repaired = message::messages_json(&message::repair_pairing(&s.messages)).map(|c| session::encode_json(&c)).unwrap_or_default();
    out.insert("repaired".into(), text(&repaired, full));
    let mut list = Vec::new();
    let mut joined = Vec::new();
    for m in &s.messages {
        let wire = m.wire().to_json(true);
        list.push(json!([common::text(m.kind().as_bytes()), m.is_reminder(), common::text(wire.as_bytes())]));
        joined.extend_from_slice(format!("{}\x00{}\x00{}\x01", m.kind(), if m.is_reminder() { "1" } else { "0" }, wire).as_bytes());
    }
    out.insert("messages".into(), if full { Value::Array(list) } else { digest(&joined) });
    Value::Object(out)
}

#[test]
fn session_matches_go() {
    let mut check = common::Check::new();
    let cases = common::cases("session");
    let base_file = common::bytes(&cases[0]["in"]["file"]);
    assert_eq!(cases[0]["in"]["what"], "base");
    let base_doc = gojson::unmarshal_any(&base_file).0;
    for c in &cases {
        let input = &c["in"];
        match c["fn"].as_str().unwrap() {
            "parse" => {
                let (doc, full) = if let Some(path) = input.get("edit") {
                    let path: Vec<String> = path.as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_owned()).collect();
                    let d = edit(&base_doc, &path, input["raw"].as_str().unwrap());
                    check.eq(format!("edit {path:?} {}", input["raw"]), &digest(d.as_bytes()), &input["doc"]);
                    (d.into_bytes(), false)
                } else if let Some(at) = input.get("cut") {
                    (base_file[..at.as_u64().unwrap() as usize].to_vec(), false)
                } else if let Some(at) = input.get("at") {
                    let mut d = base_file.clone();
                    d[at.as_u64().unwrap() as usize] = input["byte"].as_str().unwrap().as_bytes()[0];
                    (d, false)
                } else {
                    (common::bytes(&input["file"]), input["what"] == "base")
                };
                check.eq(format!("parse {input}"), &session_out(&doc, full), &c["out"]);
            }
            "Redact" => {
                let s = lossy(&common::bytes(input));
                let want = common::text(lossy(&common::bytes(&c["out"])).as_bytes());
                check.eq(format!("Redact {input}"), &common::text(redact::redact(&s).as_bytes()), &want);
            }
            "ToolResultBody" => {
                let a = input.as_array().unwrap();
                let md = match &a[3] {
                    Value::String(_) => BTreeMap::from([("c".to_owned(), Any::Number(f64::NAN))]), // a value JSON cannot write
                    v => untyped(v).into_object().unwrap(),
                };
                let got = message::tool_result_body(a[0].as_str().unwrap(), a[1].as_bool().unwrap(), &lossy(&common::bytes(&a[2])), &md);
                check.eq(format!("ToolResultBody {input}"), &common::text(got.as_bytes()), &c["out"]);
            }
            "NotRunMessage" => {
                let a: Vec<&str> = input.as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
                let m = message::not_run_message(a[0], a[1], a[2], a[3]);
                check.eq("NotRunMessage", &json!(gojson::escape_html(&m.to_json().unwrap())), &c["out"]);
            }
            "SetPlan" => {
                let a = input.as_array().unwrap();
                let start = untyped(&a[3]).into_object().unwrap();
                let was_nil = start.is_empty(); // the fixture's only empty start is Go's nil map
                let mut s = Session { plugin_state: start, ..Session::default() };
                let p = PlanState {
                    mode: a[0].as_str().unwrap().into(),
                    pending: a[1].as_str().unwrap().into(),
                    note: a[2].as_str().unwrap().into(),
                };
                s.set_plan(&p);
                let got = if was_nil && s.plugin_state.is_empty() {
                    "null".to_owned()
                } else {
                    gojson::marshal_any(&Any::Object(s.plugin_state.clone())).unwrap()
                };
                check.eq(format!("SetPlan {input}"), &json!(got), &c["out"]);
            }
            "Mutate" => {
                let a = input.as_array().unwrap();
                let mut m = message::Message {
                    role: "assistant".into(),
                    message_id: a[2].as_str().unwrap().into(),
                    metadata: untyped(&a[1]).into_object().unwrap(),
                    ..message::Message::default()
                };
                for k in a[0].as_array().unwrap() {
                    m.mutate(k.as_str().unwrap());
                }
                check.eq(format!("Mutate {input}"), &json!(gojson::marshal_any(&Any::Object(m.metadata.clone())).unwrap()), &c["out"]);
            }
            "FileMode" => {
                let v = input.as_str().unwrap().to_owned();
                check.eq(format!("FileMode {input}"), &json!(session::file_mode(&|_| v.clone())), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    // the migration's helpers are reached through the files above; the retired names are Go's seven
    assert!(["task_get", "task_next", "task_start", "task_complete", "task_cancel", "task_block", "plan_clear"]
        .iter()
        .all(|n| migrate::is_retired_tool(n)));
    check.done("session");
}
