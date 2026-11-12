//! testdata/parity/checks.json: the checks after a code change and check_code, against the same stand-in for every program
//! (bash's builtins only, written from the fixture; nothing real is built, tested or copied): verifyMode over the settings,
//! the bubblewrap command line as data, verifyRun for each language, failure and mode, check_code, and write_file /
//! edit_file on code files.
//!
//! One test function: it sets PATH, HOME, TMPDIR and the working folder, which the whole process shares.

mod common;

use regex::Regex;
use sarina::goany::Any;
use sarina::gojson;
use sarina::gopath;
use sarina::tools::files::file_tools;
use sarina::tools::registry::{Context, Registry};
use sarina::tools::verify::{check_code_tool, sandbox_args, verify_mode_with, verify_run};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect()).unwrap_or_default()
}

#[test]
fn checks_match_go() {
    let cases = common::cases("checks");
    let mut check = common::Check::new();
    let bash = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|d| format!("{d}/bash"))
        .find(|p| std::fs::metadata(p).is_ok())
        .expect("bash");
    let bash = String::from_utf8(gopath::eval_symlinks(bash.as_bytes()).unwrap()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-checks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let sandbox_re = Regex::new("sarina-sandbox-[0-9]+").unwrap();
    let hide = |s: &str| sandbox_re.replace_all(&s.replace(&top, "$BASE"), "sarina-sandbox-N").into_owned();
    let (bin, binbw) = (format!("{top}/bin"), format!("{top}/binbw"));
    let mode: Arc<Mutex<String>> = Arc::default();
    let make_tc = |shell: bool| {
        let mode = mode.clone();
        let mut tc = Context {
            root: format!("{top}/ws"),
            max_output_chars: 4000,
            environ: Some(Arc::new(move || {
                vec![
                    format!("PATH={}", std::env::var("PATH").unwrap_or_default()).into_bytes(),
                    b"MY_TOKEN=x".to_vec(),
                    format!("STANDIN_MODE={}", mode.lock().unwrap()).into_bytes(),
                ]
            })),
            ..Context::default()
        };
        tc.permissions.allow_file_write = true;
        tc.permissions.allow_shell_commands = shell;
        tc
    };
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "verifyMode" => {
                let i = &c["in"];
                let env: HashMap<&str, String> = HashMap::from([
                    ("SARINA_VERIFY_TESTS", i[1].as_str().unwrap().to_owned()),
                    ("SARINA_SANDBOX_TESTS", i[2].as_str().unwrap().to_owned()),
                    ("SARINA_ACCESS_TIER", i[3].as_str().unwrap().to_owned()),
                ]);
                let getenv = |k: &str| env.get(k).cloned().unwrap_or_default();
                let tier = || sarina::config::access_tier_from_env_with(&getenv, sarina::heimdall::Grants::default);
                let (a, b, d) = verify_mode_with(i[0].as_bool().unwrap(), &getenv, i[4].as_bool().unwrap(), &tier);
                check.eq(format!("verifyMode {i}"), &json!([a, b, d]), &c["out"]);
            }
            "sandboxArgs" => {
                let i = &c["in"];
                let exists = strs(&i[1]);
                let got = sandbox_args(
                    "/w/p",
                    "/t/x/project",
                    "/t/x",
                    "/h",
                    "/h/.cache/sarina/sandbox-go",
                    i[0].as_str().unwrap(),
                    &strs(&i[2]),
                    &|p| exists.iter().any(|e| e == p),
                );
                check.eq(format!("sandboxArgs {i}"), &json!(got), &c["out"]);
            }
            "tree" => {
                for e in c["in"].as_array().unwrap() {
                    let p = format!("{top}/{}", e[0].as_str().unwrap());
                    std::fs::create_dir_all(gopath::os_path(&gopath::dir(p.as_bytes()))).unwrap();
                    std::fs::write(&p, e[1].as_str().unwrap()).unwrap();
                }
            }
            "standin" => {
                let script = c["in"].as_str().unwrap().replacen("BASH", &bash, 1);
                for d in [&bin, &binbw, &format!("{top}/home"), &format!("{top}/tmp")] {
                    std::fs::create_dir_all(d).unwrap();
                }
                for name in ["go", "gofmt", "python3", "bash", "cargo", "shellcheck", "cp", "bwrap"] {
                    for d in [&bin, &binbw] {
                        if name == "bwrap" && *d == bin {
                            continue;
                        }
                        let p = format!("{d}/{name}");
                        std::fs::write(&p, &script).unwrap();
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
                    }
                }
                std::env::set_current_dir(&top).unwrap();
                std::env::set_var("PWD", &top);
                for k in ["GOPROXY", "GOFLAGS", "GOTOOLCHAIN", "CARGO_NET_OFFLINE", "NO_COLOR", "MY_TOKEN", "STANDIN_MODE"] {
                    std::env::remove_var(k);
                }
                std::env::set_var("HOME", format!("{top}/home"));
                std::env::set_var("TMPDIR", format!("{top}/tmp"));
                for k in ["SARINA_PRIVATE_DIRS", "SARINA_VERIFY_TESTS", "SARINA_ACCESS_TIER", "SARINA_SANDBOX_TESTS"] {
                    std::env::set_var(k, "");
                }
            }
            "verifyRun" => {
                let tc = make_tc(true);
                for (r, want) in c["in"].as_array().unwrap().iter().zip(c["out"].as_array().unwrap()) {
                    let r = strs(r);
                    *mode.lock().unwrap() = r[1].clone();
                    std::env::set_var("PATH", if r[4].is_empty() { &bin } else { &binbw });
                    let (note, md) = verify_run(
                        &format!("{top}/{}", r[0]),
                        &tc,
                        !r[2].is_empty(),
                        !r[3].is_empty(),
                        !r[4].is_empty(),
                        "the reason given",
                    );
                    let md = gojson::marshal_any(&Any::Object(md)).unwrap();
                    check.eq(format!("verifyRun {r:?}"), &json!([hide(&note), hide(&md)]), want);
                }
            }
            "calls" => {
                let mut tools = vec![check_code_tool()];
                tools.extend(file_tools().into_iter().filter(|t| matches!(t.spec.name.as_str(), "write_file" | "edit_file" | "read_file")));
                let reg = Registry::new(tools);
                let mut tcs = HashMap::from([(true, make_tc(true)), (false, make_tc(false))]);
                for (i, (call, want)) in c["in"].as_array().unwrap().iter().zip(c["out"].as_array().unwrap()).enumerate() {
                    let call = strs(call);
                    *mode.lock().unwrap() = call[2].clone();
                    std::env::set_var("PATH", if call[4].is_empty() { &bin } else { &binbw });
                    let args = gojson::unmarshal_any(call[1].as_bytes()).0.into_object().unwrap();
                    let tc = tcs.get_mut(&!call[3].is_empty()).unwrap();
                    tc.step = i as i64 + 1;
                    let r = reg.execute(&call[0], Some(args), tc);
                    let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                    check.eq(format!("call {i} {call:?}"), &json!([r.ok, hide(&r.content), hide(&md)]), want);
                }
            }
            "files" => {
                for f in c["out"].as_array().unwrap() {
                    let data = std::fs::read(format!("{top}/{}", f[0].as_str().unwrap())).unwrap();
                    let got = sarina::gotext::quote(&String::from_utf8_lossy(&data));
                    check.eq(format!("file {}", f[0]), &json!(got), &f[1]);
                }
            }
            other => panic!("unknown fn {other}"),
        }
    }
    std::env::set_current_dir("/").unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("checks");
}
