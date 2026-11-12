//! testdata/parity/secrets.json: secretName, IsSecretPath, ExfilRisk, and on the same made-up home folder IsPrivate,
//! secretGuard (hard links included), privateGuard and PrivateReference under several SARINA_PRIVATE_DIRS.
//!
//! One test function: it changes HOME, the working folder and SARINA_PRIVATE_DIRS, which the whole process shares.

mod common;

use sarina::gopath;
use sarina::tools::exfil::{exfil_risk, private_reference};
use sarina::tools::private::{is_private, private_guard};
use sarina::tools::secret::{is_secret_path, secret_guard, secret_name};
use serde_json::{json, Value};

#[test]
fn secrets_match_go() {
    let cases = common::cases("secrets");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-secrets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let base = format!("{top}/home");
    let hide = |s: &str| s.replace(&base, "$BASE").replace(&top, "$TOP");
    let contents: Vec<String> =
        cases.iter().filter(|c| c["fn"] == "ExfilRisk").map(|c| String::from_utf8(common::bytes(&c["in"])).unwrap()).collect();
    for c in &cases {
        match c["fn"].as_str().unwrap() {
            "secretName" => check.eq(format!("secretName({})", c["in"]), &json!(secret_name(&common::bytes(&c["in"]))), &c["out"]),
            "IsSecretPath" => check.eq(format!("IsSecretPath({})", c["in"]), &json!(is_secret_path(&common::bytes(&c["in"]))), &c["out"]),
            "ExfilRisk" => {
                let s = String::from_utf8(common::bytes(&c["in"])).unwrap();
                check.eq(format!("ExfilRisk({s:?})"), &common::text(exfil_risk(&s).as_bytes()), &c["out"]);
            }
            "tree" => {
                for e in c["in"].as_array().unwrap() {
                    let rel = e[0].as_str().unwrap();
                    let p = if rel.is_empty() { base.clone() } else { format!("{base}/{rel}") };
                    match e[1].as_str().unwrap() {
                        "dir" => std::fs::create_dir(&p).unwrap(),
                        "file" => std::fs::write(&p, "x").unwrap(),
                        "link" => std::os::unix::fs::symlink(e[2].as_str().unwrap(), &p).unwrap(),
                        "hard" => std::fs::hard_link(format!("{base}/{}", e[2].as_str().unwrap()), &p).unwrap(),
                        k => panic!("{k}"),
                    }
                }
                std::env::set_var("HOME", &base);
                std::env::set_current_dir(&base).unwrap();
                std::env::set_var("PWD", &base);
            }
            "home" => {
                let priv_dirs = c["in"][0].as_str().unwrap();
                std::env::set_var("SARINA_PRIVATE_DIRS", priv_dirs.replace("$BASE", &base));
                let mut out = Vec::new();
                for p in c["in"][1].as_array().unwrap() {
                    let p = p.as_str().unwrap();
                    let g = match p.strip_prefix("rel:") {
                        Some(r) => r.to_owned(),
                        None if p.starts_with('/') => p.to_owned(),
                        None => String::from_utf8(gopath::join(&[base.as_bytes(), p.as_bytes()])).unwrap(),
                    };
                    let resolved = gopath::eval_symlinks(g.as_bytes()).unwrap_or_else(|_| g.as_bytes().to_vec());
                    let err = |r: Result<(), String>| hide(&r.err().unwrap_or_default());
                    out.push(json!([
                        is_private(g.as_bytes()),
                        err(secret_guard(&g, &resolved, "read")),
                        err(private_guard(&g, &resolved, "write"))
                    ]));
                }
                let refs: Vec<Value> = contents.iter().map(|s| json!(hide(&private_reference(&s.replace("/home/bob", &base))))).collect();
                check.eq(format!("home({priv_dirs:?})"), &json!({ "paths": out, "PrivateReference": refs }), &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    std::env::set_current_dir("/").unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("secrets");
}
