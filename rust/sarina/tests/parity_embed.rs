//! testdata/parity/embed.json: Default's settings, and Embed against scripted answers from a raw loopback server (the
//! request, vectors as float32 bits or Go's error), and a refused connection.

mod common;

use sarina::embed::{default_embedder, Embedder, Ollama};
use serde_json::{json, Value};
use std::time::Duration;

#[test]
fn embed_matches_go() {
    let mut check = common::Check::new();
    let cases = common::cases("embed");
    let replies = cases.iter().find(|c| c["fn"] == "replies").unwrap()["in"].as_array().unwrap().clone();
    for c in &cases {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "replies" => continue,
            "default" => {
                let env: Vec<String> = input.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
                let e = default_embedder(&|k| match k {
                    "OLLAMA_HOST" => env[0].clone(),
                    "OPENAI_BASE_URL" => env[1].clone(),
                    "SARINA_EMBED_MODEL" => env[2].clone(),
                    _ => String::new(),
                });
                json!([e.base, e.model, e.name()])
            }
            "reply" => {
                let raw = common::bytes(&replies[input.as_u64().unwrap() as usize]);
                let (addr, got, _release) = common::raw_server(raw, Duration::ZERO, false);
                let e = Ollama { base: format!("http://{addr}"), model: "nomic-embed-text".into() };
                let r = e.embed(&["first <text> \u{e9}".to_string(), String::new()]);
                let req = got.recv_timeout(Duration::from_secs(5)).unwrap_or(Value::Null);
                match r {
                    Ok(vs) => {
                        let vs: Vec<String> =
                            vs.iter().map(|v| v.iter().map(|f| format!("{:x}", f.to_bits())).collect::<Vec<_>>().join(",")).collect();
                        json!({"request": req, "vectors": vs})
                    }
                    Err(e) => json!({"request": req, "error": e.replace(&addr, "{ADDR}")}),
                }
            }
            "refused" => {
                let ln = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let addr = ln.local_addr().unwrap().to_string();
                drop(ln);
                let e = Ollama { base: format!("http://{addr}"), model: "m".into() };
                json!(e.embed(&["x".to_string()]).err().unwrap_or_default().replace(&addr, "{ADDR}"))
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    check.done("embed");
}
