//! testdata/parity/schema.json: Go's ValidateArgs on the builtin tools' schemas and generated ones, and
//! strconv.ParseFloat on number texts, compared exactly.

mod common;

use std::collections::BTreeMap;

use sarina::goany::Any;
use sarina::gotext::{float_v, parse_float, trim_space};
use sarina::tools::schema::validate_args;
use serde_json::{json, Value};

fn any(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Number(n) => Any::Number(n.as_f64().expect("number")),
        Value::String(s) => Any::String(s.clone()),
        Value::Array(a) => Any::Array(a.iter().map(any).collect()),
        Value::Object(m) => Any::Object(m.iter().map(|(k, x)| (k.clone(), any(x))).collect()),
    }
}

/// Every number as an f64, so 3 and 3.0 compare equal (Go writes a float64 3 as `3`).
fn canon(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().expect("number")),
        Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), canon(x))).collect()),
        other => other.clone(),
    }
}

fn value(a: &Any) -> Value {
    match a {
        Any::Null => Value::Null,
        Any::Bool(b) => json!(b),
        Any::Number(f) => json!(f),
        Any::String(s) => json!(s),
        Any::Array(xs) => Value::Array(xs.iter().map(value).collect()),
        Any::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), value(x))).collect()),
    }
}

fn object(v: &Value) -> Option<BTreeMap<String, Any>> {
    match any(v) {
        Any::Object(m) => Some(m),
        Any::Null => None,
        other => panic!("not an object: {other:?}"),
    }
}

#[test]
fn schema_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("schema") {
        match c["fn"].as_str().unwrap() {
            "ValidateArgs" => {
                let schema = object(&c["in"][0]);
                let args = object(&c["in"][1]).unwrap_or_default();
                let want = &c["out"];
                let got = match validate_args(schema.as_ref(), args) {
                    Ok(m) => json!({ "ok": canon(&value(&Any::Object(m))) }),
                    Err(e) => match want.get("errs") {
                        Some(Value::Array(es)) if es.contains(&json!(e)) => want.clone(),
                        _ => json!({ "err": e }),
                    },
                };
                let want = match want.get("ok") {
                    Some(ok) => json!({ "ok": canon(ok) }),
                    None => want.clone(),
                };
                check.eq(format!("ValidateArgs({}, {})", c["in"][0], c["in"][1]), &got, &want);
            }
            "ParseFloat(TrimSpace)" => {
                let s = c["in"].as_str().unwrap();
                let got = match parse_float(trim_space(s)) {
                    Ok(f) => json!([float_v(f), ""]),
                    Err(e) => json!([float_v(e.value), e.message]),
                };
                check.eq(format!("ParseFloat({s:?})"), &got, &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    check.done("schema");
}
