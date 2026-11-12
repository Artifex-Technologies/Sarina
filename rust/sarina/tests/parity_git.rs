//! testdata/parity/git.json: git_read against the same stand-in git (bash's builtins only, written from the fixture),
//! in the same workspaces: the arguments and variables given to git, the checks, the refusal of risky settings before
//! git runs at all, git's failures and empty answers, and the bounds.
//!
//! One test function: it sets PATH, HOME, MY_TOKEN and SARINA_PRIVATE_DIRS, which the whole process shares.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::gopath;
use sarina::tools::git::git_tool;
use sarina::tools::registry::{Context, Registry};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

#[test]
fn git_matches_go() {
    let cases = common::cases("git");
    let mut check = common::Check::new();
    let bash = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|d| format!("{d}/bash"))
        .find(|p| std::fs::metadata(p).is_ok())
        .expect("bash");
    let bash = String::from_utf8(gopath::eval_symlinks(bash.as_bytes()).unwrap()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let bin = format!("{top}/bin");
    std::fs::create_dir(&bin).unwrap();
    std::env::set_var("PATH", &bin);
    std::env::set_var("MY_TOKEN", "x");
    let environ: Vec<Vec<u8>> = [format!("PATH={bin}"), "MY_TOKEN=x".into(), "LANG=C".into()].into_iter().map(String::into_bytes).collect();
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "standin" => {
                let script = c["in"].as_str().unwrap().replacen("BASH", &bash, 1);
                std::fs::write(format!("{bin}/git"), script).unwrap();
                std::fs::set_permissions(format!("{bin}/git"), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            "scenario" => {
                let n = c["in"][0].as_u64().unwrap() as u8;
                let base = format!("{top}/s/{}", (b'a' + n) as char);
                let ws = format!("{base}/ws");
                for d in [ws.clone(), format!("{ws}/priv"), format!("{base}/out"), format!("{base}/home")] {
                    std::fs::create_dir_all(d).unwrap();
                }
                std::fs::write(format!("{ws}/a.txt"), "a\n").unwrap();
                for m in c["in"][1].as_array().unwrap() {
                    let m = m.as_str().unwrap();
                    if let Some((name, content)) = m.split_once('=') {
                        std::fs::write(format!("{ws}/{name}"), content).unwrap();
                    }
                }
                std::env::set_var("HOME", format!("{base}/home"));
                std::env::set_var("SARINA_PRIVATE_DIRS", format!("{ws}/priv"));
                let env = environ.clone();
                let tc = Context {
                    root: ws.clone(),
                    max_output_chars: 1500,
                    environ: Some(Arc::new(move || env.clone())),
                    session_env: BTreeMap::from([("S1".into(), "v1".into())]),
                    ..Context::default()
                };
                let reg = Registry::new([git_tool()]);
                let mut out = Vec::new();
                for call in c["in"][2].as_array().unwrap() {
                    let args = gojson::unmarshal_any(call.as_str().unwrap().as_bytes()).0.into_object().unwrap();
                    let r = reg.execute("git_read", Some(args), &tc);
                    let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                    out.push(json!([r.ok, r.content.replace(&top, "$BASE"), md]));
                }
                let marks: Vec<&str> = [".standin-ran-revparse", ".standin-revparse-saw-secret"]
                    .into_iter()
                    .filter(|m| std::fs::metadata(format!("{ws}/{m}")).is_ok())
                    .collect();
                let marks = if marks.is_empty() { Value::Null } else { json!(marks) };
                for (i, (got, want)) in out.iter().zip(c["out"][0].as_array().unwrap()).enumerate() {
                    check.eq(format!("scenario {n} call {i} {}", c["in"][2][i]), got, want);
                }
                check.eq(format!("scenario {n}: what ran"), &marks, &c["out"][1]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("git");
}
