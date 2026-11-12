//! testdata/parity/bash.json: the bash tool and the background-process tools through the registry, with the same
//! commands (bash's builtins only; PATH holds nothing but bash, so a refused command could find nothing to run), the same
//! environment and the same calls: the guards, the child's environment, the answers, streaming, the timeout, a cancelled
//! call, background processes; and the child environment's layers.
//!
//! One test function: it sets PATH and HOME, which the whole process shares.

mod common;

use sarina::goany::Any;
use sarina::goctx::Ctx;
use sarina::gojson;
use sarina::gopath;
use sarina::tools::exec::{child_env, shell_tools, ProcManager};
use sarina::tools::files::file_tools;
use sarina::tools::registry::{Context, Emit, Registry};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn untyped(v: &Value) -> Any {
    gojson::unmarshal_any(v.to_string().as_bytes()).0
}

/// Task ids and process ids as the Go test writes them.
fn hide(s: &str, top: &str) -> String {
    let s = s.replace(top, "$BASE");
    let mut out = String::new();
    let mut rest = s.as_str();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("bg_") {
            if r.len() >= 8 && r.as_bytes()[..8].iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c)) {
                out.push_str("bg_ID");
                rest = &r[8..];
                continue;
            }
        }
        for (pre, rep, end) in [("(pid ", "(pid N", ")"), ("\"pid\":", "\"pid\":0", "")] {
            if let Some(r) = rest.strip_prefix(pre) {
                let n = r.bytes().take_while(u8::is_ascii_digit).count();
                if n > 0 && r[n..].starts_with(end) {
                    out.push_str(rep);
                    rest = &r[n..];
                    continue;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

#[test]
fn bash_matches_go() {
    let cases = common::cases("bash");
    let mut check = common::Check::new();
    // bash as Go's LookPath finds it on the PATH this test starts with
    let real = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|d| format!("{d}/bash"))
        .find(|p| std::fs::metadata(p).is_ok())
        .expect("bash");
    let real = String::from_utf8(gopath::eval_symlinks(real.as_bytes()).unwrap()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-bash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let (bin, ws, home) = (format!("{top}/bin"), format!("{top}/ws"), format!("{top}/home"));
    for d in [&bin, &ws, &home] {
        std::fs::create_dir(d).unwrap();
    }
    std::os::unix::fs::symlink(&real, format!("{bin}/bash")).unwrap();
    std::env::set_var("PATH", &bin);
    std::env::set_var("HOME", &home);
    let environ: Vec<Vec<u8>> = [
        format!("PATH={bin}"),
        format!("HOME={home}"),
        "LANG=C".into(),
        "LC_ALL=C".into(),
        "MY_TOKEN=x".into(),
        "GIT_AUTHOR_NAME=y".into(),
        "SAFE=1".into(),
        "=bad".into(),
        "NOEQUALS".into(),
        "A=1=2".into(),
    ]
    .into_iter()
    .map(String::into_bytes)
    .collect();
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "scenario" => {
                let n = c["in"][0].as_u64().unwrap();
                let env = environ.clone();
                let mut tc = Context {
                    root: ws.clone(),
                    max_output_chars: 1500,
                    command_timeout: Duration::from_secs(5),
                    environ: Some(Arc::new(move || env.clone())),
                    session_env: BTreeMap::from([("S1".into(), "v1".into())]),
                    extra_env: BTreeMap::from([("SAFE".into(), "2".into())]),
                    procs: Some(Arc::new(ProcManager::default())),
                    ..Context::default()
                };
                tc.permissions.allow_file_write = true;
                tc.permissions.allow_shell_commands = true;
                let mut tools = shell_tools();
                tools.extend(file_tools().into_iter().filter(|t| t.spec.name == "write_file"));
                let reg = Registry::new(tools);
                let mut last_id = String::new();
                for (i, (call, want)) in c["in"][1].as_array().unwrap().iter().zip(c["out"].as_array().unwrap()).enumerate() {
                    let args: Value = serde_json::from_str(&call[1].as_str().unwrap().replace("$ID", &last_id)).unwrap();
                    let args = untyped(&args).into_object().unwrap();
                    let events: Arc<Mutex<BTreeMap<String, Vec<String>>>> = Arc::default();
                    let rec = events.clone();
                    let emit: Emit = Arc::new(move |s, t| rec.lock().unwrap().entry(s.to_owned()).or_default().push(t.to_owned()));
                    let name = call[0].as_str().unwrap();
                    let r = match call[2].as_str().unwrap() {
                        "stream" => reg.execute_stream(name, Some(args), &tc, &emit),
                        "cancel" => {
                            let ctx = Ctx::with_deadline(Instant::now() + Duration::from_millis(300));
                            let tc2 = Context { ctx, ..tc.clone() };
                            reg.execute(name, Some(args), &tc2)
                        }
                        _ => reg.execute(name, Some(args), &tc),
                    };
                    if let (Some(Any::String(id)), "bash") = (r.metadata.get("task_id"), name) {
                        last_id = id.clone();
                    }
                    let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                    let ev: Vec<Value> =
                        events.lock().unwrap().iter().rev().filter(|(_, v)| !v.is_empty()).map(|(s, v)| json!([s, v])).collect();
                    let ev = if ev.is_empty() { Value::Null } else { Value::Array(ev) };
                    let got = json!([r.ok, hide(&r.content, &top), hide(&md, &top), ev]);
                    check.eq(format!("scenario {n} call {i} {}", call), &got, want);
                }
                tc.procs.as_ref().unwrap().stop_all();
            }
            "childEnv" => {
                let env: Vec<Vec<u8>> = c["in"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().as_bytes().to_vec()).collect();
                let tc = Context {
                    environ: Some(Arc::new(move || env.clone())),
                    session_env: BTreeMap::from([("S".into(), "s".into()), ("PASSWORD_X".into(), "kept".into())]),
                    extra_env: BTreeMap::from([("A".into(), "extra".into())]),
                    ..Context::default()
                };
                let got: Vec<Value> = child_env(&tc).iter().map(|b| common::text(b)).collect();
                check.eq(format!("childEnv({})", c["in"]), &Value::Array(got), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("bash");
}
