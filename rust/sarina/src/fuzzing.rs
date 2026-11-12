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

/// Any text as content the model writes and any bytes as a file name: no panic; content that would take a credential off the
/// computer always names a credential store for a forged agent too; a name that is a secret stays one in any folder and with
/// any case.
pub fn secrets(data: &[u8]) {
    use crate::tools::{exfil, secret};
    if let Ok(s) = std::str::from_utf8(data) {
        if !exfil::exfil_risk(s).is_empty() {
            assert!(!exfil::private_reference(s).is_empty(), "{s:?}");
        }
    }
    let name = data.rsplit(|&c| c == b'/' || c == b'\\').next().unwrap_or(data);
    let secret = secret::secret_name(name);
    assert_eq!(secret::is_secret_path(data), secret);
    let mut deeper = b"some/folder\\".to_vec();
    deeper.extend_from_slice(data);
    assert_eq!(secret::is_secret_path(&deeper), secret);
    if name.is_ascii() {
        assert_eq!(secret::secret_name(&name.to_ascii_uppercase()), secret);
    }
}

/// Any bytes as a JSON document (a session file, a model's or an MCP server's answer): no panic; Decode agrees with
/// Unmarshal on a whole document; what Go's encoder writes back reads to the same value, compact and indented.
pub fn json(data: &[u8]) {
    use crate::gojson;
    let whole = gojson::parse(data);
    let (first, more) = gojson::parse_first(data);
    let Ok(v) = whole else { return };
    assert_eq!(first.as_ref().ok(), Some(&v), "Decode reads what Unmarshal reads");
    assert!(!more, "nothing follows a whole document");
    let (a, _) = gojson::to_any(&v);
    for html in [true, false] {
        let mut out = String::new();
        gojson::write_any(&mut out, &a, html).expect("a decoded value always writes");
        let (back, err) = gojson::unmarshal_any(out.as_bytes());
        assert_eq!(err, None, "{out}");
        assert_eq!(back, a, "{out}");
        let (back, err) = gojson::unmarshal_any(gojson::indent(&out, "  ").as_bytes());
        assert_eq!(err, None);
        assert_eq!(back, a);
    }
}

/// Any bytes as a session file (Heimdall reads them, `sarina resume` loads them): no panic; a file that reads is written
/// back without the API key, and what is written reads back to the same file (one pass makes it Go's own form).
pub fn session(data: &[u8]) {
    use crate::session;
    let Ok(s) = session::parse_raw(data) else { return };
    let Ok(first) = s.marshal() else { return };
    assert!(!first.contains("\"api_key\""));
    assert!(first.is_ascii(), "the file is ASCII-only");
    let again = session::parse_raw(first.as_bytes()).expect("a written file reads");
    assert_eq!(again.marshal().as_deref(), Ok(first.as_str()));
    let mut migrated = again.clone();
    migrated.migrate_retired_tools();
    let mut twice = migrated.clone();
    assert_eq!(twice.migrate_retired_tools(), session::migrate::Migration::default(), "the migration is idempotent");
}
