//! The fixtures the Go side writes (go/internal/parity): reading them and writing texts the way they hold them.
#![allow(dead_code)] // each test file uses part of this

use serde_json::{json, Value};

/// The cases of testdata/parity/<name>.json.
pub fn cases(name: &str) -> Vec<Value> {
    let path = format!("{}/../../testdata/parity/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = serde_json::from_slice(&data).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(v["fixture"], name);
    v["cases"].as_array().expect("cases").clone()
}

/// A text as the fixture holds it: a string, or {"hex": ...} for bytes that are not UTF-8.
pub fn bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::String(s) => s.as_bytes().to_vec(),
        Value::Object(m) => {
            let h = m["hex"].as_str().expect("hex");
            (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).expect("hex digit")).collect()
        }
        _ => panic!("not a text: {v}"),
    }
}

/// Bytes as the fixture writes them.
pub fn text(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => Value::String(s.to_owned()),
        Err(_) => json!({ "hex": b.iter().map(|x| format!("{x:02x}")).collect::<String>() }),
    }
}

/// Compares and counts; on a difference, panics with the case.
pub struct Check {
    pub n: usize,
    pub fails: Vec<String>,
}

impl Check {
    pub fn new() -> Check {
        Check { n: 0, fails: Vec::new() }
    }

    pub fn eq(&mut self, what: impl std::fmt::Display, got: &Value, want: &Value) {
        self.n += 1;
        if got != want && self.fails.len() < 20 {
            self.fails.push(format!("{what}\n  rust: {got}\n  go:   {want}"));
        } else if got != want {
            self.fails.push(String::new());
        }
    }

    pub fn done(self, name: &str) {
        let shown: Vec<_> = self.fails.iter().filter(|f| !f.is_empty()).cloned().collect();
        assert!(self.fails.is_empty(), "{name}: {} of {} differ:\n{}", self.fails.len(), self.n, shown.join("\n"));
        eprintln!("{name}: {} comparisons equal", self.n);
    }
}

/// A Go `any` as the fixture's typed form holds it (parity.Typed).
pub fn typed(v: &sarina::goany::Any) -> Value {
    use sarina::goany::Any;
    match v {
        Any::Null => Value::Null,
        Any::Bool(b) => Value::Bool(*b),
        Any::Number(f) => json!({ "f": format!("{:x}", f.to_bits()) }),
        Any::Num(n) => json!({ "n": n }),
        Any::Int(i) => json!({ "i": i.to_string() }),
        Any::String(s) => json!({ "s": text(s.as_bytes()) }),
        Any::Array(items) => Value::Array(items.iter().map(typed).collect()),
        Any::Object(m) => json!({ "o": m.iter().map(|(k, x)| (k.clone(), typed(x))).collect::<serde_json::Map<_, _>>() }),
    }
}
