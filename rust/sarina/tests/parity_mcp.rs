//! testdata/parity/mcp.json: the MCP stdio client against the same shell stand-in server Go's test runs: the handshake,
//! listings with pages, tool calls and reads rendered as text, JSON-RPC errors, the server's own requests answered, noise
//! ignored, a timeout or cancel leaving the connection usable, a dying server's reason, the line bound, a flood of requests,
//! the child's environment and folder, Close; Spawn's errors and overlayEnv.

mod common;

use sarina::goany::Any;
use sarina::goctx::Ctx;
use sarina::mcp::client::{overlay_env, Client, ServerDef, MAX_LINE_BYTES, MAX_REPLIES, PROTOCOL_VERSION, STDERR_KEEP};
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The Go test's `short`.
fn short(s: &[u8]) -> Value {
    if s.len() <= 4096 {
        return common::text(s);
    }
    let h = sha2::Sha256::digest(s);
    json!([String::from_utf8_lossy(&s[..100]), s.len(), h.iter().map(|b| format!("{b:02x}")).collect::<String>()])
}

/// A fixture JSON value as Go's `any`.
fn any(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Number(n) => Any::Number(n.as_f64().unwrap()),
        Value::String(s) => Any::String(s.clone()),
        Value::Array(a) => Any::Array(a.iter().map(any).collect()),
        Value::Object(m) => Any::Object(m.iter().map(|(k, x)| (k.clone(), any(x))).collect()),
    }
}

