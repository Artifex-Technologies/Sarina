//! testdata/parity/llmtypes.json: go/llm's types: UsageFromProvider and Usage's JSON both ways, a tool call's arguments as
//! replayed, a tool definition's JSON, a wire message's JSON.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::gotext::lossy;
use sarina::llm::types::{usage_from_provider, FunctionCall, Message, ToolCall, ToolSpec, Usage, WireToolCall};
use serde_json::{json, Value};

#[test]
fn llm_types_match_go() {
    let mut check = common::Check::new();
    for c in common::cases("llmtypes") {
        let input = &c["in"];
        match c["fn"].as_str().unwrap() {
            "doc" => {
                let doc = common::bytes(input);
                let (v, _) = gojson::unmarshal_any(&doc);
                let u = usage_from_provider(Some(&v));
                let back = Usage::from_json(&doc);
                let args = match &v {
                    Any::Object(m) => Some(m.clone()),
                    _ => None,
                };
                let call = ToolCall { arguments: args.clone(), ..ToolCall::default() };
                let spec = ToolSpec { name: "n<".into(), description: "d & é".into(), parameters: args };
                let spec = match spec.to_json() {
                    Ok(s) => common::text(s.as_bytes()),
                    Err(e) => json!(e.0),
                };
                let got = json!({
                    "provider": u.to_json(),
                    "unmarshal": back.to_json(),
                    "zero": u.is_zero(),
                    "sum": u.add(&back).total(),
                    "args": common::text(call.arguments_json().as_bytes()),
                    "spec": spec,
                });
                check.eq(format!("doc {input}"), &got, &c["out"]);
            }
            "Message" => {
                let a = input.as_array().unwrap();
                let s = |i: usize| a[i].as_str().unwrap().to_owned();
                let calls = a[4]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| {
                        let c: Vec<String> = c.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect();
                        WireToolCall {
                            id: c[0].clone(),
                            kind: c[1].clone(),
                            function: FunctionCall { name: c[2].clone(), arguments: c[3].clone() },
                        }
                    })
                    .collect();
                let m = Message { role: s(0), content: s(1), name: s(2), tool_call_id: s(3), tool_calls: calls };
                check.eq(format!("Message {input}"), &json!(m.to_json(true)), &c["out"]);
            }
            "Wire" => {
                let a = input.as_array().unwrap();
                let args = match &a[2]["o"] {
                    Value::Object(m) if !m.is_empty() => {
                        Some(m.iter().map(|(k, v)| (k.clone(), Any::String(lossy(&common::bytes(&v["s"]))))).collect())
                    }
                    _ => None,
                };
                let call = ToolCall {
                    id: a[0].as_str().unwrap().into(),
                    name: a[1].as_str().unwrap().into(),
                    arguments: args,
                    ..ToolCall::default()
                };
                check.eq(format!("Wire {input}"), &json!(call.wire().to_json(true)), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    check.done("llmtypes");
}
