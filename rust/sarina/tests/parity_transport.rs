//! testdata/parity/transport.json: the OpenAI-compatible client's `Complete` against scripted servers (the same answers
//! with the same pauses Go's test used): the requests each connection carried, the turn with the model that answered or
//! the error, and the events handed on; the retry, the fallback models, the timer (whole call, extended by max_tokens,
//! reset by streamed lines), cancellation, the bounds, and TLS against an authority made in the test.

mod common;

use sarina::config::ModelConfig;
use sarina::goany::Any;
use sarina::goctx::Ctx;
use sarina::gohttp::tls_config;
use sarina::llm::client::{Client, Error, DEFAULT_RETRY_DELAY};
use sarina::llm::openai::Request;
use sarina::llm::types::{Message, ToolSpec, Turn};
use serde_json::{json, Value};
use sha2::Digest;
use std::time::{Duration, Instant};

pub fn turn_fields(t: &Turn) -> Value {
    let calls: Vec<Value> = t
        .tool_calls
        .iter()
        .map(|c| {
            let a = c.arguments.as_ref().map_or(Value::Null, |m| common::typed(&Any::Object(m.clone())));
            json!([c.id, c.name, a, common::text(c.raw_arguments.as_bytes()), c.parse_error, c.truncated])
        })
        .collect();
    let u = &t.usage;
    json!({"content": common::text(t.content.as_bytes()), "reasoning": common::text(t.reasoning.as_bytes()), "finish": t.finish_reason,
        "hasFinish": t.has_finish_reason,
        "usage": [u.input_tokens, u.output_tokens, u.reasoning_tokens, u.cache_creation_input_tokens, u.cache_read_input_tokens],
        "usageReported": t.usage_reported, "calls": if calls.is_empty() { Value::Null } else { json!(calls) },
        "raw": t.raw.as_ref().map_or(Value::Null, |r| common::typed(&Any::Object(r.clone()))), "model": t.model})
}

/// The Go test's errShort.
pub fn err_short(e: &Error, addr: &str) -> Value {
    let (msg, status) = match e {
        Error::Backend(b) => (b.message.clone(), b.status),
        Error::Ctx(s) => (s.to_string(), -1),
        Error::Cli(s) => (s.clone(), -2),
    };
    let msg = if addr.is_empty() { msg } else { msg.replace(addr, "{ADDR}") };
    if msg.len() > 4096 {
        let h = sha2::Sha256::digest(msg.as_bytes());
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        return json!([String::from_utf8_lossy(&msg.as_bytes()[..200]), msg.len(), hex, status]);
    }
    json!([common::text(msg.as_bytes()), status])
}

#[test]
fn complete_matches_go() {
    let mut check = common::Check::new();
    let ca = common::TestCa::new("Sarina parity test CA");
    let other = common::TestCa::new("Another test CA");
    for c in common::cases("transport") {
        let input = &c["in"];
        if c["fn"] == "defaults" {
            check.eq(
                "defaults",
                &json!([DEFAULT_RETRY_DELAY.as_nanos() as u64, sarina::config::DEFAULT_TIMEOUT_SECONDS as i64]),
                &c["out"],
            );
            continue;
        }
        let name = input["name"].as_str().unwrap();
        let mut roots = ca.roots();
        let (srv, https) = match input["tls"].as_str().unwrap() {
            "trusted" => (Some(ca.server(&[[127, 0, 0, 1]], &[])), true),
            "untrusted" => {
                roots = other.roots();
                (Some(ca.server(&[[127, 0, 0, 1]], &[])), true)
            }
            "wrongip" => (Some(ca.server(&[[127, 0, 0, 2]], &[])), true),
            "dnsonly" => (Some(ca.server(&[], &["localhost"])), true),
            _ => (None, false),
        };
        let script = common::steps(&input["script"]);
        let base_in = input["base"].as_str().unwrap();
        let mut server = None;
        let mut addr = String::new();
        let base = if base_in == "refused" {
            let ln = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            addr = ln.local_addr().unwrap().to_string();
            drop(ln);
            format!("http://{addr}/v1")
        } else if input["script"].as_array().is_some_and(|a| !a.is_empty()) {
            let s = common::raw_seq(script, srv);
            addr = s.addr.clone();
            server = Some(s);
            format!("{}://{addr}/v1/", if https { "https" } else { "http" })
        } else {
            base_in.to_string()
        };
        let key = match input["apiKey"].as_str().unwrap() {
            "" => "test-key",
            k => k,
        };
        let fallbacks: Vec<String> =
            input["fallbacks"].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect()).unwrap_or_default();
        let mut client = Client::new(ModelConfig {
            model: "m1".into(),
            base_url: base,
            api_key: key.into(),
            temperature: 0.2,
            timeout_seconds: input["timeout"].as_f64().unwrap(),
            fallbacks,
            ..ModelConfig::default()
        });
        client.retry_delay = Duration::from_millis(input["retryMs"].as_u64().unwrap().max(30));
        client.tls = Some(tls_config(roots));
        if let Some(m) = input["maxResp"].as_u64().filter(|&m| m > 0) {
            client.max_response = m as usize;
        }
        let mut req = Request {
            messages: vec![
                Message { role: "system".into(), content: "be brief".into(), ..Message::default() },
                Message { role: "user".into(), content: "hi <there>".into(), ..Message::default() },
            ],
            stream: input["stream"].as_bool().unwrap(),
            max_tokens: input["maxTokens"].as_i64().unwrap(),
            ..Request::default()
        };
        if input["tools"].as_bool().unwrap() {
            req.tools = vec![ToolSpec { name: "read_file".into(), description: "Reads a file".into(), ..ToolSpec::default() }];
        }
        let ctx = match input["ctxMs"].as_u64().unwrap() {
            0 => Ctx::background(),
            ms => Ctx::with_deadline(Instant::now() + Duration::from_millis(ms)),
        };
        if let Some(ms) = input["cancelMs"].as_u64().filter(|&m| m > 0) {
            let c2 = ctx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                c2.cancel();
            });
        }
        let mut evs = Vec::new();
        let mut on = |e: sarina::llm::openai::StreamEvent| {
            evs.push(json!([e.kind, common::text(e.delta.as_bytes()), e.finish_reason]));
        };
        let r = client.complete(&ctx, &req, Some(&mut on));
        let (turn, err) = match &r {
            Ok(t) => (turn_fields(t), Value::Null),
            Err(e) => (Value::Null, err_short(e, &addr)),
        };
        let requests = server.as_ref().map_or(Value::Null, |s| {
            let seen = common::settle(&s.seen);
            if seen.is_empty() {
                Value::Null
            } else {
                Value::Array(seen)
            }
        });
        let got = json!({"turn": turn, "err": err, "events": if evs.is_empty() { Value::Null } else { Value::Array(evs) },
            "requests": requests});
        check.eq(name, &got, &c["out"]);
        drop(server);
    }
    check.done("transport");
}
