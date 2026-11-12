//! Properties that hold for any input, and the fuzz targets' bodies run on stable: random bytes, random lines built from
//! the shell's own characters, and every fixture input with each byte changed.

mod common;

use std::collections::BTreeMap;

use proptest::prelude::*;
use sarina::fuzzing;
use sarina::goany::Any;
use sarina::tools::schema::validate_args;

/// JSON-like values, as tool arguments arrive.
fn any_value() -> impl Strategy<Value = Any> {
    let leaf = prop_oneof![
        Just(Any::Null),
        any::<bool>().prop_map(Any::Bool),
        prop::num::f64::NORMAL.prop_map(Any::Number),
        prop::sample::select(vec![0.0, 1.0, -1.0, 1.5, 10.0, 1e21]).prop_map(Any::Number),
        "[ a-z0-9.eExp_+\\-]{0,8}".prop_map(Any::String),
        prop::sample::select(vec!["true", " FALSE ", "1e3", "0x1p3", "1_0", "inf", "nan"]).prop_map(|s| Any::String(s.into())),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Any::Array),
            prop::collection::btree_map("[a-e]", inner, 0..4).prop_map(Any::Object),
        ]
    })
}

/// Schemas as the tools declare them (and broken ones).
fn any_schema() -> impl Strategy<Value = Any> {
    let typ = prop::sample::select(vec!["object", "array", "string", "integer", "number", "boolean", "", "x"]);
    let leaf = (typ, prop::option::of(prop::sample::select(vec![0.0, 0.5, 1.0, 3.0, -2.5, 1e21]))).prop_map(|(t, n)| {
        let mut m = BTreeMap::new();
        m.insert("type".to_owned(), Any::String(t.to_owned()));
        if let Some(n) = n {
            for k in ["minimum", "maximum", "minLength"] {
                m.insert(k.to_owned(), Any::Number(n));
            }
        }
        Any::Object(m)
    });
    leaf.prop_recursive(3, 16, 4, |inner| {
        (prop::collection::btree_map("[a-e]", inner.clone(), 0..4), prop::collection::vec("[a-f]", 0..3), inner).prop_map(
            |(props, req, items)| {
                let mut m = BTreeMap::new();
                m.insert("type".to_owned(), Any::String("object".into()));
                m.insert("properties".to_owned(), Any::Object(props));
                m.insert("required".to_owned(), Any::Array(req.into_iter().map(Any::String).collect()));
                m.insert("items".to_owned(), items);
                Any::Object(m)
            },
        )
    })
}

