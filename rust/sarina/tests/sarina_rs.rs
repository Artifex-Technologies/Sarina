//! The `sarina-rs` binary as it starts (testdata/parity/entry.json from go/cmd/sarina): the command each name stands for,
//! the command lines a contained Sarina refuses (message and exit status byte for byte) and those it answers (`version`,
//! `help`: in Rust since 2026-11-18, never handed over), the usage and `version` answered here, and the one hand-over
//! left until the Go build is removed: a command with an argument that is not UTF-8, given to the Go build by execve (a
//! stand-in in bash's builtins here): the arguments unchanged, the environment, the folder, the exit status and the
//! caller's SIGPIPE disposition. (argv[0] is passed as it came; a script cannot show it: the kernel puts the script's path
//! there.)

mod common;

use std::os::unix::ffi::OsStrExt;
use std::process::Command;

use sarina::cli::entry;
use serde_json::json;

const STAND_IN: &str = r#"#!/bin/bash
for a in "$@"; do printf 'arg=%s\n' "$a"; done
printf 'pwd=%s\nmark=%s\n' "$PWD" "$PARITY_MARK"
while IFS=$'\t' read -r k v; do [[ $k == SigIgn: ]] && printf 'sigpipe_ignored=%s\n' $(( 0x$v >> 12 & 1 )); done < /proc/$$/status
exit "${EXIT_WITH:-0}"
"#;

