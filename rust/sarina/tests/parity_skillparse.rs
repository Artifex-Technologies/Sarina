//! testdata/parity/skillparse.json: the skill file format (Parse), ValidName and Expand. Texts are compared as strings: a
//! byte that is not UTF-8 in Go's text is U+FFFD in Rust's.

mod common;

use sarina::gotext::lossy;
use sarina::skills::parse::{parse, valid_name};
use serde_json::{json, Value};

fn as_strings(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("hex") => json!(lossy(&common::bytes(v))),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), as_strings(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(as_strings).collect()),
        other => other.clone(),
    }
}

#[test]
fn skillparse_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("skillparse") {
        let input = &c["in"];
        match c["fn"].as_str().unwrap() {
            "parse" => {
                let got = match parse(&common::bytes(&input[0]), input[1].as_str().unwrap()) {
                    Ok(s) => json!([{"name": s.name, "description": s.description, "whenToUse": s.when_to_use,
                        "allowed": if s.has_allowed { json!(s.allowed_tools) } else { Value::Null }, "hasAllowed": s.has_allowed,
                        "user": s.user_invocable, "model": s.model_invocable, "body": s.body,
                        "prefetch": s.prefetch.as_ref().map_or(Value::Null, |p| json!(p)), "expanded": s.expand("ARGS $ARGUMENTS")}, ""]),
                    Err(e) => json!([Value::Null, e]),
                };
                check.eq(format!("parse {input}"), &got, &as_strings(&c["out"]));
            }
            "validName" => {
                check.eq(format!("validName {input}"), &json!(valid_name(&common::bytes(input))), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    check.done("skillparse");
}