#[test]
fn mcp_client_matches_go() {
    let mut check = common::Check::new();
    let cases = common::cases("mcp");
    let script = cases.iter().find(|c| c["fn"] == "script").unwrap();
    let root = tempdir();
    let server = format!("{root}/fake-mcp");
    write(&server, script["in"].as_str().unwrap().as_bytes(), 0o755);
    let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
    let clean = |s: &str| s.replace(&root, "{DIR}").replace(&format!("pwd={cwd}\n"), "pwd={CWD}\n");
    let base: Vec<String> = ["FAKE_VAR=base", "FAKE_KEEP=kept", "PATH=/usr/bin:/bin"].iter().map(|s| s.to_string()).collect();
    for c in &cases {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "script" => json!([MAX_LINE_BYTES, STDERR_KEEP, MAX_REPLIES, PROTOCOL_VERSION]),
            "overlay" => {
                let extra: BTreeMap<String, String> = input
                    .as_object()
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect())
                    .unwrap_or_default();
                let b: Vec<String> = ["A=1", "FAKE_VAR=base", "NOEQUALS", "=weird", "A=2"].iter().map(|s| s.to_string()).collect();
                json!(overlay_env(&b, &extra))
            }
            "spawn" => {
                let p = |i: usize| input[i].as_str().unwrap().replace("{DIR}", &root);
                if p(0) == "c" {
                    write(&format!("{root}/noexec"), b"#!/bin/sh\n", 0o644);
                }
                let def = ServerDef { name: p(0), command: p(1), cwd: p(2), ..ServerDef::default() };
                // Go's test sets PATH to a folder that does not exist, then the system's
                let old = std::env::var("PATH").unwrap_or_default();
                std::env::set_var("PATH", format!("{root}/bin:/usr/bin:/bin"));
                let r = Client::spawn(&def, &base);
                std::env::set_var("PATH", old);
                json!(clean(&r.err().unwrap_or_default()))
            }
            "client" => {
                let dir = format!("{root}/{}", input["dir"].as_str().unwrap());
                std::fs::create_dir_all(&dir).unwrap();
                let replies: Vec<&str> = input["replies"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
                write(&format!("{dir}/replies"), format!("{}\n", replies.join("\n")).as_bytes(), 0o644);
                if let Some(e) = input["stderr"].as_str() {
                    write(&format!("{dir}/stderr"), e.as_bytes(), 0o644);
                }
                if input["stubborn"].as_bool() == Some(true) {
                    write(&format!("{dir}/stubborn"), b"", 0o644);
                }
                let env: BTreeMap<String, String> = input["env"]
                    .as_object()
                    .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect())
                    .unwrap_or_default();
                let mut def = ServerDef {
                    name: format!("fake {}", input["dir"].as_str().unwrap()),
                    command: server.clone(),
                    args: vec![dir.clone()],
                    env,
                    ..ServerDef::default()
                };
                if let Some(cw) = input["cwd"].as_str() {
                    def.cwd = format!("{dir}/{cw}");
                    std::fs::create_dir_all(&def.cwd).unwrap();
                }
                let cl = Client::spawn(&def, &base).unwrap();
                let mut results = Vec::new();
                for op in input["ops"].as_array().unwrap() {
                    let ctx = Ctx::background();
                    if let Some(ms) = op["cancelMs"].as_u64() {
                        let c2 = ctx.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(ms));
                            c2.cancel();
                        });
                    }
                    let d = Duration::from_millis(op["timeMs"].as_u64().unwrap());
                    let name = op["name"].as_str().unwrap_or("");
                    let (out, err) = match op["op"].as_str().unwrap() {
                        "init" => (Value::Null, cl.initialize(&ctx, d).err()),
                        "tools" => {
                            let (list, err) = cl.list_tools(&ctx, d);
                            let o: Vec<Value> = list
                                .iter()
                                .map(|t| {
                                    let s = t.input_schema.as_ref().map_or(Value::Null, |m| common::typed(&Any::Object(m.clone())));
                                    json!([t.name, t.description, s])
                                })
                                .collect();
                            (if o.is_empty() { Value::Null } else { json!(o) }, err)
                        }
                        "resources" => {
                            let (list, err) = cl.list_resources(&ctx, d);
                            let o: Vec<Value> = list.iter().map(|r| json!([r.uri, r.name, r.description, r.mime_type])).collect();
                            (if o.is_empty() { Value::Null } else { json!(o) }, err)
                        }
                        "call" => {
                            let args = op.get("args").map(|a| any(a).into_object().unwrap_or_default());
                            match cl.call_tool(&ctx, name, args.as_ref(), d) {
                                Ok(r) => (json!([short(r.text.as_bytes()), r.is_error]), None),
                                Err(e) => (json!(["", false]), Some(e)),
                            }
                        }
                        "read" => match cl.read_resource(&ctx, op["uri"].as_str().unwrap(), d) {
                            Ok(s) => (short(s.as_bytes()), None),
                            Err(e) => (json!(""), Some(e)),
                        },
                        "raw" => {
                            let params = op.get("params").map(any);
                            match cl.call(&ctx, name, params.as_ref(), d) {
                                Ok(Some(raw)) => (short(&raw), None),
                                Ok(None) => (Value::Null, None),
                                Err(e) => (Value::Null, Some(e)),
                            }
                        }
                        other => panic!("op {other}"),
                    };
                    results.push(json!([out, clean(&err.unwrap_or_default())]));
                }
                let alive = cl.alive();
                let start = Instant::now();
                cl.close();
                let prompt = start.elapsed() < Duration::from_secs(3);
                let read = |n: &str| clean(&String::from_utf8_lossy(&std::fs::read(format!("{dir}/{n}")).unwrap_or_default()));
                let mut notes: Vec<String> = read("notes").trim().split('\n').map(str::to_string).collect();
                notes.sort();
                let notes_v = if input["dir"] == "flood" {
                    let flood =
                        notes.iter().filter(|n| n.starts_with(r#"{"id":"f"#) && n.ends_with(r#","jsonrpc":"2.0","result":{}}"#)).count();
                    json!(flood > 0 && flood < 5000)
                } else {
                    json!(notes)
                };
                json!({"results": results, "alive": alive, "prompt": prompt, "tail": common::text(cl.stderr_tail().as_bytes()),
                    "requests": short(read("requests").as_bytes()), "notes": notes_v, "env": read("env"), "dropped": cl.dropped() > 0})
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {}", c["fn"], input.get("dir").unwrap_or(input)), &got, &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&root);
    check.done("mcp");
}

fn tempdir() -> String {
    let d = std::env::temp_dir().join(format!("sarina-mcp-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.to_string_lossy().into_owned()
}

fn write(path: &str, b: &[u8], mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, b).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
