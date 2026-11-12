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
        let Any::Object(schema) = schema else { unreachable!() };
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