#[test]
fn sarina_rs_starts_as_go_does() {
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-rs-entry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let go = tmp.join("sarina-go");
    std::fs::write(&go, STAND_IN).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&go, std::fs::Permissions::from_mode(0o755)).unwrap();
    let lock = tmp.join("contained");
    std::fs::write(&lock, r#"{"reason":"a test lock: <b> & é"}"#).unwrap();
    let exe = env!("CARGO_BIN_EXE_sarina-rs");
    for name in ["sarina", "sarina-service", "sarina-gui"] {
        std::os::unix::fs::symlink(exe, tmp.join(name)).unwrap();
    }
    let run = |name: &str, args: &[&[u8]], contained: bool| {
        let mut cmd = Command::new(tmp.join(name));
        cmd.args(args.iter().map(|a| std::ffi::OsStr::from_bytes(a))).current_dir(&tmp);
        cmd.env("SARINA_GO_BINARY", &go)
            .env("PARITY_MARK", "kept")
            .env("SARINA_CONTAINED_PATH", if contained { lock.clone() } else { tmp.join("none") });
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
                    // let through (version, help): answered here, the Go build never started
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

    // the hand-over (only a command with an argument that is not UTF-8 is still given to the Go build): the arguments as
    // they came, the folder, the environment, the exit status
    let out = run("sarina", &[b"memory-show", b"caf\xe9", b"--allow-write"], false);
    let want = format!("arg=memory-show\narg=caf\u{FFFD}\narg=--allow-write\npwd={}\nmark=kept\nsigpipe_ignored=0\n", tmp.display());
    check.eq("hand-over", &json!(String::from_utf8_lossy(&out.stdout)), &json!(want));
    let mut cmd = Command::new(tmp.join("sarina"));
    cmd.arg("memory-show").arg(std::ffi::OsStr::from_bytes(b"\xff"));
    cmd.env("SARINA_GO_BINARY", &go).env("SARINA_CONTAINED_PATH", tmp.join("none")).env("EXIT_WITH", "7");
    check.eq("exit status", &json!(cmd.output().unwrap().status.code()), &json!(7));
    // a caller that ignores SIGPIPE (systemd's default for services) hands that on, as it would to the Go build itself
    let out = Command::new("/bin/bash")
        .arg("-c")
        .arg(format!("trap '' PIPE; exec {} memory-show $'\\xff'", tmp.join("sarina").display()))
        .env("SARINA_GO_BINARY", &go)
        .env("SARINA_CONTAINED_PATH", tmp.join("none"))
        .output()
        .unwrap();
    check.eq("sigpipe ignored", &json!(String::from_utf8_lossy(&out.stdout).contains("sigpipe_ignored=1\n")), &json!(true));
    // no Go build: 127 and what is missing; a relative SARINA_GO_BINARY is not taken
    let mut cmd = Command::new(tmp.join("sarina"));
    cmd.arg("memory-show").arg(std::ffi::OsStr::from_bytes(b"\xff"));
    cmd.env("SARINA_GO_BINARY", tmp.join("absent")).env("SARINA_CONTAINED_PATH", tmp.join("none"));
    let out = cmd.output().unwrap();
    check.eq(
        "missing go build",
        &json!([out.status.code(), String::from_utf8_lossy(&out.stderr)]),
        &json!([
            127,
            format!(
                "sarina: memory-show with an argument that is not UTF-8 is not in the Rust build yet, and the Go build ({}) is missing\n",
                tmp.join("absent").display()
            )
        ]),
    );
    // a ported command runs here (the stand-in is never started); one with an argument that is not UTF-8 goes to Go
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let ported = |args: &[&[u8]], no_go: bool| {
        let mut cmd = Command::new(tmp.join("sarina"));
        cmd.args(args.iter().map(|a| std::ffi::OsStr::from_bytes(a)))
            .current_dir(&tmp)
            .env("HOME", &home)
            .env("SARINA_GO_BINARY", &go)
            .env("SARINA_CONTAINED_PATH", tmp.join("none"));
        if no_go {
            cmd.env("SARINA_RS_NO_GO", "1");
        }
        let out = cmd.output().unwrap();
        json!([out.status.code(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)])
    };
    check.eq("ported", &ported(&[b"memory-show"], false), &json!([0, "No curated memory is stored.\n", ""]));
    check.eq("ported, no Go", &ported(&[b"memory-show"], true), &json!([0, "No curated memory is stored.\n", ""]));
    let away = ported(&[b"memory-show", b"\xff"], false);
    check.eq("not UTF-8 handed over", &json!(away[1].as_str().unwrap().starts_with("arg=memory-show\n")), &json!(true));
    check.eq(
        "not UTF-8 with SARINA_RS_NO_GO",
        &ported(&[b"memory-show", b"\xff"], true),
        &json!([2, "", "sarina-rs: memory-show with an argument that is not UTF-8 is not ported (SARINA_RS_NO_GO is set)\n"]),
    );
    // version and the usage are answered here, with or without a Go build (the stand-in is never started); the CLI
    // differential compares them with Go's byte for byte
    let usage = sarina::cli::usage();
    for no_go in [false, true] {
        let b = sarina::cli::version::read_build();
        check.eq(format!("version {no_go}"), &ported(&[b"version"], no_go), &json!([0, b.text() + "\n", ""]));
        check.eq(format!("version --json {no_go}"), &ported(&[b"version", b"--json"], no_go), &json!([0, b.json() + "\n", ""]));
        check.eq(format!("no command {no_go}"), &ported(&[], no_go), &json!([2, usage, ""]));
        for h in ["help", "--help", "-h"] {
            check.eq(format!("{h} {no_go}"), &ported(&[h.as_bytes(), b"\xff"], no_go), &json!([0, usage, ""]));
        }
        for (c, q) in [(&b"serve"[..], "\"serve\""), (b"Version", "\"Version\""), (b"--", "\"--\""), (b"caf\xe9", "\"caf\\xe9\"")] {
            check.eq(
                format!("unknown {q} {no_go}"),
                &ported(&[c, b"\xff"], no_go),
                &json!([2, "", format!("sarina: unknown command {q}\n\n{usage}")]),
            );
        }
    }
    check.eq("the table", &json!(sarina::cli::COMMANDS.len()), &json!(52));
    check.eq("relative go build", &json!(entry::go_binary(&|_: &str| b"sarina-go".to_vec())), &json!(entry::GO_BUILD.as_bytes()));
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("sarina-rs");
}
