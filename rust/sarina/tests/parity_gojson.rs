//! testdata/parity/gojson.json: Go 1.27's encoding/json (the v1 API on the v2 engine) reading documents into an `any`
//! (Unmarshal, one Decoder.Decode and More), writing them back (Marshal, an Encoder without HTML escaping, indented), and
//! writing float64s and strings.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::gotext::lossy;
use serde_json::{json, Map, Value};

/// The value as the fixture's typed form holds it (parity.Typed).
pub fn typed(v: &Any) -> Value {
    match v {
        Any::Null => Value::Null,
        Any::Bool(b) => Value::Bool(*b),
        Any::Number(f) => json!({ "f": format!("{:x}", f.to_bits()) }),
        Any::Num(n) => json!({ "n": n }),
        Any::Int(i) => json!({ "i": i.to_string() }),
        Any::String(s) => json!({ "s": common::text(s.as_bytes()) }),
        Any::Array(items) => Value::Array(items.iter().map(typed).collect()),
        Any::Object(m) => json!({ "o": m.iter().map(|(k, x)| (k.clone(), typed(x))).collect::<Map<_, _>>() }),
    }
}

fn err_text(e: &Option<gojson::Error>) -> String {
    e.as_ref().map(|e| e.0.clone()).unwrap_or_default()
}

fn marshal_out(v: &Any) -> Value {
    let mut out = Map::new();
    match gojson::marshal_any(v) {
        Ok(s) => {
            out.insert("html".into(), common::text(s.as_bytes()));
        }
        Err(e) => {
            out.insert("err".into(), json!(e.0));
        }
    }
    let mut plain = String::new();
    if gojson::write_any(&mut plain, v, false).is_ok() {
        out.insert("indent".into(), common::text(gojson::indent(&plain, "  ").as_bytes()));
        out.insert("plain".into(), common::text(plain.as_bytes()));
    }
    Value::Object(out)
}

#[test]
fn gojson_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("gojson") {
        let input = &c["in"];
        match c["fn"].as_str().unwrap() {
            "doc" if input.get("objects").is_some() || input.get("arrays").is_some() => {
                let doc = match input.get("objects") {
                    Some(n) => {
                        let n = n.as_u64().unwrap() as usize;
                        format!("{}1{}", "{\"a\":".repeat(n), "}".repeat(n))
                    }
                    None => {
                        let n = input["arrays"].as_u64().unwrap() as usize;
                        format!("{}{}", "[".repeat(n), "]".repeat(n))
                    }
                };
                // on a thread with a small, known stack: reading, converting and dropping 10,000 levels must not recurse
                let (u, d) = std::thread::Builder::new()
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        let (_, u) = gojson::unmarshal_any(doc.as_bytes());
                        (u, gojson::parse_first(doc.as_bytes()).0.err().map(|e| e.0).unwrap_or_default())
                    })
                    .unwrap()
                    .join()
                    .unwrap();
                check.eq(format!("nesting {input}"), &json!({"unmarshal": err_text(&u), "decode": d}), &c["out"]);
            }
            "doc" => {
                let doc = common::bytes(input);
                let (v, e) = gojson::unmarshal_any(&doc);
                let mut out = Map::new();
                out.insert("unmarshal".into(), json!({"v": typed(&v), "err": err_text(&e)}));
                if e.is_none() {
                    out.insert("marshal".into(), marshal_out(&v));
                }
                let (r, more) = gojson::parse_first(&doc);
                let (dv, de) = match r {
                    Ok(v) => gojson::to_any(&v),
                    Err(e) => (Any::Null, Some(e)),
                };
                out.insert("decode".into(), json!({"v": typed(&dv), "err": err_text(&de), "more": more}));
                check.eq(format!("doc {input}"), &Value::Object(out), &c["out"]);
            }
            "float" => {
                let f = f64::from_bits(u64::from_str_radix(input.as_str().unwrap(), 16).unwrap());
                let mut s = String::new();
                let got = match gojson::write_f64(&mut s, f) {
                    Ok(()) => json!({"json": s, "err": ""}),
                    Err(e) => json!({"json": "", "err": e.0}),
                };
                check.eq(format!("float {f:e}"), &got, &c["out"]);
            }
            "string" => {
                let s = lossy(&common::bytes(input));
                check.eq(format!("string {input}"), &marshal_out(&Any::String(s)), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    check.done("gojson");
}
