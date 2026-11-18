//! The `sarina-rs` binary as it starts (testdata/parity/entry.json from go/cmd/sarina): the command each name stands for,
//! the command lines a contained Sarina refuses (message and exit status byte for byte) and those it answers (`version`,
//! `help`: in Rust since 2026-11-18), the usage and `version` answered here, and a command with an argument that is not
//! UTF-8: refused (one line on standard error naming the command, the argument's position and the argument as `%q`
//! quotes it, exit status 2, nothing run; Go ran the command: a difference on purpose since 2026-11-18). Nothing is
//! handed to the Go build.

mod common;

use std::os::unix::ffi::OsStrExt;
use std::process::Command;

use sarina::cli::entry;
use serde_json::json;

#[test]
fn sarina_rs_starts_as_go_does() {
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-rs-entry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let lock = tmp.join("contained");
    std::fs::write(&lock, r#"{"reason":"a test lock: <b> & é"}"#).unwrap();
    let exe = env!("CARGO_BIN_EXE_sarina-rs");
    for name in ["sarina", "sarina-service", "sarina-gui"] {
        std::os::unix::fs::symlink(exe, tmp.join(name)).unwrap();
    }
    let run = |name: &str, args: &[&[u8]], contained: bool| {
        let mut cmd = Command::new(tmp.join(name));
        cmd.args(args.iter().map(|a| std::ffi::OsStr::from_bytes(a))).current_dir(&tmp);
        cmd.env("SARINA_CONTAINED_PATH", if contained { lock.clone() } else { tmp.join("none") });
        cmd.output().unwrap()
    };
    for c in common::cases("entry") {
        match c["fn"].as_str().unwrap() {
            "alias" => {
                let got = entry::alias_command(&common::bytes(&c["in"])).unwrap_or("");
                check.eq(format!("alias {}", c["in"]), &json!(got), &c["out"]);
            }
            "contained" => {
                let name = c["in"]["name"].as_str().unwrap();
                let args: Vec<Vec<u8>> =
                    c["in"]["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().as_bytes().to_vec()).collect();
                let refs: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
                let out = run(name, &refs, true);
                let code = out.status.code().unwrap_or(-1);
                let mut got = json!({"refused": code == 3, "code": code});
                if code == 3 {
                    got["stderr"] = json!(String::from_utf8_lossy(&out.stderr));
                } else {
                    // let through (version, help): answered here
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    check.eq(
                        format!("{name} {args:?} answered in Rust"),
                        &json!(
                            stdout.starts_with("sarina ") || stdout.starts_with("{\"version\":") || stdout.starts_with("Usage: sarina ")
                        ),
                        &json!(true),
                    );
                }
                check.eq(format!("contained {name} {args:?}"), &got, &c["out"]);
            }
            f => panic!("unknown case {f}"),
        }
    }

    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let ported = |name: &str, args: &[&[u8]], contained: bool| {
        let mut cmd = Command::new(tmp.join(name));
        cmd.args(args.iter().map(|a| std::ffi::OsStr::from_bytes(a)))
            .current_dir(&home)
            .env("HOME", &home)
            .env("SARINA_VAULT", home.join("vault"))
            .env("SARINA_CONTAINED_PATH", if contained { lock.clone() } else { tmp.join("none") });
        let out = cmd.output().unwrap();
        json!([out.status.code(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)])
    };
    check.eq("ported", &ported("sarina", &[b"memory-show"], false), &json!([0, "No curated memory is stored.\n", ""]));
    // a command with an argument that is not UTF-8: refused, the first such argument named by its position and quoted as
    // Go's %q quotes (\xHH), exit status 2, nothing run (the commands take text; Go ran them)
    let refused =
        |what: &str, pos: usize, q: &str| json!([2, "", format!("sarina: {what}: argument {pos} is not UTF-8: {q}; nothing was run\n")]);
    for (name, args, what, pos, q) in [
        ("sarina", &[&b"memory-show"[..], b"caf\xe9", b"--allow-write"][..], "memory-show", 1, "\"caf\\xe9\""),
        ("sarina", &[b"memory-show", b"--cwd", b"\xff"], "memory-show", 2, "\"\\xff\""),
        ("sarina", &[b"note", b"caf\xc3\xa9 ok", b"\xc3", b"\xfe"], "note", 2, "\"\\xc3\""),
        ("sarina", &[b"note", b"line\none \xe2\x80"], "note", 1, "\"line\\none \\xe2\\x80\""),
        ("sarina", &[b"vault-init", b"--path", b"v", b"\xed\xa0\x80"], "vault-init", 3, "\"\\xed\\xa0\\x80\""),
        ("sarina", &[b"version", b"\xff"], "version", 1, "\"\\xff\""),
        ("sarina-gui", &[b"\xff"], "gui", 1, "\"\\xff\""),
        ("sarina-service", &[b"--addr", b"\x80"], "service", 2, "\"\\x80\""),
    ] {
        check.eq(format!("refused {name} {args:?}"), &ported(name, args, false), &refused(what, pos, q));
    }
    // nothing ran: the home the commands write to (the vault, ~/.sarina) is still empty
    check.eq("nothing written", &json!(std::fs::read_dir(&home).unwrap().count()), &json!(0));
    // Heimdall's lock comes first: a contained Sarina gives its own message, whatever the arguments
    check.eq(
        "contained first",
        &ported("sarina", &[b"memory-show", b"\xff"], true),
        &json!([3, "", entry::contained_message("a test lock: <b> & é")]),
    );
    // version and the usage are answered here; the CLI differential compares them with Go's byte for byte
    let usage = sarina::cli::usage();
    let b = sarina::cli::version::read_build();
    check.eq("version", &ported("sarina", &[b"version"], false), &json!([0, b.text() + "\n", ""]));
    check.eq("version --json", &ported("sarina", &[b"version", b"--json"], false), &json!([0, b.json() + "\n", ""]));
    check.eq("no command", &ported("sarina", &[], false), &json!([2, usage, ""]));
    for h in ["help", "--help", "-h"] {
        check.eq(h, &ported("sarina", &[h.as_bytes(), b"\xff"], false), &json!([0, usage, ""]));
    }
    for (c, q) in [(&b"serve"[..], "\"serve\""), (b"Version", "\"Version\""), (b"--", "\"--\""), (b"caf\xe9", "\"caf\\xe9\"")] {
        check.eq(
            format!("unknown {q}"),
            &ported("sarina", &[c, b"\xff"], false),
            &json!([2, "", format!("sarina: unknown command {q}\n\n{usage}")]),
        );
    }
    check.eq("the table", &json!(sarina::cli::COMMANDS.len()), &json!(52));
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("sarina-rs");
}
