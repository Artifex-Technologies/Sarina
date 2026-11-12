//! testdata/parity/fsutil.json: ResolveInRoot, ResolveForWrite and relPath on the same made-up tree for several workspace
//! roots, and TruncateOutput and preview.
//!
//! One test function: it changes HOME and the working folder, which the whole process shares.

mod common;

use sarina::gopath;
use sarina::tools::fsutil::{preview, rel_path, resolve_for_write, resolve_in_root, truncate_output};
use serde_json::{json, Value};

#[test]
fn fsutil_matches_go() {
    let cases = common::cases("fsutil");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-fsutil-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let base = format!("{top}/t");
    let hide = |b: &[u8]| String::from_utf8_lossy(b).replace(&base, "$BASE").replace(&top, "$TOP");
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "tree" => {
                for e in c["in"].as_array().unwrap() {
                    let rel = e[0].as_str().unwrap();
                    let p = if rel.is_empty() { base.clone() } else { format!("{base}/{rel}") };
                    match e[1].as_str().unwrap() {
                        "dir" => std::fs::create_dir(&p).unwrap(),
                        "file" => std::fs::write(&p, "x").unwrap(),
                        "link" => std::os::unix::fs::symlink(e[2].as_str().unwrap().replace("$BASE", &base), &p).unwrap(),
                        k => panic!("{k}"),
                    }
                }
                std::env::set_var("HOME", format!("{base}/home"));
                std::env::set_current_dir(&base).unwrap();
                std::env::set_var("PWD", &base);
            }
            "resolve" => {
                let root = c["in"][0].as_str().unwrap().replace("$BASE", &base);
                let mut out = Vec::new();
                for p in c["in"][1].as_array().unwrap() {
                    let p = p.as_str().unwrap().replace("$BASE", &base);
                    let split = |r: Result<Vec<u8>, String>| match r {
                        Ok(v) => (v, String::new()),
                        Err(e) => (Vec::new(), e),
                    };
                    let (inr, e1) = split(resolve_in_root(&root, &p));
                    let (w, e2) = split(resolve_for_write(&root, &p));
                    out.push(json!([
                        hide(&inr),
                        hide(e1.as_bytes()),
                        hide(&w),
                        hide(e2.as_bytes()),
                        hide(&rel_path(&root, &w)),
                        hide(&rel_path(&root, p.as_bytes()))
                    ]));
                }
                check.eq(format!("resolve(root {root})"), &Value::Array(out), &c["out"]);
            }
            "TruncateOutput" => {
                let got = truncate_output(c["in"][0].as_str().unwrap(), c["in"][1].as_i64().unwrap());
                check.eq(format!("TruncateOutput({})", c["in"]), &json!(got), &c["out"]);
            }
            "preview" => {
                let got = preview(c["in"][0].as_str().unwrap(), c["in"][1].as_u64().unwrap() as usize);
                check.eq(format!("preview({})", c["in"]), &json!(got), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    std::env::set_current_dir("/").unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("fsutil");
}
