//! testdata/parity/analysiscache.json: New over files of every shape, Get, Set's file byte for byte and its errors.

mod common;

use sarina::analysiscache::Cache;
use sarina::goany::Any;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

fn values() -> Vec<Any> {
    let o = |kv: Vec<(&str, Any)>| Any::Object(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<BTreeMap<_, _>>());
    vec![
        Any::Null,
        Any::Bool(true),
        Any::Number(1.5),
        Any::String("Graphics".into()),
        Any::String("é \u{2028} <tag> & \u{1F600}".into()),
        Any::Array(vec![Any::String("a".into()), Any::Number(1.5), Any::Null]),
        o(vec![("label", Any::String("Graphics".into())), ("score", Any::Number(0.62))]),
        o(vec![("z", o(vec![("b", Any::Array(vec![])), ("a", o(vec![]))]))]),
        Any::Number(1e21),
        Any::Number(1e-7),
        Any::Number(-0.0),
        Any::Number(100.0),
        Any::Number(f64::NAN),
        Any::Number(f64::INFINITY),
        Any::Number(12345678901234567890.0),
    ]
}

#[test]
fn analysiscache_matches_go() {
    let mut check = common::Check::new();
    let base = std::env::temp_dir().join(format!("sarina-cache-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let temp_name = regex::Regex::new(r"\.cache-[0-9]+\.tmp").unwrap();
    for (n, c) in common::cases("analysiscache").iter().enumerate() {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "cache" => {
                let dir = base.join(format!("c{n}")).join("sub");
                let path = dir.join("c.json");
                let ns = input["ns"].as_str().unwrap();
                if !input["file"].is_null() {
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(&path, common::bytes(&input["file"])).unwrap();
                }
                let pb = path.to_str().unwrap().as_bytes();
                let cache = Cache::new(pb, ns);
                let mut out = Vec::new();
                for content in ["hello", "x", "", "a"] {
                    let v = cache.get(content.as_bytes());
                    out.push(json!([v.is_some(), v.as_ref().map_or(Value::Null, common::typed)]));
                }
                let all = values();
                let vals = if input["file"].is_null() { &all[..] } else { &all[5..8] };
                for (j, v) in vals.iter().enumerate() {
                    let err = cache.set(["hello", "x", "é", ""][j % 4].as_bytes(), v.clone()).err().unwrap_or_default();
                    let raw = std::fs::read(&path).unwrap_or_default();
                    out.push(json!([err.replace(dir.to_str().unwrap(), "{DIR}"), common::text(&raw)]));
                }
                if let Ok(m) = std::fs::metadata(&path) {
                    out.push(json!(m.permissions().mode() & 0o777));
                }
                let back = Cache::new(pb, ns);
                for content in ["hello", "x", "é", ""] {
                    let v = back.get(content.as_bytes());
                    out.push(json!([v.is_some(), v.as_ref().map_or(Value::Null, common::typed)]));
                }
                json!(out)
            }
            "setFails" => {
                let b = base.join(format!("f{n}"));
                std::fs::create_dir_all(b.join("ro")).unwrap();
                std::fs::write(b.join("file"), b"").unwrap();
                std::fs::set_permissions(b.join("ro"), std::fs::Permissions::from_mode(0o555)).unwrap();
                let p = b.join(input.as_str().unwrap());
                let err = Cache::new(p.to_str().unwrap().as_bytes(), "ns").set(b"x", Any::Number(1.0)).err().unwrap_or_default();
                std::fs::set_permissions(b.join("ro"), std::fs::Permissions::from_mode(0o755)).unwrap();
                let err = temp_name.replace_all(&err, ".cache-N.tmp").into_owned();
                json!(err.replace(b.to_str().unwrap(), "{DIR}"))
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&base);
    check.done("analysiscache");
}
