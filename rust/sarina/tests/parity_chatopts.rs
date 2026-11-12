//! testdata/parity/chatopts.json: chat's and resume's flags (Go's `newOpts`) and `build` (the model and runtime
//! configuration, the allowlist) on a fresh run and on stored sessions, one's own and another account's; the usage on -h
//! and the parse errors. The access tier is the one Go read for the case.

mod common;

use sarina::cli::chatopts::ChatOpts;
use serde_json::{json, Value};
use std::collections::HashMap;

#[test]
fn chat_options_match_go() {
    let cases = common::cases("chatopts");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-chatopts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("ws")).unwrap();
    let top = String::from_utf8(sarina::gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    std::fs::write(format!("{top}/schema.json"), r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#)
        .unwrap();
    std::fs::write(format!("{top}/array.json"), "[1,2]").unwrap();
    std::fs::write(format!("{top}/broken.json"), r#"{"a":"#).unwrap();
    std::env::set_current_dir(&top).unwrap();
    let hide = |s: &str| s.replace(&top, "$BASE");
    let unhide = |s: &str| s.replace("$BASE", &top);
    for c in &cases {
        let input = &c["in"];
        let env: HashMap<String, String> = input["env"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect())
            .unwrap_or_default();
        let getenv = move |k: &str| env.get(k).cloned().unwrap_or_default();
        let args: Vec<String> =
            input["args"].as_array().map(|a| a.iter().map(|x| unhide(x.as_str().unwrap())).collect()).unwrap_or_default();
        let mut o = ChatOpts::new("chat", &getenv);
        let mut stderr = Vec::new();
        let mut out = serde_json::Map::new();
        match sarina::flagx::parse(&mut o.fs, &args, &mut stderr) {
            Err(e) => {
                out.insert("parse_error".into(), json!(e));
            }
            Ok(positional) => {
                out.insert("positional".into(), if positional.is_empty() { Value::Null } else { json!(positional) });
                let stored_text = unhide(input["stored"].as_str().unwrap());
                let stored = if stored_text.is_empty() { None } else { Some(sarina::session::parse(stored_text.as_bytes()).unwrap()) };
                o.foreign_loaded = input["foreign"].as_bool().unwrap();
                match o.build(&getenv, stored.as_ref(), input["tier"].as_i64().unwrap()) {
                    Err(e) => {
                        out.insert("error".into(), json!(hide(&e)));
                    }
                    Ok((m, rt, allow)) => {
                        out.insert("model".into(), common::text(hide(&m.to_json(true).unwrap_or_default()).as_bytes()));
                        out.insert("api_key".into(), json!(m.api_key));
                        out.insert("runtime".into(), common::text(hide(&rt.to_json(true).unwrap_or_default()).as_bytes()));
                        out.insert("allow".into(), allow.map_or(Value::Null, |a| json!(a)));
                        if let Some(eo) = o.escalation() {
                            out.insert(
                                "escalation".into(),
                                json!({"cli_path": eo.cli_path, "triggers": if eo.triggers.is_empty() { Value::Null } else { json!(eo.triggers) },
                                       "disable_resource_check": eo.disable_resource_check}),
                            );
                        }
                    }
                }
            }
        }
        out.insert("stderr".into(), common::text(hide(&String::from_utf8_lossy(&stderr)).as_bytes()));
        check.eq(c["fn"].as_str().unwrap(), &Value::Object(out), &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("chatopts");
}
