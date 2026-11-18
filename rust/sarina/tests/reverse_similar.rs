//! Go's TestSimilarRunsOnlyWithALibraryAndShowsOnlyTheReport (go/reverse/reverse_test.go), the Rust half: `similar` hands
//! labyrinth-rea the same sandbox script and the same Ghidra script as Go (both read from Go's source here, so a change
//! to one side alone fails), with the file named as the sandbox sees it. Its own test binary: it sets LABYRINTH_REA_SAMPLES.

use sarina::reverse::{args, BSIM_SCRIPT, SIMILAR_SCRIPT};

#[test]
fn similar_hands_the_sandbox_gos_scripts() {
    let go = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../go/reverse/reverse.go")).unwrap();
    let start = go.find("const similarScript = `").expect("Go's similarScript") + "const similarScript = `".len();
    let len = go[start..].find('`').unwrap();
    assert_eq!(SIMILAR_SCRIPT, &go[start..start + len]);
    let java = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../go/reverse/LabyrinthBSim.java")).unwrap();
    assert_eq!(BSIM_SCRIPT, java);
    let d = std::env::temp_dir().join(format!("sarina-reverse-similar-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("x.bin"), "x").unwrap();
    std::env::set_var("LABYRINTH_REA_SAMPLES", &d);
    let a = args("similar", d.join("x.bin").to_str().unwrap(), "ignored", 0).unwrap();
    let a: Vec<String> = a.into_iter().map(|b| String::from_utf8(b).unwrap()).collect();
    assert_eq!(a, ["--exec", "sh", "-c", SIMILAR_SCRIPT, "bsim", "/samples/x.bin", BSIM_SCRIPT]);
    assert!(args("similar", "/etc/passwd", "", 0).is_err(), "a file outside the folders is refused as for every command");
    let _ = std::fs::remove_dir_all(&d);
}
