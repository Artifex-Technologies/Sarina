//! testdata/parity/openai.json: the OpenAI-compatible client's wire format: BuildBody for requests of every shape,
//! parseResponse, readStream (the turn and every event) and the bounds on what is read.

mod common;

use sarina::config::OutputSchema;
use sarina::goany::Any;
use sarina::llm::openai::{build_body, parse_response, read_stream, BackendError, Request};
use sarina::llm::types::{FunctionCall, Message, ToolSpec, Turn, WireToolCall};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn obj(kv: Vec<(&str, Any)>) -> BTreeMap<String, Any> {
    kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

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

fn result_out(r: Result<Turn, BackendError>) -> (Value, Value) {
    match r {
        Ok(t) => (turn_fields(&t), Value::Null),
        Err(e) => (Value::Null, json!([common::text(e.message.as_bytes()), e.status])),
    }
}

fn requests() -> Vec<Request> {
    let msgs = vec![
        Message { role: "system".into(), content: "be <brief> & kind".into(), ..Message::default() },
        Message { role: "user".into(), content: "hi \u{e9} /no_think  ".into(), ..Message::default() },
        Message {
            role: "assistant".into(),
            tool_calls: vec![WireToolCall {
                id: "c1".into(),
                kind: "function".into(),
                function: FunctionCall { name: "read_file".into(), arguments: r#"{"path":"<a>"}"#.into() },
            }],
            ..Message::default()
        },
        Message {
            role: "tool".into(),
            content: "result".into(),
            tool_call_id: "c1".into(),
            name: "read_file".into(),
            ..Message::default()
        },
    ];
    let string_type = Any::Object(obj(vec![("type", Any::String("string".into()))]));
    let tools = vec![
        ToolSpec {
            name: "read_file".into(),
            description: "Reads <a> file & more".into(),
            parameters: Some(obj(vec![
                ("type", Any::String("object".into())),
                ("properties", Any::Object(obj(vec![("path", string_type.clone())]))),
            ])),
        },
        ToolSpec { name: "noop".into(), description: "nothing".into(), parameters: None },
    ];
    let schema = OutputSchema {
        name: "out".into(),
        schema: obj(vec![("type", Any::String("object".into())), ("properties", Any::Object(obj(vec![("a", string_type)])))]),
        strict: true,
    };
    let user = |c: &str| Message { role: "user".into(), content: c.into(), ..Message::default() };
    vec![
        Request::default(),
        Request { messages: msgs.clone(), tools: tools.clone(), stream: true, max_tokens: 512, ..Request::default() },
        Request { messages: msgs[..1].to_vec(), schema: Some(schema), ..Request::default() },
        Request {
            messages: vec![user("a /no_think"), Message { role: "assistant".into(), content: "b".into(), ..Message::default() }],
            ..Request::default()
        },
        Request { messages: msgs[..2].to_vec(), no_think: true, tools: tools[..1].to_vec(), ..Request::default() },
        Request { messages: vec![user("/no_think x")], ..Request::default() },
    ]
}

#[test]
fn openai_matches_go() {
    let mut check = common::Check::new();
    let reqs = requests();
    for c in common::cases("openai") {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "body" => {
                let temp = f64::from_bits(u64::from_str_radix(input[1].as_str().unwrap(), 16).unwrap());
                match build_body("qwen <3>", temp, &reqs[input[0].as_u64().unwrap() as usize]) {
                    Ok(b) => json!([common::text(b.as_bytes()), ""]),
                    Err(e) => json!([Value::Null, e]),
                }
            }
            "response" => {
                let (t, e) = result_out(parse_response(&common::bytes(input)));
                json!([t, e])
            }
            "stream" | "streamBound" => {
                let data = common::bytes(&input[0]);
                let (resp, line) = (input[1].as_u64().unwrap() as usize, input[2].as_u64().unwrap() as usize);
                let mut touches = 0;
                let mut evs = Vec::new();
                let r = read_stream(&data[..], resp, line, &mut || touches += 1, &mut |e| {
                    let u = e.usage.as_ref().map_or(Value::Null, |u| json!([u.input_tokens, u.output_tokens, u.reasoning_tokens]));
                    evs.push(json!([
                        e.kind,
                        common::text(e.delta.as_bytes()),
                        e.tool_call_index,
                        e.tool_call_id,
                        e.tool_name,
                        common::text(e.arguments_delta.as_bytes()),
                        e.finish_reason,
                        u
                    ]));
                });
                let (t, e) = result_out(r);
                json!([t, e, evs, touches])
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    check.done("openai");
}
