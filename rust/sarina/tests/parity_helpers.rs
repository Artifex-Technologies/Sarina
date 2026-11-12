//! testdata/parity/helpers.json: the helper client against the same shell stand-in for the interpreter the Go test runs
//! (start, classify, group, errors, damaged and over-long answers, a helper that dies, Close, the requests sent), and
//! Available and Passthrough.

mod common;

use sarina::goany::Any;
use sarina::helpers::{available, passthrough, Server, PATH_ENV, PYTHON_ENV};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

#[test]
fn helpers_matches_go() {
    let mut check = common::Check::new();
    let root = std::env::temp_dir().join(format!("sarina-helpers-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let rs = root.to_str().unwrap().to_string();
    let cases = common::cases("helpers");
    let scripts = &cases.iter().find(|c| c["fn"] == "scripts").unwrap()["in"];
    for (name, key) in [("python", "python"), ("nomodule", "nomodule")] {
        let p = root.join(name);
        std::fs::write(&p, scripts[key].as_str().unwrap()).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let py = format!("{rs}/python");
    let clean = |s: &str| s.replace(&rs, "{DIR}");
    for c in &cases {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "scripts" => continue,
            "server" => {
                let dir = root.join(input["dir"].as_str().unwrap());
                std::fs::create_dir_all(&dir).unwrap();
                let replies: Vec<String> =
                    input["replies"].as_array().unwrap().iter().map(|r| String::from_utf8(common::bytes(r)).unwrap()).collect();
                std::fs::write(dir.join("replies"), format!("{}\n", replies.join("\n"))).unwrap();
                let getenv = |k: &str| match k {
                    PYTHON_ENV => py.clone(),
                    PATH_ENV => String::new(),
                    other => std::env::var(other).unwrap_or_default(),
                };
                let r = Server::start_with(dir.to_str().unwrap(), input["maxLine"].as_u64().unwrap() as usize, &getenv);
                let mut out = serde_json::Map::new();
                match r {
                    Err(e) => {
                        out.insert("start".into(), json!(clean(&e)));
                    }
                    Ok(s) => {
                        out.insert("start".into(), json!(""));
                        out.insert("prototypes".into(), json!(s.prototypes));
                        let err = |e: Result<(), String>| e.err().map(|e| clean(&e)).unwrap_or_default();
                        let mut results = Vec::new();
                        let classify = |t: &str| match s.classify(t) {
                            // the fixture writes a whole number as Go's encoder does (1, not 1.0)
                            Ok((l, sc)) if sc.fract() == 0.0 && sc.abs() < 1e15 => json!([l, sc as i64, ""]),
                            Ok((l, sc)) => json!([l, sc, ""]),
                            Err(e) => json!(["", 0, clean(&e)]),
                        };
                        results.push(classify("draw <pictures> \u{e9}"));
                        let cap =
                            BTreeMap::from([("name".to_string(), Any::String("a.nfc".into())), ("size".to_string(), Any::Number(12.5))]);
                        results.push(match s.group(&[cap], 0.7) {
                            Ok(g) => json!([g, ""]),
                            Err(e) => json!([Value::Null, clean(&e)]),
                        });
                        for t in ["second", "third", "fourth"] {
                            results.push(classify(t));
                        }
                        out.insert("results".into(), json!(results));
                        out.insert("close".into(), json!(err(s.close())));
                    }
                }
                let reqs = std::fs::read(dir.join("requests")).unwrap_or_default();
                out.insert("requests".into(), common::text(&reqs));
                Value::Object(out)
            }
            "passthrough" => {
                let python = input[0].as_str().unwrap().replace("{DIR}", &rs);
                let args: Vec<String> = input[1].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
                std::env::set_var("FAKE_EXIT", input[2].as_str().unwrap());
                let getenv = |k: &str| match k {
                    PYTHON_ENV => python.clone(),
                    PATH_ENV => String::new(),
                    other => std::env::var(other).unwrap_or_default(),
                };
                let (mut sin, mut sout, mut serr) = (std::io::empty(), Vec::new(), Vec::new());
                let code = passthrough(&mut sin, &mut sout, &mut serr, &args, &getenv);
                let avail = available(&getenv).err().unwrap_or_default();
                json!([clean(&avail), code, String::from_utf8_lossy(&sout), clean(&String::from_utf8_lossy(&serr))])
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&root);
    check.done("helpers");
}
