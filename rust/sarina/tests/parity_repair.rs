//! testdata/parity/repair.json: RepairJSON, ParseArguments and Normalize on the Go tests' inputs, tool calls in every shape
//! local servers write them, every cut of each, and generated damage.

mod common;

use sarina::goany::Any;
use sarina::llm::normalize::normalize;
use sarina::llm::repair::{parse_arguments, repair_json};
use sarina::llm::types::{ToolCall, Turn};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn text(v: &Value) -> String {
    String::from_utf8(common::bytes(v)).expect("the fixture's texts are UTF-8")
}

fn turn_out(t: &Turn) -> Value {
    let calls: Vec<Value> = t
        .tool_calls
        .iter()
        .map(|c| {
            let a = c.arguments.as_ref().map_or(Value::Null, |m| common::typed(&Any::Object(m.clone())));
            json!([c.id, c.name, a, common::text(c.raw_arguments.as_bytes()), c.parse_error, c.truncated])
        })
        .collect();
    json!({"content": common::text(t.content.as_bytes()), "reasoning": common::text(t.reasoning.as_bytes()),
        "calls": if calls.is_empty() { Value::Null } else { json!(calls) }})
}

#[test]
fn repair_matches_go() {
    let mut check = common::Check::new();
    let names: Vec<String> = vec!["bash".into(), "read_file".into()];
    for c in common::cases("repair") {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "repair" => {
                let s = text(input);
                let (v, trunc, ok) = repair_json(&s);
                let (args, atrunc, aerr) = parse_arguments(&s);
                let a = args.map_or(Value::Null, |m| common::typed(&Any::Object(m)));
                json!([v.as_ref().map_or(Value::Null, common::typed), trunc, ok, a, atrunc, aerr])
            }
            "normalize" => {
                let mut t = Turn {
                    content: text(&input[0]),
                    reasoning: "earlier".into(),
                    finish_reason: input[1].as_str().unwrap().into(),
                    ..Turn::default()
                };
                normalize(&mut t, if input[2] == true { &names } else { &[] });
                turn_out(&t)
            }
            "ids" => {
                let call = |id: &str, name: &str| ToolCall { id: id.into(), name: name.into(), ..ToolCall::default() };
                let mut b = call("call_1", "b");
                b.truncated = true;
                b.arguments = Some(BTreeMap::from([("x".to_string(), Any::Number(1.0))]));
                let mut d = call("dup", "d");
                d.truncated = true;
                d.parse_error = "already".into();
                let mut t = Turn {
                    content: "<think>x</think>".into(),
                    finish_reason: input.as_str().unwrap().into(),
                    tool_calls: vec![
                        call("", "a"),
                        b,
                        call("call_1", "c"),
                        d,
                        call("dup", "e"),
                        call("call_4", "f"),
                        call("", "g"),
                        call("call_6_1", "h"),
                        call("call_6", "i"),
                        call("", "j"),
                    ],
                    ..Turn::default()
                };
                normalize(&mut t, &names);
                turn_out(&t)
            }
            "ids2" => {
                let call = |id: &str, name: &str| ToolCall { id: id.into(), name: name.into(), ..ToolCall::default() };
                let mut t = Turn {
                    tool_calls: vec![
                        call("call_1", "a"),
                        call("", "b"),
                        call("call_1_1", "c"),
                        call("call_1", "d"),
                        call("call_3", "e"),
                        call("call_3_1", "f"),
                        call("", "g"),
                    ],
                    ..Turn::default()
                };
                normalize(&mut t, &[]);
                turn_out(&t)
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    check.done("repair");
}
