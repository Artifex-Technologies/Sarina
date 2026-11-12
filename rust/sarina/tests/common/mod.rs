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

/// The Go test's RawServer: one connection, one request read, the bytes answered after `delay`; with `open` the
/// connection is held until `release` is dropped.
pub fn raw_server(
    response: Vec<u8>,
    delay: std::time::Duration,
    open: bool,
) -> (String, std::sync::mpsc::Receiver<Value>, std::sync::mpsc::Sender<()>) {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::mpsc;
    use std::time::Duration;
    let ln = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = ln.local_addr().unwrap().to_string();
    let (tx, rx) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let Ok((c, _)) = ln.accept() else { return };
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut r = BufReader::new(c.try_clone().unwrap());
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let (mut ctype, mut n) = (String::new(), 0usize);
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                match k.to_ascii_lowercase().as_str() {
                    "content-type" => ctype = v.trim().to_string(),
                    "content-length" => n = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
        }
        let mut body = vec![0u8; n.min(1 << 20)];
        let _ = r.read_exact(&mut body);
        let parts: Vec<&str> = line.trim_end().splitn(3, ' ').collect();
        let _ = tx.send(json!({"method": parts[0], "uri": parts.get(1).copied().unwrap_or(""), "contentType": ctype,
            "body": text(&body)}));
        std::thread::sleep(delay);
        let mut c = c;
        let _ = c.write_all(&response);
        if open {
            let _ = wait.recv();
        }
    });
    (addr, rx, release)
}
