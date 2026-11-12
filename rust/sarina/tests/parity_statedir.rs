//! testdata/parity/statedir.json: `StateDir` on a made-up tree: workspaces whose .port_sessions is a plain folder,
//! missing, a link out, a link inside, a dangling link, or whose agent, scratchpad or a deeper folder is a link out; the
//! default folders and others.

mod common;

use sarina::tools::fsutil::state_dir;
use serde_json::json;
use std::os::unix::fs::symlink;

#[test]
fn state_dir_matches_go() {
    let cases = common::cases("statedir");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-statedir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(sarina::gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let outside = format!("{top}/outside");
    std::fs::create_dir_all(&outside).unwrap();
    for v in ["plain", "missing", "link-out", "link-in", "dangling", "agent-out", "scratch-out", "nested-out"] {
        let ws = format!("{top}/{v}");
        let ps = format!("{ws}/.port_sessions");
        std::fs::create_dir_all(&ws).unwrap();
        match v {
            "plain" => std::fs::create_dir_all(format!("{ps}/agent")).unwrap(),
            "link-out" => symlink(&outside, &ps).unwrap(),
            "link-in" => {
                std::fs::create_dir_all(format!("{ws}/real")).unwrap();
                symlink("real", &ps).unwrap();
            }
            "dangling" => symlink(format!("{top}/nowhere"), &ps).unwrap(),
            "agent-out" => {
                std::fs::create_dir_all(&ps).unwrap();
                symlink(&outside, format!("{ps}/agent")).unwrap();
            }
            "scratch-out" => {
                std::fs::create_dir_all(&ps).unwrap();
                symlink("../../outside", format!("{ps}/scratchpad")).unwrap();
            }
            "nested-out" => {
                std::fs::create_dir_all(format!("{ps}/agent")).unwrap();
                symlink(&outside, format!("{ps}/agent/deeper")).unwrap();
            }
            _ => {}
        }
    }
    for c in &cases {
        let input = &c["in"];
        let unhide = |s: &str| s.replace("$BASE", &top);
        let (root, dir) = (unhide(input[1].as_str().unwrap()), unhide(input[2].as_str().unwrap()));
        let (got, err) = match state_dir(&root, &dir) {
            Ok(d) => (d, String::new()),
            Err(e) => (String::new(), e),
        };
        let hide = |s: &str| s.replace(&top, "$BASE");
        check.eq(format!("StateDir {input}"), &json!([hide(&got), hide(&err)]), &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("statedir");
}
