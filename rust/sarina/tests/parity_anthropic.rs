//! testdata/parity/anthropic.json: the Anthropic client: BuildBody, parseAnthropic, the endpoints, anthropicID, and
//! Complete against scripted servers (the same answers and pauses as Go's test): the retry, the key scrubbed from every
//! error, the timer, cancellation, the replay of a finished answer as events, TLS against an authority made in the test.

mod common;

use sarina::config::{ModelConfig, OutputSchema};
use sarina::goany::Any;
use sarina::goctx::Ctx;
use sarina::gohttp::tls_config;
use sarina::llm::anthropic::{anthropic_id, parse_anthropic, AnthropicClient};
use sarina::llm::client::Error;
use sarina::llm::openai::{Request, StreamEvent};
use sarina::llm::types::{FunctionCall, Message, ToolSpec, Turn, WireToolCall};
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

fn turn_fields(t: &Turn) -> Value {
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
        "raw": t.raw.as_ref().map_or(Value::Null, |r| common::typed(&Any::Object(r.clone())))})
}

fn err_short(e: &Error, addr: &str) -> Value {
    let (msg, status) = match e {
        Error::Backend(b) => (b.message.clone(), b.status),
        Error::Ctx(s) => (s.to_string(), -1),
    };
    let msg = if addr.is_empty() { msg } else { msg.replace(addr, "{ADDR}") };
    if msg.len() > 4096 {
        let h = sha2::Sha256::digest(msg.as_bytes());
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        return json!([String::from_utf8_lossy(&msg.as_bytes()[..200]), msg.len(), hex, status]);
    }
    json!([common::text(msg.as_bytes()), status])
}

fn s(v: &str) -> Any {
    Any::String(v.into())
}

