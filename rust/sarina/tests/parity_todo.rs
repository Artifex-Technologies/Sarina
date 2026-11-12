//! testdata/parity/todo.json: the to-do and plan tools through the registry on the same starting stores, every answer
//! with its metadata and the store files after each scenario; the time, content hashes and random ids written as
//! placeholders as the Go test writes them.

mod common;

use regex::Regex;
use sarina::goany::Any;
use sarina::gojson;
use sarina::gopath;
use sarina::tools::registry::{Context, Registry};
use sarina::tools::todo::todo_tools;
use serde_json::{json, Value};

fn hide(s: &str, top: &str) -> String {
    let time = Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z").unwrap();
    let sha = Regex::new(r"[0-9a-f]{64}").unwrap();
    let id = Regex::new(r"task_([0-9]+_)?[0-9a-f]{6}([0-9a-f]{4})?(?-u:\b)").unwrap();
    let s = s.replace(top, "$BASE");
    let s = time.replace_all(&s, "TIME");
    let s = sha.replace_all(&s, "SHA");
    id.replace_all(&s, "task_RAND").into_owned()
}

#[test]
fn todo_matches_go() {
    let cases = common::cases("todo");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-todo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    for c in &cases {
        let n = c["in"][0].as_u64().unwrap() as u8;
        let base = format!("{top}/{}", (b'a' + n) as char);
        let (ws, outside) = (format!("{base}/ws"), format!("{base}/outside"));
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let store = format!("{ws}/.port_sessions");
        for f in c["in"][1].as_array().unwrap() {
            match f[0].as_str().unwrap() {
                "portfile" => std::fs::write(&store, "x").unwrap(),
                "link" => std::os::unix::fs::symlink(&outside, &store).unwrap(),
                k => {
                    std::fs::create_dir_all(&store).unwrap();
                    std::fs::write(format!("{store}/{k}_runtime.json"), f[1].as_str().unwrap()).unwrap();
                }
            }
        }
        let mut tc = Context { root: ws.clone(), max_output_chars: 4000, ..Context::default() };
        tc.permissions.allow_file_write = true;
        let reg = Registry::new(todo_tools());
        let mut out = Vec::new();
        for call in c["in"][2].as_array().unwrap() {
            let args = gojson::unmarshal_any(call[1].as_str().unwrap().as_bytes()).0.into_object().unwrap();
            let r = reg.execute(call[0].as_str().unwrap(), Some(args), &tc);
            let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
            out.push(json!([r.ok, hide(&r.content, &top), hide(&md, &top)]));
        }
        let mut files = Vec::new();
        for dir in [&store, &outside] {
            for name in ["task_runtime.json", "plan_runtime.json"] {
                if let Ok(data) = std::fs::read(format!("{dir}/{name}")) {
                    let rel = format!("{dir}/{name}")[base.len() + 1..].to_owned();
                    files.push(json!([rel, hide(&String::from_utf8_lossy(&data), &top)]));
                }
            }
        }
        let files = if files.is_empty() { Value::Null } else { Value::Array(files) };
        for (i, (got, want)) in out.iter().zip(c["out"][0].as_array().unwrap()).enumerate() {
            check.eq(format!("scenario {n} call {i} {}", c["in"][2][i]), got, want);
        }
        check.eq(format!("scenario {n}: the store after"), &files, &c["out"][1]);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("todo");
}
