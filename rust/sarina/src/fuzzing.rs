//! The fuzz targets' bodies (`rust/fuzz`, nightly), also run on stable by tests/properties.rs. Each one feeds outside input
//! to a parser or a guard and checks what must hold whatever the input.

use crate::shellparse;
use crate::tools::destructive::{check_destructive, check_privileged};

fn base(word: &[u8]) -> &[u8] {
    match word.iter().rposition(|&c| c == b'/' || c == b'\\') {
        Some(i) => &word[i + 1..],
        None => word,
    }
}

/// The program a guard sees for a command's first word: its base name, lowered, without ".exe".
fn program(word: &[u8]) -> String {
    let base = base(word);
    let lower = crate::gotext::to_lower_bytes(base);
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// Any bytes as a shell command line: the parser does not panic, and parsing is deterministic (the pipeline ids and the
/// here-documents depend on nothing but the input).
pub fn shellparse(data: &[u8]) {
    assert_eq!(shellparse::parse(data), shellparse::parse(data));
}

/// Any bytes through both bash guards: no panic, blocked exactly when there is a reason, a command line that does not parse
/// is refused by both, and a command whose program is privileged (or, for the destructive check, removes or rewrites
/// things) is refused wherever it sits in the line.
pub fn bash_guards(data: &[u8]) {
    let d = check_destructive(data, b"/nonexistent-sarina-fuzz/ws");
    let p = check_privileged(data);
    assert_eq!(d.blocked, !d.reason.is_empty());
    assert_eq!(p.blocked, !p.reason.is_empty());
    let Ok(cmds) = shellparse::parse(data) else {
        assert!(d.blocked && p.blocked, "a line that does not parse must be refused");
        return;
    };
    for c in cmds.iter().filter(|c| !c.data && !c.words.is_empty()) {
        let prog = program(&c.words[0].text);
        if matches!(prog.as_str(), "sudo" | "doas" | "pkexec" | "run0" | "su" | "runuser" | "claude-sudo" | "sarina-sudo") {
            assert!(p.blocked, "{prog} not refused by the privileged check");
        }
        if matches!(prog.as_str(), "rm" | "mv" | "dd" | "shred" | "truncate" | "reboot" | "poweroff") || prog.starts_with("mkfs") {
            assert!(d.blocked, "{prog} not refused by the destructive check");
        }
        if base(&c.words[0].text).contains(&0) {
            assert!(d.blocked && p.blocked, "a program built at run time must be refused");
        }
    }
}

/// Any text as a number a model sent: Go's ParseFloat does not panic, a value it accepts is what Rust's own parser makes of
/// the same digits (when Rust takes them), and every finite result prints and parses back to itself.
pub fn parse_float(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else { return };
    match crate::gotext::parse_float(s) {
        Ok(f) => {
            if !s.contains(['_', 'x', 'X', 'p', 'P', 'n', 'N', 'i', 'I']) {
                if let Ok(r) = s.parse::<f64>() {
                    assert_eq!(f.to_bits(), r.to_bits(), "{s:?}");
                }
            }
            if f.is_finite() {
                assert_eq!(crate::gotext::parse_float(&crate::gotext::float_v(f)).map(f64::to_bits), Ok(f.to_bits()));
            }
        }
        Err(e) => assert!(e.value == 0.0 || e.value.is_infinite()),
    }
}
