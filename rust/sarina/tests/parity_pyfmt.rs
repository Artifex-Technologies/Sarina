//! testdata/parity/pyfmt.json: Go's pyfmt (Truthy, Str, Repr, TypeName) on JSON values decoded with UseNumber and without,
//! and ReprString.

mod common;

use sarina::goany::Any;
use sarina::gotext::lossy;
use sarina::pyfmt;
use serde_json::{json, Value};

/// The value as Go's decoder gave it, rebuilt from the fixture's typed form (a json.Number's text, a float64's bits).
fn any(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Array(a) => Any::Array(a.iter().map(any).collect()),
        Value::Object(m) if m.contains_key("n") => Any::Num(m["n"].as_str().unwrap().to_owned()),
        Value::Object(m) if m.contains_key("f") => Any::Number(f64::from_bits(u64::from_str_radix(m["f"].as_str().unwrap(), 16).unwrap())),
        Value::Object(m) if m.contains_key("s") => Any::String(lossy(&common::bytes(&m["s"]))),
        Value::Object(m) => Any::Object(m["o"].as_object().unwrap().iter().map(|(k, x)| (k.clone(), any(x))).collect()),
        other => panic!("{other}"),
    }
}

#[test]
fn pyfmt_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("pyfmt") {
        if c["fn"] == "ReprString" {
            let s = lossy(&common::bytes(&c["in"]));
            check.eq(format!("ReprString({s:?})"), &common::text(pyfmt::repr_string(&s).as_bytes()), &c["out"]);
            continue;
        }
        for key in ["number", "float"] {
            let want = &c["out"][key];
            let got = match want.get("value") {
                // Go's decoder refuses a float64 out of range (1e400)
                None => want.clone(),
                Some(tv) => {
                    let v = any(tv);
                    json!({
                        "value": tv,
                        "truthy": pyfmt::truthy(&v),
                        "str": common::text(pyfmt::str(&v).as_bytes()),
                        "repr": common::text(pyfmt::repr(&v).as_bytes()),
                        "type": pyfmt::type_name(&v),
                    })
                }
            };
            check.eq(format!("{key} {}", c["in"]), &got, want);
        }
    }
    check.done("pyfmt");
}
