//! testdata/parity/mcpmanager.json: MCP's definitions loader on the same made-up files (the operator files,
//! SARINA_MCP_FILE, the workspace's only with the opt-in, the owner's signed list, every problem), and the manager
//! against the client fixture's shell stand-in server (Connect, the tools offered, Status, calls, the result bound, the
//! resource tools, a restart after the server exits).
//!
//! One test function: it sets HOME and the working folder, which the whole process shares.

mod common;

use sarina::goany::Any;
use sarina::goctx::Ctx;
use sarina::gojson;
use sarina::gopath;
use sarina::mcp::client::ServerDef;
use sarina::mcp::config::{load, Config, LoadOptions, ENV_FILE, SOURCE_OPERATOR};
use sarina::mcp::manager::{clean_description, exposed_name, truncate, Manager, Options};
use sarina::tools::registry::{Class, Context, Registry};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

fn num(f: f64) -> Value {
    if f.fract() == 0.0 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

fn def_out(d: &ServerDef, hide: &dyn Fn(&str) -> String) -> Value {
    let ro: Vec<&String> = d.read_only_tools.keys().collect();
    let env = if d.env.is_empty() {
        "null".to_owned()
    } else {
        gojson::marshal_any(&Any::Object(d.env.iter().map(|(k, v)| (k.clone(), Any::String(v.clone()))).collect())).unwrap()
    };
    json!([
        d.name,
        d.command,
        if d.args.is_empty() { Value::Null } else { json!(d.args) },
        env,
        hide(&d.cwd),
        d.description,
        d.read_only,
        ro,
        num(d.timeout_seconds),
        d.source,
        hide(&d.file)
    ])
}

fn null_if_empty(v: Vec<Value>) -> Value {
    if v.is_empty() {
        Value::Null
    } else {
        Value::Array(v)
    }
}

#[test]
fn mcp_manager_matches_go() {
    let cases = common::cases("mcpmanager");
    let script = common::cases("mcp").into_iter().find(|c| c["fn"] == "script").unwrap()["in"].as_str().unwrap().to_owned();
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-mcpmanager-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let hide = |s: &str| s.replace(&top, "$BASE");
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "tree" => {
                for e in c["in"].as_array().unwrap() {
                    let p = format!("{top}/{}", e[0].as_str().unwrap());
                    std::fs::create_dir_all(gopath::os_path(&gopath::dir(p.as_bytes()))).unwrap();
                    match e[1].as_str().unwrap() {
                        "DIR" => std::fs::create_dir_all(&p).unwrap(),
                        "BIG" => std::fs::write(&p, format!("{{\"servers\":[]}}{}", " ".repeat(256 << 10))).unwrap(),
                        content => std::fs::write(&p, content).unwrap(),
                    }
                }
                std::env::set_current_dir(&top).unwrap();
                std::env::set_var("PWD", &top);
                std::env::set_var("HOME", format!("{top}/home"));
            }
            "load" => {
                let env_files = [
                    format!("{top}/env1.json"),
                    "env2.json".into(),
                    format!("{top}/env3.json"),
                    String::new(),
                    format!("{top}/env4.json"),
                    format!("{top}/untrusted.json"),
                    format!("{top}/adir.json"),
                    format!("{top}/home/.sarina/mcp.json"),
                ]
                .join(":");
                let mut o = LoadOptions {
                    home: format!("{top}/home"),
                    getenv: Some(Arc::new(move |k: &str| if k == ENV_FILE { env_files.clone() } else { String::new() })),
                    workspace: format!("{top}/ws"),
                    trust: Some(Arc::new(
                        |p: &str| if p.contains("untrusted") { "not on the owner's signed list".into() } else { String::new() },
                    )),
                    ..LoadOptions::default()
                };
                match c["in"].as_str().unwrap() {
                    "trustws" => o.trust_workspace = true,
                    "notrust" => o.trust = None,
                    "nohome" => o.home = String::new(),
                    _ => {}
                }
                let cfg = load(&o);
                let got = json!([
                    null_if_empty(cfg.servers.iter().map(|d| def_out(d, &hide)).collect()),
                    null_if_empty(cfg.notices.iter().map(|n| json!(hide(n))).collect()),
                    null_if_empty(cfg.problems.iter().map(|n| json!(hide(n))).collect()),
                    null_if_empty(cfg.files.iter().map(|n| json!(hide(n))).collect())
                ]);
                check.eq(format!("load {}", c["in"]), &got, &c["out"]);
            }
            "manager" => {
                let server = format!("{top}/fake-mcp");
                std::fs::write(&server, &script).unwrap();
                std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
                let one_replies: Vec<String> = c["in"][0].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_owned()).collect();
                let init1 = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}}"#.to_owned();
                let srvs: Vec<(&str, &str, Vec<String>, ServerDef)> = vec![
                    (
                        "srv one",
                        "one",
                        one_replies,
                        ServerDef { read_only_tools: BTreeMap::from([("ro".into(), true)]), ..ServerDef::default() },
                    ),
                    ("srv-two", "two", vec![], ServerDef { command: format!("{top}/nothere"), ..ServerDef::default() }),
                    ("srv three", "three", vec![init1.clone(), "@exit:2".into()], ServerDef::default()),
                    (
                        "srv.one",
                        "dup",
                        vec![
                            init1.clone(),
                            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo"}]}}"#.into(),
                            r#"{"jsonrpc":"2.0","id":3,"result":{"resources":[{"uri":"file:///z","name":"z"}]}}"#.into(),
                            r#"{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"file:///alpha-z","name":"z2"}]}}"#.into(),
                            r#"{"jsonrpc":"2.0","id":5,"result":{"resources":[]}}"#.into(),
                        ],
                        ServerDef { read_only: true, timeout_seconds: 5.0, ..ServerDef::default() },
                    ),
                    (
                        "srv four",
                        "four",
                        vec![init1.clone(), r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"die"}]}}"#.into(), "@exit:0".into()],
                        ServerDef::default(),
                    ),
                    (
                        "srv five",
                        "five",
                        vec![r#"{"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"refused"}}"#.into()],
                        ServerDef::default(),
                    ),
                    (
                        "srv six",
                        "six",
                        vec![
                            init1.clone(),
                            r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"partial"}],"nextCursor":"p2"}}"#.into(),
                            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-1,"message":"page two failed"}}"#.into(),
                            r#"{"jsonrpc":"2.0","id":4,"result":{"resources":[]}}"#.into(),
                            r#"{"jsonrpc":"2.0","id":5,"result":{"resources":[]}}"#.into(),
                        ],
                        ServerDef::default(),
                    ),
                ];
                let mut cfg = Config::default();
                for (name, dir, replies, def) in &srvs {
                    let d = format!("{top}/srv/{dir}");
                    std::fs::create_dir_all(&d).unwrap();
                    std::fs::write(format!("{d}/replies"), replies.join("\n") + "\n").unwrap();
                    let mut def = def.clone();
                    def.name = name.to_string();
                    if def.command.is_empty() {
                        def.command = server.clone();
                    }
                    def.args = vec![d];
                    def.source = SOURCE_OPERATOR.into();
                    def.file = format!("{top}/mcp.json");
                    cfg.servers.push(def);
                }
                let mut m = Manager::new(
                    Some(&cfg),
                    Options { env: vec!["PATH=/usr/bin:/bin".into(), "FAKE_VAR=base".into()], max_result_chars: 300, ..Options::default() },
                );
                m.connect(&Ctx::background());
                let probs: Vec<Value> = m.problems.iter().map(|p| json!(hide(p))).collect();
                let status: Vec<Value> = m
                    .status()
                    .iter()
                    .map(|s| json!([s.name, hide(&s.command), s.source, hide(&s.file), s.tools, hide(&s.error), s.read_only]))
                    .collect();
                let ts = m.tools();
                let specs: Vec<Value> = ts
                    .iter()
                    .map(|t| {
                        let class = match t.class {
                            Class::ReadOnly => 1,
                            Class::Mutating => 2,
                            Class::PlanControl => 3,
                            Class::Unset => 0,
                        };
                        let params = gojson::marshal_any(&Any::Object(t.spec.parameters.clone().unwrap_or_default())).unwrap();
                        json!([t.spec.name, t.spec.description, params, class])
                    })
                    .collect();
                let reg = Registry::new(ts);
                let mut out = Vec::new();
                for (i, call) in c["in"][1].as_array().unwrap().iter().enumerate() {
                    let args = gojson::unmarshal_any(call[1].as_str().unwrap().as_bytes()).0.into_object().unwrap();
                    let tc = Context { max_output_chars: if i == 2 { 100 } else { 0 }, ..Context::default() };
                    let r = reg.execute(call[0].as_str().unwrap(), Some(args), &tc);
                    let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                    out.push(json!([r.ok, hide(&r.content), md]));
                }
                m.close();
                let requests: Vec<Value> = srvs
                    .iter()
                    .map(|(_, dir, _, _)| {
                        let b = std::fs::read(format!("{top}/srv/{dir}/requests")).unwrap_or_default();
                        json!([dir, hide(&String::from_utf8_lossy(&b))])
                    })
                    .collect();
                let want = &c["out"];
                check.eq("manager problems", &null_if_empty(probs), &want[0]);
                check.eq("manager status", &Value::Array(status), &want[1]);
                for (i, (g, w)) in specs.iter().zip(want[2].as_array().unwrap()).enumerate() {
                    check.eq(format!("manager spec {i}"), g, w);
                }
                check.eq("manager number of tools", &json!(specs.len()), &json!(want[2].as_array().unwrap().len()));
                for (i, (g, w)) in out.iter().zip(want[3].as_array().unwrap()).enumerate() {
                    check.eq(format!("manager call {i} {}", c["in"][1][i]), g, w);
                }
                for (i, (g, w)) in requests.iter().zip(want[4].as_array().unwrap()).enumerate() {
                    check.eq(format!("manager requests {i}"), g, w);
                }
            }
            "exposedName" => {
                // the cases run in order on one set of used names, as in Go
                thread_local! { static USED: std::cell::RefCell<HashSet<String>> = std::cell::RefCell::new(HashSet::new()); }
                let name = USED.with(|u| {
                    let n = exposed_name(c["in"][0].as_str().unwrap(), c["in"][1].as_str().unwrap(), &u.borrow());
                    u.borrow_mut().insert(n.clone());
                    n
                });
                check.eq(format!("exposedName {}", c["in"]), &json!(name), &c["out"]);
            }
            "cleanDescription" => {
                check.eq(format!("cleanDescription {}", c["in"]), &json!(clean_description(c["in"].as_str().unwrap())), &c["out"])
            }
            "Truncate" => {
                let got = truncate(c["in"][0].as_str().unwrap(), c["in"][1].as_u64().unwrap() as usize);
                check.eq(format!("Truncate {}", c["in"]), &json!(got), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    std::env::set_current_dir("/").unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("mcpmanager");
}
