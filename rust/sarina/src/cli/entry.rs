//! Go: `go/cmd/sarina/main.go`, as `sarina-rs` starts (docs/RUST_PORT.md, "The plan for `sarina-rs`"): the command the
//! name it was started as stands for, Heimdall's containment lock before anything else, and where the Go build is that
//! every command not ported yet is handed to.

use crate::gopath;
use crate::gotext;

/// Go's `aliasCommand`: `sarina-service` is `service`, `sarina-gui` is `gui` (the lower-cased base name, `.exe` cut).
pub fn alias_command(argv0: &[u8]) -> Option<&'static str> {
    let name = gotext::to_lower_bytes(&gopath::base(argv0));
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "sarina-service" => Some("service"),
        "sarina-gui" => Some("gui"),
        _ => None,
    }
}

/// Whether a contained Sarina still answers this command line (only `version`, `help` and `--help` do).
pub fn allowed_when_contained(args: &[Vec<u8>]) -> bool {
    matches!(args.first().map(Vec::as_slice), Some(b"version" | b"help" | b"--help"))
}

/// Go's message (and exit status 3) for a contained Sarina.
pub fn contained_message(why: &str) -> String {
    format!("sarina: Heimdall has contained me ({why}). The owner releases me in Settings > Security, or: sudo heimdalld contain release\n")
}

/// Where the Go build is installed.
pub const GO_BUILD: &str = "/usr/lib/sarina/sarina-go";

/// The Go build every command not ported yet goes to: `$SARINA_GO_BINARY` when it is an absolute path, else
/// [`GO_BUILD`]. It is never looked up on PATH.
pub fn go_binary(getenv: &dyn Fn(&str) -> Vec<u8>) -> Vec<u8> {
    let v = getenv("SARINA_GO_BINARY");
    if gopath::is_abs(&v) {
        v
    } else {
        GO_BUILD.as_bytes().to_vec()
    }
}

/// The message (exit status 127) when the Go build a command needs is not there.
pub fn missing_go_message(command: &[u8], path: &[u8]) -> String {
    let c = if command.is_empty() { "sarina".to_owned() } else { gotext::lossy(command) };
    format!("sarina: {c} is not in the Rust build yet, and the Go build ({}) is missing\n", gotext::lossy(path))
}
