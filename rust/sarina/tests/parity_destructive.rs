//! testdata/parity/destructive.json: Go's CheckDestructive (for each workspace root) and CheckPrivileged on the shell corpus
//! and on writes around the workspace, compared byte for byte.

mod common;

use sarina::gopath;
use sarina::tools::destructive::{check_destructive, check_privileged, Verdict};
use serde_json::{json, Value};

/// The roots of go/tools/destructive_parity_test.go, in its order.
const ROOTS: [&str; 5] = ["", "/nonexistent-sarina-parity/ws", "/nonexistent-sarina-parity/ws/", "ws-relative", "/"];

/// One verdict against the fixture's: its reason, or one of the reasons Go gave when it named more than one (map order).
fn matches(v: &Verdict, want: &Value) -> Value {
    assert_eq!(v.blocked, !v.reason.is_empty());
    let got = common::text(&v.reason);
    match want.get("reasons") {
        Some(Value::Array(rs)) if rs.contains(&got) => want.clone(),
        _ => got,
    }
}

#[test]
fn destructive_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("destructive") {
        let fname = c["fn"].as_str().unwrap();
        match fname {
            "Check" => {
                let cmd = common::bytes(&c["in"]);
                let want = &c["out"];
                let d: Vec<Value> = ROOTS
                    .iter()
                    .zip(want["destructive"].as_array().unwrap())
                    .map(|(root, w)| matches(&check_destructive(&cmd, root.as_bytes()), w))
                    .collect();
                let got = json!({ "destructive": d, "privileged": matches(&check_privileged(&cmd), &want["privileged"]) });
                check.eq(format!("Check({:?})", String::from_utf8_lossy(&cmd)), &got, want);
            }
            "within" | "filepath.Rel" => {
                let root = common::bytes(&c["in"][0]);
                let p = common::bytes(&c["in"][1]);
                let got = if fname == "within" {
                    let r = gopath::rel(&root, &p);
                    json!(r.is_ok_and(|r| r != b".." && !r.starts_with(b"../")))
                } else {
                    match gopath::rel(&root, &p) {
                        Ok(r) => json!([common::text(&r), ""]),
                        Err(e) => json!(["", e]),
                    }
                };
                check.eq(format!("{fname}({c})"), &got, &c["out"]);
            }
            other => panic!("unknown fn {other}"),
        }
    }
    check.done("destructive");
}
