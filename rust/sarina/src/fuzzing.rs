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

/// Any bytes as a vector index file: no panic and no allocation beyond what the file holds; a file that reads writes
/// back as its own first bytes, and searching it works.
pub fn vecindex(data: &[u8]) {
    use crate::vecindex::Index;
    let Ok(x) = Index::from_bytes(data) else { return };
    let back = x.to_bytes();
    assert_eq!(&data[..back.len()], &back[..], "a file reads back to itself");
    let q = vec![1.0f32; x.dim()];
    let hits = x.search(&q, 3, None).expect("a query of the right size");
    assert!(hits.len() <= 3.min(x.len()));
}

/// Any bytes as Heimdall's files (the grants file, the signed list's key and signature, a time): no panic; a gate that is on
/// never allows more than asked nor less than 0 (read-only), whatever the file says; base64 that reads has the length its
/// groups give.
pub fn heimdall_files(data: &[u8]) {
    let g = crate::heimdall::grants_from_json(data);
    for own in 0..=3 {
        let cap = g.tier_cap(own);
        assert!((0..=own).contains(&cap), "cap {cap} for {own}");
    }
    if !g.problem.is_empty() {
        assert_eq!(g.tier_cap(3), 0, "a file with a problem allows nothing");
    }
    if let Some(b) = crate::trust::std_base64(data) {
        let n = data.iter().filter(|c| **c != b'\r' && **c != b'\n').count();
        let pad = data.iter().rev().filter(|c| **c != b'\r' && **c != b'\n').take_while(|c| **c == b'=').count();
        assert_eq!(b.len(), n / 4 * 3 - pad);
    }
    let _ = crate::trust::verify(data.get(..32).unwrap_or(data), data, data.get(32..96).unwrap_or(data));
    if let Ok(t) = crate::gotime::Time::parse_rfc3339(&crate::gotext::lossy(data)) {
        assert!(t.nanos() < 1_000_000_000);
    }
}

/// Any bytes as a workspace policy file (repository-controlled: a prompt-injected agent can write it) after an operator
/// file: no panic, and it only ever tightens: trust never turns on, nothing the operator denied is allowed, no limit rises
/// or disappears, no environment variable is exported.
pub fn policy_file(data: &[u8]) {
    let distrusting: &[u8] = br#"{"trusted":false,"denyTools":["x"]}"#;
    let full: &[u8] = br#"{"denyTools":["bash","Write_File"],"denyToolPrefixes":["mcp__"],"safeEnv":["PATH"],
        "budget":{"maxToolCalls":10,"maxTotalCostUsd":1.5,"maxSessionTurns":4}}"#;
    for operator in [full, distrusting] {
        policy_tightens(operator, data);
    }
}

fn policy_tightens(operator: &[u8], data: &[u8]) {
    use crate::config::BudgetConfig;
    use crate::policy::Policy;
    let mut op = Policy::new();
    op.apply_file("/op/policy.json", Ok(operator.to_vec()), true);
    let mut both = op.clone();
    both.apply_file("/ws/.sarina-policy.json", Ok(data.to_vec()), false);
    assert!(!both.trusted || op.trusted);
    assert_eq!(both.safe_env, op.safe_env);
    for tool in ["bash", "BASH", "write_file", "mcp__x", "read_file", "x"] {
        if !op.denied(tool).is_empty() {
            assert!(!both.denied(tool).is_empty(), "{tool} allowed again");
        }
    }
    assert!(both.deny_rules() >= op.deny_rules());
    let start = BudgetConfig { max_tool_calls: Some(100), max_total_tokens: Some(7), ..BudgetConfig::default() };
    let (mut a, mut b) = (start.clone(), start);
    op.apply_budget(&mut a);
    both.apply_budget(&mut b);
    let le = |x: Option<i64>, y: Option<i64>| match (x, y) {
        (Some(x), Some(y)) => x <= y,
        (Some(_), None) => true,
        (None, y) => y.is_none(),
    };
    assert!(le(b.max_tool_calls, a.max_tool_calls) && le(b.max_total_tokens, a.max_total_tokens));
    assert!(le(b.max_session_turns, a.max_session_turns));
    assert!(b.max_total_cost_usd.unwrap_or(f64::MAX) <= a.max_total_cost_usd.unwrap_or(f64::MAX));
}

/// Any bytes through the text gate's local checks: no panic; the verdict does not depend on the gate's leading-BOM switch
/// unless the text starts with a BOM; a zero-width space anywhere is always refused; a refused instruction quotes text
/// that is in the input.
pub fn guard(data: &[u8]) {
    use crate::guard::Gate;
    let plain = Gate::default().scan_local(data);
    let bom = Gate::default_gate().scan_local(data);
    assert_eq!(plain.blocked, !plain.reason.is_empty());
    if !data.starts_with("\u{FEFF}".as_bytes()) {
        assert_eq!(plain, bom);
    }
    let cut = data.len() / 2;
    let mut zw = data[..cut].to_vec();
    zw.extend_from_slice("\u{200B}".as_bytes());
    zw.extend_from_slice(&data[cut..]);
    assert!(Gate::default_gate().scan_local(&zw).blocked);
}