/// Lines from the characters that matter to the shell parser and the guards.
fn shellish() -> impl Strategy<Value = Vec<u8>> {
    let atoms = prop::sample::select(vec![
        &b"rm"[..],
        b"sudo",
        b"bash",
        b"-c",
        b"env",
        b"x",
        b" ",
        b" ",
        b"'",
        b"\"",
        b"`",
        b"$",
        b"$(",
        b")",
        b"(",
        b"${",
        b"}",
        b"\\",
        b"\n",
        b";",
        b"|",
        b"&",
        b"<<",
        b"<<-",
        b"<",
        b">",
        b">&",
        b"2>",
        b"EOF",
        b"$'",
        b"\\x72",
        b"\xff",
        b"\0",
        b"#",
        b"=",
        b"f()",
        b"find",
        b"-exec",
        b"python3",
        b"curl",
        b"git",
        b"push",
        b"-f",
        b"tee",
        b"/etc/x",
        b"~",
        b"eval",
        b"for",
        b"in",
        b"case",
    ]);
    prop::collection::vec(atoms, 0..24).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn shellparse_any_bytes(data in prop::collection::vec(any::<u8>(), 0..200)) {
        fuzzing::shellparse(&data);
    }

    #[test]
    fn shellparse_shellish(data in shellish()) {
        fuzzing::shellparse(&data);
    }

    #[test]
    fn guards_any_bytes(data in prop::collection::vec(any::<u8>(), 0..200)) {
        fuzzing::bash_guards(&data);
    }

    #[test]
    fn guards_shellish(data in shellish()) {
        fuzzing::bash_guards(&data);
    }

    #[test]
    fn parse_float_any_text(s in "[ 0-9a-fA-FxXpPeE._+\\-infatyINFATY]{0,24}") {
        fuzzing::parse_float(s.as_bytes());
    }

    #[test]
    fn parse_float_any_bytes(data in prop::collection::vec(any::<u8>(), 0..40)) {
        fuzzing::parse_float(&data);
    }

    #[test]
    fn parse_float_round_trips(f in any::<f64>().prop_filter("finite", |f| f.is_finite())) {
        fuzzing::parse_float(sarina::gotext::float_v(f).as_bytes());
    }

    /// ValidateArgs never panics, never adds a key, and is idempotent: its own output passes unchanged.
    #[test]
    fn validate_args_is_idempotent(schema in any_schema(), args in prop::collection::btree_map("[a-f]", any_value(), 0..5)) {
        let Ok(schema) = schema.into_object() else { unreachable!() };
        if let Ok(out) = validate_args(Some(&schema), args.clone()) {
            prop_assert!(out.keys().all(|k| args.contains_key(k)));
            prop_assert_eq!(validate_args(Some(&schema), out.clone()), Ok(out));
        }
    }

    #[test]
    fn secrets_any_bytes(data in prop::collection::vec(any::<u8>(), 0..80)) {
        fuzzing::secrets(&data);
    }

    #[test]
    fn secrets_credential_shaped(s in "[ ~$/{}A-Za-z._=\\-]{0,12}(\\.ssh|\\.gnupg|id_rsa|key4\\.db|\\.ENV|x\\.pem)[ /'\"a-z]{0,6}(curl|nc|wget|fetch\\()?") {
        fuzzing::secrets(s.as_bytes());
    }

    #[test]
    fn json_any_bytes(data in prop::collection::vec(any::<u8>(), 0..120)) {
        fuzzing::json(&data);
    }

    /// JSON-shaped bytes: the fuzz body, and every document serde_json reads (strict) reads here to the same value.
    #[test]
    fn json_shaped(s in "[ \\t\\n{}\\[\\],:\"a-c0-9.eE+\\-\\\\/u]{0,60}|(null|true|false|[0-9.eE+\\-]{1,12}|\"[a-z\\\\\"/btnrfu0-9]{0,10}\"|[,:\\[\\]{} ]){0,20}") {
        fuzzing::json(s.as_bytes());
        if let Ok(strict) = serde_json::from_str::<serde_json::Value>(&s) {
            let (v, err) = sarina::gojson::unmarshal_any(s.as_bytes());
            if err.is_none() {
                prop_assert_eq!(common_any(&strict), v);
            }
        }
    }

    #[test]
    fn session_any_bytes(data in prop::collection::vec(any::<u8>(), 0..200)) {
        fuzzing::session(&data);
    }

    /// A saved index with bytes changed or cut, through the fuzz body (the header of a real file keeps most inputs
    /// reading past the magic).
    #[test]
    fn vecindex_damaged(at in 0usize..200, byte in any::<u8>(), cut in 0usize..220) {
        let mut x = sarina::vecindex::Index::new(5, 2, 9).unwrap();
        for i in 0..6u64 {
            x.add(i, &[i as f32, 1.0, -2.0, 0.5, 3.0]).unwrap();
        }
        let mut data = x.to_bytes();
        let at = at % data.len();
        data[at] = byte;
        fuzzing::vecindex(&data);
        fuzzing::vecindex(&data[..cut.min(data.len())]);
    }

    #[test]
    fn vecindex_any_bytes(data in prop::collection::vec(any::<u8>(), 0..80)) {
        let mut d = b"SVEC1\n".to_vec();
        d.extend(data);
        fuzzing::vecindex(&d);
    }

    #[test]
    fn heimdall_files_any_bytes(data in prop::collection::vec(any::<u8>(), 0..120)) {
        fuzzing::heimdall_files(&data);
    }

    /// Grants-shaped documents (a cap of any kind, escalations with times of any shape) and base64-shaped and time-shaped
    /// text, through the fuzz body.
    #[test]
    fn heimdall_files_shaped(
        cap in "(-?[0-9]{1,20}(\\.[0-9])?|null|true|\"3\"|\\[1\\])",
        key in "(access_tier|ACCESS_TIER|Access_Tier|access-tier|acceſs_tier)",
        until in "[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}[Tt][0-9]{1,2}:[0-9]{2}:[0-9]{2}([.,][0-9]{0,12})?(Z|[+-][0-9]{2}:[0-9]{2}|z|)",
        b64 in "[A-Za-z0-9+/=\r\n ]{0,24}",
    ) {
        let doc = format!(r#"{{"{key}":{cap},"escalations":[{{"tier":"claude","until":"{until}"}}]}}"#);
        fuzzing::heimdall_files(doc.as_bytes());
        fuzzing::heimdall_files(until.as_bytes());
        fuzzing::heimdall_files(b64.as_bytes());
    }

    #[test]
    fn policy_file_any_bytes(data in prop::collection::vec(any::<u8>(), 0..120)) {
        fuzzing::policy_file(&data);
    }

    /// Policy-shaped workspace files: every key the policy knows, in any case and with underscores, with values of any
    /// kind, through the fuzz body (a workspace file may only tighten).
    #[test]
    fn policy_file_shaped(parts in prop::collection::vec((
        "(trusted|TRUSTED|deny_tools|denyTools|DenyToolPrefixes|safeEnv|budget|hooks|x)",
        "(true|false|null|-?[0-9]{1,3}(\\.[0-9])?|\"[a-zA-Z_]{0,8}\"|\\[\"bash\",\" \",1\\]|\\[\"MCP__\"\\]|\\{\"(max_?)?(tool_?calls|total_?cost_?usd|session_?turns|total_?tokens)\":-?[0-9]{1,4}(\\.5)?\\})",
    ), 0..6)) {
        let body: Vec<String> = parts.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
        fuzzing::policy_file(format!("{{{}}}", body.join(",")).as_bytes());
    }

    #[test]
    fn guard_any_bytes(data in prop::collection::vec(any::<u8>(), 0..120)) {
        fuzzing::guard(&data);
    }

    /// Phrase-shaped text: the gate's own words in any case and order, with spaces, dots, format characters and bytes
    /// that are not UTF-8 between them.
    #[test]
    fn guard_shaped(words in prop::collection::vec(
        "(ignore|disregard|you|are|now|no longer|all|previous|instructions|system prompt|reveal|token|password|sk-[a-z]{20}|AKIA[A-Z]{16}|: |=|\\.|\n|\u{200B}|\u{FEFF}|\u{E0041}|é|K)",
        0..10,
    ), sep in "( |  |\t|\u{A0})") {
        fuzzing::guard(words.join(&sep).as_bytes());
    }

    #[test]
    fn cache_file_any_bytes(data in prop::collection::vec(any::<u8>(), 0..120)) {
        fuzzing::cache_file(&data);
    }

    /// Cache-shaped files: a namespace and entries of any JSON value, through the round trip.
    #[test]
    fn cache_file_shaped(ns in "(ns|NS|)", vals in prop::collection::vec(any_value(), 0..4)) {
        let entries: Vec<String> = vals
            .iter()
            .enumerate()
            .map(|(i, v)| format!("\"k{i}\":{}", sarina::gojson::any_json(v, false).unwrap_or_else(|_| "null".into())))
            .collect();
        let doc = format!(r#"{{"namespace":"{ns}","entries":{{{}}}}}"#, entries.join(","));
        fuzzing::cache_file(doc.as_bytes());
    }

    #[test]
    fn model_text_any_bytes(data in prop::collection::vec(any::<u8>(), 0..120)) {
        fuzzing::model_text(&data);
    }

    /// Answers shaped like tool calls: tags, fences, quotes of both kinds, cut anywhere.
    #[test]
    fn model_text_shaped(parts in prop::collection::vec(
        "(<tool_call>|</tool_call>|<\\|python_tag\\|>|<\\|eot_id\\|>|<think>|</think>|```(json)?\\n?|\\{|\\}|\\[|\\]|,|:|\"name\"|\"bash\"|'read_file'|\"arguments\"|\"parameters\"|\"function\"|\"\\{\\\\\"a\\\\\": 1\"|null|1|\\n| |\\\\)",
        0..24,
    ), cut in 0usize..200) {
        let s = parts.concat();
        let cut = (0..=cut.min(s.len())).rev().find(|i| s.is_char_boundary(*i)).unwrap_or(0);
        fuzzing::model_text(s.as_bytes());
        fuzzing::model_text(&s.as_bytes()[..cut]);
    }

    /// Whatever follows `sudo `, the line is refused by the privileged check (unless the word is data, which a plain word
    /// after sudo never is).
    #[test]
    fn sudo_anything_is_refused(rest in "[a-zA-Z0-9 ._/=-]{0,40}") {
        let line = format!("sudo {rest}");
        prop_assert!(sarina::tools::destructive::check_privileged(line.as_bytes()).blocked);
    }
}

/// Every command line of the fixtures, and each one with one byte changed, through the fuzz bodies.
#[test]
fn fixture_inputs_mutated() {
    let mut n = 0;
    for c in common::cases("shellparse") {
        let src = common::bytes(&c["in"]);
        fuzzing::shellparse(&src);
        fuzzing::bash_guards(&src);
        for (i, b) in [0u8, b'\n', b'\'', b'$', b'(', b')'].iter().enumerate() {
            if !src.is_empty() {
                let mut m = src.clone();
                let at = (i * 7919 + src.len() / 2) % src.len();
                m[at] = *b;
                fuzzing::shellparse(&m);
                fuzzing::bash_guards(&m);
                n += 1;
            }
        }
    }
    assert!(n > 10_000);
}

/// A serde_json value as Go's `any` (numbers as float64, as Go reads them).
fn common_any(v: &serde_json::Value) -> Any {
    match v {
        serde_json::Value::Null => Any::Null,
        serde_json::Value::Bool(b) => Any::Bool(*b),
        serde_json::Value::Number(n) => Any::Number(n.as_f64().unwrap()),
        serde_json::Value::String(s) => Any::String(s.clone()),
        serde_json::Value::Array(a) => Any::Array(a.iter().map(common_any).collect()),
        serde_json::Value::Object(m) => Any::Object(m.iter().map(|(k, x)| (k.clone(), common_any(x))).collect()),
    }
}

/// Every session file of the fixture, and the made-up one with bytes changed and values swapped, through the fuzz body.
#[test]
fn session_files_mutated() {
    let cases = common::cases("session");
    let base = common::bytes(&cases[0]["in"]["file"]);
    for c in &cases {
        if let Some(f) = c["in"].get("file") {
            fuzzing::session(&common::bytes(f));
        }
    }
    let swaps: [&[u8]; 8] = [b"null", b"1e400", b"\"x\"", b"[]", b"{}", b"-1", b"\"\\ud800\"", b"true"];
    let mut n = 0;
    for at in (0..base.len()).step_by(7) {
        let mut m = base.clone();
        m[at] = b"\"{}[],:0a\\ \x00\xff"[at % 13];
        fuzzing::session(&m);
        // a value swapped in after a colon
        if base[at] == b':' {
            for s in swaps {
                let mut m = base[..=at].to_vec();
                m.extend_from_slice(s);
                let rest = &base[at + 1..];
                let end = rest.iter().position(|&c| c == b',' || c == b'\n').unwrap_or(rest.len());
                m.extend_from_slice(&rest[end..]);
                fuzzing::session(&m);
                n += 1;
            }
        }
    }
    assert!(n > 100, "{n}");
}
