//! Go: `go/cmd/sarina/main.go`, as `sarina-rs` starts (docs/RUST_PORT.md, "The plan for `sarina-rs`"): the command the
//! name it was started as stands for, Heimdall's containment lock before anything else, and the refusal of a command
//! whose arguments are not all UTF-8 (Sarina's commands take text; Go's took the bytes, a difference on purpose since
//! 2026-11-18).

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

/// The arguments after `command` as text, or the refusal (one line for standard error, exit status 2, nothing run) when
/// one is not UTF-8: the command, the first such argument's position (1 is the first after the command) and the argument
/// quoted as the usage quotes a name (Go's `%q`: the bytes that are not UTF-8 as `\xHH`).
pub fn text_arguments(command: &str, rest: &[Vec<u8>]) -> Result<Vec<String>, String> {
    rest.iter()
        .enumerate()
        .map(|(i, a)| {
            String::from_utf8(a.clone())
                .map_err(|_| format!("sarina: {command}: argument {} is not UTF-8: {}; nothing was run\n", i + 1, gotext::quote_bytes(a)))
        })
        .collect()
}
