//! testdata/parity/testguard.json: on the same made-up folder tree, Go's filepath.EvalSymlinks, Abs, Clean, Join, Dir and
//! Base, testguard's resolve, and Allow under each combination of SARINA_TEST, SARINA_TEST_ROOT and TMPDIR.
//!
//! One test function: it changes the working folder and the environment, which the whole process shares.

mod common;

use std::os::unix::fs::symlink;

use sarina::{gopath, testguard};
use serde_json::{json, Value};

fn path_in(base: &str, p: &str) -> String {
    if let Some(rest) = p.strip_prefix("rel:") {
        rest.to_owned()
    } else if p.starts_with('/') {
        p.to_owned()
    } else if p.is_empty() {
        base.to_owned()
    } else {
        format!("{base}/{p}")
    }
}

#[test]
fn testguard_matches_go() {
    let cases = common::cases("testguard");
    let tmp = std::env::temp_dir().join(format!("sarina-parity-testguard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let base = format!("{top}/tree");
    std::fs::create_dir(&base).unwrap();
    assert_eq!(cases[0]["fn"], "tree");
    for e in cases[0]["in"].as_array().unwrap() {
        let p = format!("{base}/{}", e[0].as_str().unwrap());
        match e[1].as_str().unwrap() {
            "dir" => std::fs::create_dir(&p).unwrap(),
            "file" => std::fs::write(&p, "x").unwrap(),
            "link" => symlink(e[2].as_str().unwrap().replace("$BASE", &base), &p).unwrap(),
            k => panic!("{k}"),
        }
    }
    std::env::set_current_dir(&base).unwrap();
    std::env::set_var("PWD", &base); // Go's t.Chdir sets it too
    let hide = |b: &[u8]| String::from_utf8_lossy(b).replace(&base, "$BASE").replace(&top, "$TOP");
    let paths: Vec<String> = cases.iter().filter(|c| c["fn"] == "EvalSymlinks").map(|c| c["in"].as_str().unwrap().to_owned()).collect();
    let mut check = common::Check::new();
    for c in &cases[1..] {
        let f = c["fn"].as_str().unwrap();
        let got = match f {
            "EvalSymlinks" | "Abs" | "resolve" | "EvalSymlinks via link" | "Abs via link" | "resolve via link" => {
                let p = path_in(&base, c["in"].as_str().unwrap());
                if let Some(plain) = f.strip_suffix(" via link") {
                    // the working folder reached through a link, as the Go test does it (t.Chdir sets $PWD)
                    let dir = format!("{base}/link-ws/sub");
                    std::env::set_current_dir(&dir).unwrap();
                    std::env::set_var("PWD", &dir);
                    let got = match plain {
                        "EvalSymlinks" => {
                            gopath::eval_symlinks(p.as_bytes()).map(|r| json!([hide(&r), false])).unwrap_or(json!(["", true]))
                        }
                        "Abs" => gopath::abs(p.as_bytes()).map(|r| json!([hide(&r), false])).unwrap_or(json!(["", true])),
                        _ => json!(hide(&testguard::resolve(p.as_bytes()))),
                    };
                    std::env::set_current_dir(&base).unwrap();
                    std::env::set_var("PWD", &base);
                    check.eq(format!("{f}({})", c["in"]), &got, &c["out"]);
                    continue;
                }
                match f {
                    "EvalSymlinks" => match gopath::eval_symlinks(p.as_bytes()) {
                        Ok(r) => json!([hide(&r), false]),
                        Err(_) => json!(["", true]),
                    },
                    "Abs" => match gopath::abs(p.as_bytes()) {
                        Ok(r) => json!([hide(&r), false]),
                        Err(_) => json!(["", true]),
                    },
                    _ => json!(hide(&testguard::resolve(p.as_bytes()))),
                }
            }
            "Clean" => common::text(&gopath::clean(&common::bytes(&c["in"]))),
            "Dir" => common::text(&gopath::dir(&common::bytes(&c["in"]))),
            "Base" => common::text(&gopath::base(&common::bytes(&c["in"]))),
            "Join" => common::text(&gopath::join(&[&common::bytes(&c["in"][0]), &common::bytes(&c["in"][1])])),
            "Allow" => {
                let env = |name: &str, i: usize| {
                    let v = c["in"][i].as_str().unwrap().replace("$BASE", &base);
                    std::env::set_var(name, v);
                };
                env(testguard::ENV, 0);
                env(testguard::ROOT_ENV, 1);
                env("TMPDIR", 2);
                let out: Vec<Value> = paths
                    .iter()
                    .map(|p| json!(hide(testguard::allow(path_in(&base, p).as_bytes()).err().unwrap_or_default().as_bytes())))
                    .collect();
                Value::Array(out)
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{f}({})", c["in"]), &got, &c["out"]);
    }
    std::env::set_current_dir("/").unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("testguard");
}
