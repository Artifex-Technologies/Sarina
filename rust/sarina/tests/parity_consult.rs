//! testdata/parity/consult.json: Heimdall's review client: the request Consult sends and the verdict it makes of
//! scripted answers from a raw loopback server, and the URL settings.

mod common;

use sarina::heimdall::{consult, url, Verdict, ENV_VAR, LEGACY_ENV_VAR};
use serde_json::{json, Value};
use std::time::Duration;

fn verdict_out(v: Option<Verdict>) -> Value {
    match v {
        None => Value::Null,
        Some(v) => json!({"denied": v.denied, "reasons": v.reasons.map_or(Value::Null, |r| json!(r))}),
    }
}

fn reply_bytes(spec: &Value) -> Vec<u8> {
    let big = spec["big"].as_u64().unwrap() as usize;
    if big == 0 {
        return common::bytes(&spec["raw"]);
    }
    let body = format!(r#"{{"decision":"deny","pad":"{}"}}"#, "x".repeat(big - r#"{"decision":"deny","pad":""}"#.len()));
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

#[test]
fn consult_matches_go() {
    let mut check = common::Check::new();
    let cases = common::cases("consult");
    let specs = cases.iter().find(|c| c["fn"] == "replies").unwrap()["in"].as_array().unwrap().clone();
    for c in &cases {
        let input = &c["in"];
        match c["fn"].as_str().unwrap() {
            "replies" => {}
            "reply" => {
                let spec = &specs[input.as_u64().unwrap() as usize];
                let (addr, got, _release) =
                    common::raw_server(reply_bytes(spec), Duration::from_millis(spec["delay"].as_u64().unwrap()), spec["open"] == true);
                let u = format!("http://{addr}/v1/review?probe=1");
                let v = consult("ignore \u{e9} <b>\u{FFFD}", "memory_proposal", "s-1", &|k| {
                    if k == ENV_VAR {
                        u.clone()
                    } else {
                        String::new()
                    }
                });
                let req = got.recv_timeout(Duration::from_secs(5)).unwrap_or(Value::Null);
                check.eq(format!("reply {input}"), &json!({"verdict": verdict_out(v), "request": req}), &c["out"]);
            }
            "env" => {
                let (a, b) = (input[0].as_str().unwrap().to_string(), input[1].as_str().unwrap().to_string());
                let getenv = |k: &str| match k {
                    ENV_VAR => a.clone(),
                    LEGACY_ENV_VAR => b.clone(),
                    _ => String::new(),
                };
                check.eq(
                    format!("env {input}"),
                    &json!([verdict_out(consult("text", "memory_proposal", "", &getenv)), url(&getenv)]),
                    &c["out"],
                );
            }
            other => panic!("unknown fn {other}"),
        }
    }
    check.done("consult");
}