fn o(kv: Vec<(&str, Any)>) -> BTreeMap<String, Any> {
    kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn msg(role: &str, content: &str) -> Message {
    Message { role: role.into(), content: content.into(), ..Message::default() }
}

/// Go's anthropicRequests.
fn requests() -> Vec<Request> {
    let call = |id: &str, name: &str, args: &str| WireToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall { name: name.into(), arguments: args.into() },
    };
    let calls = vec![
        call("call.1/\u{e9}", "list_dir", r#"{"path":"<a> & b","n":1.5e3,"z":null}"#),
        call("", "read_file", "not json"),
        call("c3", "x", "null"),
        call("c4", "y", "[1,2]"),
        call("c5", "z", r#"{"a":1,"a":2,"b":"é\ud800"}"#),
    ];
    let schema = OutputSchema {
        name: "out<1>".into(),
        schema: o(vec![
            ("type", s("object")),
            ("properties", Any::Object(o(vec![("a", Any::Object(o(vec![("type", s("string")), ("pattern", s("<&>"))])))]))),
        ]),
        strict: false,
    };
    let tools = vec![
        ToolSpec {
            name: "list_dir".into(),
            description: "lists <dirs> & more".into(),
            parameters: Some(o(vec![
                ("type", s("object")),
                ("properties", Any::Object(o(vec![("path", Any::Object(o(vec![("type", s("string"))])))]))),
            ])),
        },
        ToolSpec { name: "plain".into(), ..ToolSpec::default() },
    ];
    let mut assistant = msg("assistant", "sure");
    assistant.tool_calls = calls;
    let tool =
        |id: &str, content: &str| Message { role: "tool".into(), tool_call_id: id.into(), content: content.into(), ..Message::default() };
    vec![
        Request::default(),
        Request {
            messages: vec![
                msg("system", "sys one"),
                msg("system", "  "),
                msg("user", "list <files>"),
                assistant,
                tool("call.1/\u{e9}", "a.txt"),
                tool("", " \n"),
                msg("user", "thanks"),
                msg("system", "late note"),
            ],
            tools: tools.clone(),
            ..Request::default()
        },
        Request {
            messages: vec![msg("assistant", "I start"), msg("assistant", "  "), msg("developer", "odd role"), msg("user", "")],
            max_tokens: 100,
            schema: Some(schema),
            ..Request::default()
        },
        Request { messages: vec![tool("t!", "r"), msg("user", "q")], max_tokens: -5, tools: tools[1..].to_vec(), ..Request::default() },
        Request { messages: vec![msg("user", "a /no_think")], stream: true, no_think: true, ..Request::default() },
    ]
}

#[test]
fn anthropic_matches_go() {
    let mut check = common::Check::new();
    let cases = common::cases("anthropic");
    let key = cases.iter().find(|c| c["fn"] == "key").unwrap()["out"].as_str().unwrap().to_string();
    let reqs = requests();
    let ca = common::TestCa::new("Sarina parity test CA");
    let other = common::TestCa::new("Another test CA");
    for c in &cases {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "key" => continue,
            "body" => {
                let temp = f64::from_bits(u64::from_str_radix(input[1].as_str().unwrap(), 16).unwrap());
                let cl = AnthropicClient::new(ModelConfig { model: "claude <x>".into(), temperature: temp, ..ModelConfig::default() });
                match cl.build_body(&reqs[input[0].as_u64().unwrap() as usize]) {
                    Ok(b) => json!([common::text(b.as_bytes()), ""]),
                    Err(e) => json!([Value::Null, e]),
                }
            }
            "parse" => match parse_anthropic(&common::bytes(input)) {
                Ok(t) => json!([turn_fields(&t), Value::Null]),
                Err(e) => json!([Value::Null, [common::text(e.message.as_bytes()), e.status]]),
            },
            "endpoints" => {
                let cl = AnthropicClient::new(ModelConfig { base_url: input.as_str().unwrap().into(), ..ModelConfig::default() });
                json!([cl.endpoint(), cl.models_endpoint()])
            }
            "id" => json!(anthropic_id(&String::from_utf8_lossy(&common::bytes(input)))),
            "complete" => {
                let mut roots = ca.roots();
                let (srv, https) = match input["tls"].as_str().unwrap() {
                    "trusted" => (Some(ca.server(&[[127, 0, 0, 1]], &[])), true),
                    "untrusted" => {
                        roots = other.roots();
                        (Some(ca.server(&[[127, 0, 0, 1]], &[])), true)
                    }
                    _ => (None, false),
                };
                let base_in = input["base"].as_str().unwrap();
                let (mut server, addr, base) = if base_in == "refused" {
                    let ln = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                    let addr = ln.local_addr().unwrap().to_string();
                    drop(ln);
                    (None, addr.clone(), format!("http://{addr}/{key}"))
                } else {
                    let srv = common::raw_seq(common::steps(&input["script"]), srv);
                    let addr = srv.addr.clone();
                    (Some(srv), addr.clone(), format!("{}://{addr}{base_in}", if https { "https" } else { "http" }))
                };
                let api_key = match input["apiKey"].as_str().unwrap() {
                    "" => key.clone(),
                    k => k.to_string(),
                };
                let mut cl = AnthropicClient::new(ModelConfig {
                    model: "claude-test".into(),
                    base_url: base,
                    api_key,
                    temperature: 0.3,
                    timeout_seconds: input["timeout"].as_f64().unwrap(),
                    ..ModelConfig::default()
                });
                cl.retry_delay = Duration::from_millis(input["retryMs"].as_u64().unwrap().max(30));
                cl.tls = Some(tls_config(roots));
                if let Some(m) = input["maxResp"].as_u64().filter(|&m| m > 0) {
                    cl.max_response = m as usize;
                }
                let ctx = Ctx::background();
                if let Some(ms) = input["cancelMs"].as_u64().filter(|&m| m > 0) {
                    let c2 = ctx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(ms));
                        c2.cancel();
                    });
                }
                let req = Request {
                    messages: vec![msg("user", "hi")],
                    stream: input["stream"].as_bool().unwrap(),
                    max_tokens: input["maxTokens"].as_i64().unwrap(),
                    ..Request::default()
                };
                let mut evs = Vec::new();
                let mut on = |e: StreamEvent| {
                    let u = e.usage.map_or(Value::Null, |u| json!([u.input_tokens, u.output_tokens]));
                    evs.push(json!([e.kind, common::text(e.delta.as_bytes()), e.finish_reason, u]));
                };
                let start = Instant::now();
                let r = cl.complete(&ctx, &req, Some(&mut on));
                let _ = start;
                if let Err(e) = &r {
                    assert!(!e.text().contains(&key), "the key is in the error: {}", e.text());
                }
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
                server.take();
                json!({"turn": turn, "err": err, "events": if evs.is_empty() { Value::Null } else { Value::Array(evs) },
                    "requests": requests})
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {}", c["fn"], input.get("name").unwrap_or(input)), &got, &c["out"]);
    }
    check.done("anthropic");
}
