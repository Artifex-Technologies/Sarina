//! testdata/parity/files.json: the file tools (read_file, list_dir, glob_search, grep_search, write_file, edit_file) through
//! the registry on the same made-up tree, every answer with its metadata and the workspace after each scenario.
//!
//! One test function: it sets HOME and SARINA_PRIVATE_DIRS, which the whole process shares.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::gopath;
use sarina::tools::files::file_tools;
use sarina::tools::registry::{Context, Registry};
use serde_json::{json, Value};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};

fn make_tree(base: &str, tree: &[Value]) {
    for e in tree {
        let p = format!("{base}/{}", e[0].as_str().unwrap());
        let data = common::bytes(&e[2]);
        match e[1].as_str().unwrap() {
            "dir" => std::fs::create_dir(&p).unwrap(),
            "file" => {
                std::fs::write(&p, &data).unwrap();
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
            "link" => std::os::unix::fs::symlink(String::from_utf8(data).unwrap(), &p).unwrap(),
            "hard" => std::fs::hard_link(format!("{base}/{}", String::from_utf8(data).unwrap()), &p).unwrap(),
            "fifo" => {
                let c = std::ffi::CString::new(p.clone()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo {p}");
            }
            k => panic!("{k}"),
        }
    }
}

/// The workspace in lexical order as Go's WalkDir lists it.
fn snapshot(ws: &str) -> Value {
    let mut out = Vec::new();
    gopath::walk_dir(ws.as_bytes(), &mut |p, _, _| {
        if p == ws.as_bytes() {
            return gopath::Walk::Continue;
        }
        let rel = String::from_utf8(p[ws.len() + 1..].to_vec()).unwrap();
        let m = std::fs::symlink_metadata(String::from_utf8_lossy(p).as_ref()).unwrap();
        let mode = format!("{:o}", m.permissions().mode() & 0o777);
        let t = m.file_type();
        out.push(if t.is_dir() {
            json!([rel, "dir", mode, ""])
        } else if t.is_fifo() {
            json!([rel, "fifo", mode, ""])
        } else if t.is_symlink() {
            json!([rel, "link", "", std::fs::read_link(String::from_utf8_lossy(p).as_ref()).unwrap().to_str().unwrap()])
        } else {
            json!([rel, "file", mode, common::text(&std::fs::read(String::from_utf8_lossy(p).as_ref()).unwrap())])
        });
        gopath::Walk::Continue
    })
    .unwrap();
    Value::Array(out)
}

fn untyped(v: &Value) -> Any {
    gojson::unmarshal_any(v.to_string().as_bytes()).0
}

#[test]
fn files_match_go() {
    let cases = common::cases("files");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let mut tree = Vec::new();
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "tree" => tree = c["in"].as_array().unwrap().clone(),
            "scenario" => {
                let n = c["in"][0].as_u64().unwrap();
                let base = format!("{top}/{n}");
                std::fs::create_dir(&base).unwrap();
                make_tree(&base, &tree);
                std::env::set_var("HOME", format!("{base}/home"));
                std::env::set_var("SARINA_PRIVATE_DIRS", format!("{base}/ws/priv"));
                let ws = format!("{base}/ws");
                let reg = Registry::new(file_tools());
                let mut tc = Context { root: ws.clone(), max_output_chars: 1500, ..Context::default() };
                tc.permissions.allow_file_write = true;
                let mut out = Vec::new();
                for call in c["in"][1].as_array().unwrap() {
                    let args: Value = serde_json::from_str(call[1].as_str().unwrap()).unwrap();
                    tc.step = call[2].as_str().unwrap().parse().unwrap();
                    let r = reg.execute(call[0].as_str().unwrap(), Some(untyped(&args).into_object().unwrap()), &tc);
                    let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap().replace(&top, "$BASE");
                    out.push(json!([r.ok, r.content.replace(&top, "$BASE"), md]));
                }
                let want = &c["out"];
                for (i, (got, w)) in out.iter().zip(want[0].as_array().unwrap()).enumerate() {
                    // a pattern Go's regexp refuses is refused here too, in the regex crate's words (docs/RUST_PORT.md)
                    if w[1].as_str().is_some_and(|t| t.starts_with("invalid pattern: error parsing regexp: ")) {
                        let refused = got[0] == false && got[1].as_str().is_some_and(|t| t.starts_with("invalid pattern: "));
                        check.eq(format!("scenario {n} call {i}: refused"), &json!(refused), &json!(true));
                        continue;
                    }
                    check.eq(format!("scenario {n} call {i} {}", c["in"][1][i]), got, w);
                }
                check.eq(format!("scenario {n}: number of calls"), &json!(out.len()), &json!(want[0].as_array().unwrap().len()));
                check.eq(format!("scenario {n}: the workspace after"), &snapshot(&ws), &want[1]);
            }
            "pathMatch" => {
                let pat = common::bytes(&c["in"][0]);
                let got: Vec<Value> = c["in"][1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| match gopath::path_match(&pat, &common::bytes(n)) {
                        Ok(m) => json!([m, false]),
                        Err(_) => json!([false, true]),
                    })
                    .collect();
                check.eq(format!("path.Match({})", c["in"][0]), &Value::Array(got), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("files");
}
