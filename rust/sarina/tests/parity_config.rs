//! testdata/parity/config.json: go/config: ModelFromEnv, the integer settings, ContextConfig's Settings against the
//! environment, AsInt and AsFloat, the JSON of a default runtime and of a model.

mod common;

use sarina::config::{self, ContextConfig};
use sarina::goany::Any;
use sarina::gojson;
use sarina::gotext::lossy;
use serde_json::{json, Value};
use std::collections::HashMap;

fn env_of(v: &Value) -> HashMap<String, String> {
    v.as_object().unwrap().iter().map(|(k, x)| (k.clone(), x.as_str().unwrap().to_owned())).collect()
}

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

/// A ContextConfig back from the JSON Go wrote of it.
fn context_config(json: &str) -> ContextConfig {
    let m = gojson::unmarshal_any(json.as_bytes()).0.into_object().unwrap();
    let b = |k: &str| matches!(m.get(k), Some(Any::Bool(true)));
    let n = |k: &str| config::as_int(m.get(k)).unwrap_or(0);
    ContextConfig {
        disable_notes: b("disable_notes"),
        notes_every_turns: n("notes_every_turns"),
        notes_window_percent: n("notes_window_percent"),
        disable_clearing: b("disable_clearing"),
        clear_window_percent: n("clear_window_percent"),
        clear_keep_recent: n("clear_keep_recent"),
        clear_min_tokens: n("clear_min_tokens"),
        disable_llm_compaction: b("disable_llm_compaction"),
        disable_discovery: b("disable_context_discovery"),
    }
}

#[test]
fn config_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("config") {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "ModelFromEnv" => {
                let env = env_of(input);
                let m = config::model_from_env(&|k| env.get(k).cloned().unwrap_or_default());
                json!({"json": m.to_json(true).unwrap_or_else(|e| e.0), "key": m.api_key})
            }
            "intEnv" => {
                let v = input.as_str().unwrap().to_owned();
                let get = |_: &str| v.clone();
                json!([config::request_max_tokens_from_env(&get), config::context_window_from_env(&get)])
            }
            "Settings" => {
                let a = input.as_array().unwrap();
                let cfg = a[0].as_str().map(context_config);
                let env = env_of(&a[1]);
                let s = ContextConfig::settings(cfg.as_ref(), &|k| env.get(k).cloned().unwrap_or_default());
                let zero = cfg.as_ref().is_none_or(ContextConfig::is_zero);
                // a float as Go's encoder writes it (1, not 1.0)
                let f = |x: f64| {
                    let mut t = String::new();
                    gojson::write_f64(&mut t, x).unwrap();
                    serde_json::from_str::<Value>(&t).unwrap()
                };
                json!([
                    s.notes,
                    s.notes_every_turns,
                    f(s.notes_window_fraction),
                    s.clear,
                    f(s.clear_window_fraction),
                    s.clear_keep_recent,
                    s.clear_min_tokens,
                    s.llm_compaction,
                    s.discovery,
                    zero
                ])
            }
            "As" => {
                let v = untyped(input);
                let n = config::as_int(Some(&v));
                let f = config::as_float(Some(&v));
                let mut fj = String::new();
                if let Err(e) = gojson::write_f64(&mut fj, f.unwrap_or(0.0)) {
                    fj = e.0;
                }
                json!([n.unwrap_or(0), n.is_some(), fj, f.is_some()])
            }
            "DefaultRuntime" => json!(config::default_runtime(input.as_str().unwrap()).to_json(true).unwrap()),
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    check.done("config");
}
