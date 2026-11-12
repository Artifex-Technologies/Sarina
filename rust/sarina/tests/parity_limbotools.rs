//! testdata/parity/limbotools.json: the limbo_profiles and limbo_validate tools through the registry against the same
//! scripted agent socket: the listing and its empty forms, the timeout clamped, blank arguments, an infrastructure
//! failure as an error with its metadata, Limbo's errors, the shell gate, a missing socket.
//!
//! One test function: it sets SARINA_LIMBO_SOCKET, which the whole process shares.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::limbo::tools;
use sarina::tools::registry::{Context, Registry};
use serde_json::{json, Value};

#[test]
fn limbo_tools_match_go() {
    let mut check = common::Check::new();
    let reg = Registry::new(tools());
    for c in common::cases("limbotools") {
        match c["fn"].as_str().unwrap() {
            "specs" => {
                let specs: Vec<Value> = reg
                    .specs()
                    .iter()
                    .map(|sp| {
                        let params = gojson::marshal_any(&Any::Object(sp.parameters.clone().unwrap_or_default())).unwrap();
                        json!([sp.name, sp.description, params])
                    })
                    .collect();
                check.eq("specs", &Value::Array(specs), &c["out"]);
            }
            "call" => {
                let i = &c["in"];
                let dir = std::env::temp_dir().join(format!("lmt-{}-{}", std::process::id(), i[0]));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).unwrap();
                let sock = dir.join("agent.sock");
                let sp = sock.to_str().unwrap().to_owned();
                let replies: Vec<Vec<u8>> = i[3].as_array().map_or(Vec::new(), |a| a.iter().map(common::bytes).collect());
                let seen = if i[5].as_bool().unwrap() {
                    std::env::set_var("SARINA_LIMBO_SOCKET", "");
                    None
                } else {
                    std::env::set_var("SARINA_LIMBO_SOCKET", &sp);
                    Some(common::raw_unix_server(&sock, replies))
                };
                let args = gojson::unmarshal_any(i[2].as_str().unwrap().as_bytes()).0.into_object().unwrap();
                let mut tc = Context::default();
                tc.permissions.allow_shell_commands = !i[4].as_bool().unwrap();
                let r = reg.execute(i[1].as_str().unwrap(), Some(args), &tc);
                let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                let reqs = seen.map_or(Value::Null, |s| {
                    let v = s.lock().unwrap().clone();
                    if v.is_empty() {
                        Value::Null
                    } else {
                        Value::Array(v)
                    }
                });
                let got = json!([[r.ok, r.content.replace(&sp, "{SOCK}"), md], reqs]);
                check.eq(format!("call {}", i[0]), &got, &c["out"]);
                let _ = std::fs::remove_dir_all(&dir);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    check.done("limbotools");
}
