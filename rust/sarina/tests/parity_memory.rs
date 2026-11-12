//! testdata/parity/memory.json: curated memory's tools through the registry, the files they write, the prompt blocks and
//! the summary, and the files that may not be used (a link out of the workspace, a linked folder, a link to a secret
//! file, a named pipe, one too large, a folder). The gate is the local checks only (no Heimdall address is set).
//!
//! One test function: it sets the Heimdall variables, which the whole process shares.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::gopath;
use sarina::memory::{tools, Runtime};
use sarina::tools::registry::{Context, Registry};
use serde_json::{json, Value};

#[test]
fn memory_matches_go() {
    std::env::set_var("HEIMDALL_REVIEW_URL", "");
    std::env::set_var("SENTRY_REVIEW_URL", "");
    let cases = common::cases("memory");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-memory-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let hide = |s: &str| s.replace(&top, "$BASE");
    let setup = |name: &str| -> (String, String) {
        let (ws, home) = (format!("{top}/{name}/ws"), format!("{top}/{name}/home"));
        std::fs::create_dir_all(format!("{ws}/.port_sessions/agent")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        (ws, home)
    };
    let read = |p: &str| String::from_utf8_lossy(&std::fs::read(p).unwrap_or_default()).into_owned();
    let outside = format!("{top}/outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(format!("{outside}/note"), "OUTSIDE NOTE").unwrap();
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "tools" => {
                let (ws, home) = setup("main");
                std::fs::write(
                    format!("{ws}/.port_sessions/agent/s1.json"),
                    r#"{"session_id":"s1","messages":[{"role":"user","content":"where is the blue widget?"},{"role":"assistant","content":"The blue widget is in the box."}]}"#,
                )
                .unwrap();
                let reg = Registry::new(tools());
                let mut tc = Context { root: ws.clone(), memory_home: home.clone(), ..Context::default() };
                tc.permissions.allow_file_write = true;
                let mut out = Vec::new();
                for call in c["in"].as_array().unwrap() {
                    let args = gojson::unmarshal_any(call[1].as_str().unwrap().as_bytes()).0.into_object().unwrap();
                    let r = reg.execute(call[0].as_str().unwrap(), Some(args), &tc);
                    let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                    out.push(json!([r.ok, hide(&r.content), hide(&md)]));
                }
                let rt = Runtime::from_workspace(&ws, &home);
                let mut blocks = vec![json!(rt.render_prompt_block()), json!(hide(&rt.render_summary()))];
                for (task, n) in [("widget blue build", 2), ("nothing", 1), ("", 3), ("go cargo build", 0), ("x", 50)] {
                    blocks.push(json!(rt.render_relevant_prompt_block(task, n)));
                }
                let want = &c["out"];
                for (i, (g, w)) in out.iter().zip(want[0].as_array().unwrap()).enumerate() {
                    check.eq(format!("call {i} {}", c["in"][i]), g, w);
                }
                check.eq("agent file", &json!(read(&rt.agent_path)), &want[1]);
                check.eq("user file", &json!(read(&rt.user_path)), &want[2]);
                check.eq("blocks", &Value::Array(blocks), &want[3]);
            }
            "file" => {
                let v = c["in"].as_str().unwrap();
                let (ws, home) = setup(v);
                let mem = format!("{ws}/.port_sessions/memory");
                let agent = format!("{mem}/AGENT_MEMORY.md");
                if v == "link-dir" {
                    std::os::unix::fs::symlink(&outside, &mem).unwrap();
                } else {
                    std::fs::create_dir_all(&mem).unwrap();
                }
                match v {
                    "link-out" => std::os::unix::fs::symlink(format!("{outside}/note"), &agent).unwrap(),
                    "link-secret" => {
                        std::fs::write(format!("{ws}/.env"), "KEY=1").unwrap();
                        std::os::unix::fs::symlink("../../.env", &agent).unwrap();
                    }
                    "fifo" => {
                        let p = std::ffi::CString::new(agent.clone()).unwrap();
                        assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o644) }, 0);
                    }
                    "big" => std::fs::write(&agent, format!("first\n---\n{}", "x".repeat(1 << 20))).unwrap(),
                    "dir" => std::fs::create_dir_all(&agent).unwrap(),
                    "plain" => std::fs::write(&agent, b"one\n---\n\n---\n two \xff\n---\n").unwrap(),
                    _ => {}
                }
                let rt = Runtime::from_workspace(&ws, &home);
                let (entries, err) = match rt.entries("agent") {
                    Ok(e) if e.is_empty() => (Value::Null, String::new()),
                    Ok(e) => (json!(e), String::new()),
                    Err(e) => (Value::Null, e),
                };
                let add_err = rt.add("agent", "a new fact").err().unwrap_or_default();
                let rm_err = rt.remove("agent", "one").err().unwrap_or_default();
                let after = match std::fs::symlink_metadata(&agent) {
                    Ok(m) if m.is_file() && m.len() < 4096 => common::text(&std::fs::read(&agent).unwrap()),
                    _ => Value::Null,
                };
                let got = json!([
                    entries,
                    hide(&err),
                    hide(&add_err),
                    hide(&rm_err),
                    after,
                    hide(&rt.render_prompt_block()),
                    read(&format!("{outside}/note"))
                ]);
                check.eq(format!("file {v}"), &got, &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("memory");
}
