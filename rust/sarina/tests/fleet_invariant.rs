//! fleet-launch is CLI-only, structurally (go/chymaera/fleet/invariant_test.go's rule, for the Rust build): the model can
//! call only what the tool registry holds, so the fleet module may be named by nothing but the crate's module list and the
//! command table, and no tool definition may name a launcher. If one of these fails, do not weaken it: a prompt-injected
//! session must never be able to start another agent.

use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs" || x == "json") {
            out.push(p);
        }
    }
}

#[test]
fn nothing_outside_the_command_table_names_the_fleet() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    assert!(files.len() > 50, "only {} files under {}: the scan is not looking at the tree", files.len(), src.display());
    let needles = ["fleet::", "fleet-launch", "fleet_launch", "FleetLaunch", "fleetLaunch", "mod fleet"];
    for f in &files {
        let rel = f.strip_prefix(&src).unwrap().to_string_lossy().into_owned();
        if rel.starts_with("fleet/") {
            continue;
        }
        let text = std::fs::read_to_string(f).unwrap();
        for (n, line) in text.lines().enumerate() {
            let allowed = match rel.as_str() {
                "lib.rs" => line.trim() == "pub mod fleet;",
                // the command table: `("fleet-launch", "SUMMARY", crate::fleet::launch::command),` and comments
                "cli/mod.rs" => {
                    let t = line.trim();
                    (t.starts_with("(\"fleet-") && t.ends_with("::command),") && t.contains(", crate::fleet::")) || t.starts_with("//")
                }
                _ => false,
            };
            if !allowed && needles.iter().any(|x| line.contains(x)) {
                panic!("{rel}:{} names the fleet outside the command table: {}", n + 1, line.trim());
            }
        }
    }
}

#[test]
fn no_tool_definition_is_a_launcher() {
    let defs = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools/defs.json")).unwrap();
    let re = regex::Regex::new(r"(?i)fleet[-_ ]?(launch|usage|status)|agent[-_]?launch|launch another agent").unwrap();
    assert!(!re.is_match(&defs), "a tool definition names the launcher: {:?}", re.find(&defs).map(|m| m.as_str()));
}
