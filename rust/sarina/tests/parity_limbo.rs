//! testdata/parity/limbo.json: the Limbo client against a scripted agent socket: profiles by name and with details, the
//! skipped list, validation runs (start, polling, the result, its text), bundles, Limbo's errors, damaged and over-long
//! answers, and the socket rules.

mod common;

use sarina::limbo::{Client, RunResult};
use serde_json::{json, Value};
use std::time::Duration;

fn result_out(r: &RunResult) -> Value {
    json!({"id": r.run_id, "profile": r.profile, "status": r.status, "exit": r.exit_code, "stdout": common::text(r.stdout.as_bytes()),
        "stderr": common::text(r.stderr.as_bytes()), "truncated": r.truncated, "error": common::text(r.error.as_bytes()), "code": r.code,
        "remedy": r.remedy, "infra": r.infrastructure_failure(), "ok": r.succeeded(), "text": common::text(r.as_text().as_bytes())})
}

fn null_if_empty(v: Vec<Value>) -> Value {
    if v.is_empty() {
        Value::Null
    } else {
        Value::Array(v)
    }
}

fn strings(v: Vec<String>) -> Vec<Value> {
    v.into_iter().map(Value::String).collect()
}

#[test]
fn limbo_matches_go() {
    let mut check = common::Check::new();
    let no_env = |_: &str| String::new();
    for c in common::cases("limbo") {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "call" => {
                let dir = std::env::temp_dir().join(format!("lmb-{}-{}", std::process::id(), input["i"]));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                let sock = dir.join("agent.sock");
                let replies: Vec<Vec<u8>> = input["replies"].as_array().map_or(Vec::new(), |a| a.iter().map(common::bytes).collect());
                let seen = common::raw_unix_server(&sock, replies);
                let sp = sock.to_str().unwrap().to_string();
                let client = Client { path: sp.clone(), poll_interval: Duration::from_millis(1), ..Client::default() };
                let err_text = |e: Option<String>| e.unwrap_or_default().replace(&sp, "{SOCK}");
                let (profile, command) = (input["profile"].as_str().unwrap(), input["command"].as_str().unwrap());
                let timeout = input["timeout"].as_i64().unwrap();
                let mut out = serde_json::Map::new();
                match input["call"].as_str().unwrap() {
                    "list" => {
                        let r = client.list_profiles(&no_env);
                        let (names, e) = match r {
                            Ok(n) => (null_if_empty(strings(n)), None),
                            Err(e) => (Value::Null, Some(e.0)),
                        };
                        out.insert("names".into(), names);
                        out.insert("error".into(), json!(err_text(e)));
                    }
                    "describe" => {
                        let (l, e) = match client.describe_profiles_and_skipped(&no_env) {
                            Ok(l) => (l, None),
                            Err(e) => (Default::default(), Some(e.0)),
                        };
                        out.insert("profiles".into(), null_if_empty(strings(l.profiles.iter().map(|p| p.as_text()).collect())));
                        out.insert("skipped".into(), null_if_empty(strings(l.skipped_lines())));
                        out.insert("error".into(), json!(err_text(e)));
                    }
                    call => {
                        let r = if call == "bundle" {
                            client.validate_bundle(profile, command, &common::bytes(&input["bundle"]), timeout, &no_env)
                        } else {
                            client.validate(profile, command, timeout, &no_env)
                        };
                        let (r, e) = match r {
                            Ok(r) => (r, None),
                            Err(e) => (RunResult::default(), Some(e.0)),
                        };
                        out.insert("result".into(), result_out(&r));
                        out.insert("error".into(), json!(err_text(e)));
                    }
                }
                let reqs = seen.lock().unwrap().clone();
                out.insert("requests".into(), null_if_empty(reqs));
                let _ = std::fs::remove_dir_all(&dir);
                Value::Object(out)
            }
            "socket" => {
                let client = Client { path: input.as_str().unwrap().to_string(), ..Client::default() };
                json!(client.list_profiles(&|_| String::new()).err().map(|e| e.0).unwrap_or_default())
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {}", c["fn"], input.get("i").unwrap_or(input)), &got, &c["out"]);
    }
    check.done("limbo");
}