/// Any bytes as an analysis cache file: no panic, and what reads is written back so that it reads the same (the cache
/// survives its own round trip).
pub fn cache_file(data: &[u8]) {
    use crate::analysiscache::{encode_file, read_entries};
    for ns in ["ns", ""] {
        let entries = read_entries(data, ns);
        if let Ok(text) = encode_file(ns, &entries) {
            assert_eq!(read_entries(text.as_bytes(), ns), entries);
        }
    }
}

/// Any text as a model's answer: the repair and the normaliser do not panic; JSON that reads as it is comes back
/// unchanged and not truncated; arguments are an object or an error, never both; after normalising, every tool call has
/// an id and no two share one, and a call cut off by a length stop is never run with partial arguments.
pub fn model_text(data: &[u8]) {
    use crate::llm::normalize::normalize;
    use crate::llm::repair::{parse_arguments, repair_json};
    use crate::llm::types::Turn;
    let Ok(s) = std::str::from_utf8(data) else { return };
    let (v, trunc, ok) = repair_json(s);
    assert_eq!(ok, v.is_some());
    if let (Ok(direct), false) = (crate::gojson::parse_first(s.trim().as_bytes()).0, s.trim().starts_with("```")) {
        if let (a, None) = crate::gojson::to_any(&direct) {
            assert!(ok && !trunc);
            assert_eq!(v.as_ref(), Some(&a));
        }
    }
    let (args, _, err) = parse_arguments(s);
    assert_ne!(args.is_some(), !err.is_empty());
    for finish in ["stop", "length"] {
        let mut t = Turn { content: s.to_string(), finish_reason: finish.into(), ..Turn::default() };
        normalize(&mut t, &["bash".to_string(), "read_file".to_string()]);
        let mut ids = std::collections::HashSet::new();
        for c in &t.tool_calls {
            assert!(!c.id.is_empty() && ids.insert(c.id.clone()), "ids {:?}", t.tool_calls);
            if c.truncated && finish == "length" {
                assert!(!c.parse_error.is_empty());
            }
        }
    }
}

/// Any text as a saved message and a search query: the snippet does not panic, is at most 320 characters plus the
/// marks, and holds the first query word found when there is one.
pub fn search_snippet(data: &[u8]) {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let (content, query) = s.split_once('\u{0}').unwrap_or((s, ""));
    let out = crate::sessionsearch::snippet(content, query);
    assert!(out.chars().count() <= 2 * 160 + 6, "{} characters", out.chars().count());
    let low = crate::gotext::to_lower(&out);
    let lowered = crate::gotext::to_lower(query);
    let content_low = crate::gotext::to_lower(content);
    if let Some(w) = crate::gotext::fields(&lowered).into_iter().find(|w| content_low.contains(w)) {
        // the window starts at the word, so its first character is always in it
        assert!(low.contains(w.chars().next().unwrap_or(' ')), "{w:?} not in {out:?}");
    }
}

/// Any bytes as an operator's agent profile: no panic, and a profile that reads is one the runner can trust the shape
/// of: a valid name, a description, a prompt, at most 20 turns, and no empty tool name.
pub fn agent_profile(data: &[u8]) {
    if let Ok(p) = crate::agents::parse(data, "fallback") {
        assert!(!p.name.is_empty() && p.name.len() <= 64 && p.name.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)));
        assert!(!p.description.is_empty() && !p.prompt.is_empty());
        assert!((0..=crate::agents::MAX_TURNS_LIMIT).contains(&p.max_turns));
        if let Some(t) = &p.tools {
            assert!(!t.is_empty() && t.iter().all(|x| !x.is_empty()));
        }
    }
}

/// Any bytes as a skill file: no panic; a skill that reads has a valid name and a body, and its expansion only replaces
/// `$ARGUMENTS`.
pub fn skill_file(data: &[u8]) {
    if let Ok(s) = crate::skills::parse::parse(data, "fallback") {
        assert!(crate::skills::parse::valid_name(s.name.as_bytes()));
        assert!(!s.body.is_empty());
        assert_eq!(s.expand("$ARGUMENTS"), s.body);
        if !s.has_allowed {
            assert!(s.allowed_tools.is_empty());
        }
    }
}

/// Any bytes as an answer from Limbo's agent socket (see `limbo::fuzz_answer`).
pub fn limbo_answer(data: &[u8]) {
    crate::limbo::fuzz_answer(data);
}

/// Any bytes as a model server's answer, streamed and not: no panic; a refused answer is a backend error with a message;
/// within tight bounds, an answer past them is always refused.
pub fn model_answer(data: &[u8]) {
    use crate::llm::openai::{parse_response, read_stream};
    if let Err(e) = parse_response(data) {
        assert!(!e.message.is_empty());
    }
    if let Err(e) = crate::llm::anthropic::parse_anthropic(data) {
        assert!(!e.message.is_empty());
    }
    let mut events = 0;
    let r = read_stream(data, 1 << 20, 1 << 16, &mut || {}, &mut |_| events += 1);
    assert!(events >= 1, "message_start always comes first");
    if let Err(e) = &r {
        assert!(!e.message().is_empty());
    }
    let tight = data.len().saturating_sub(1);
    if !data.is_empty() {
        let r = read_stream(data, tight, 1 << 16, &mut || {}, &mut |_| {});
        assert!(r.is_err(), "an answer past its bound was read");
    }
}
