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

/// The Go test's RawUnixServer: one connection per response, in order; the requests seen so far. "@big:N" is a 200 with N
/// spaces of body.
pub fn raw_unix_server(path: &std::path::Path, responses: Vec<Vec<u8>>) -> std::sync::Arc<std::sync::Mutex<Vec<Value>>> {
    use std::io::{BufRead, BufReader, Read, Write};
    let ln = std::os::unix::net::UnixListener::bind(path).unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for resp in responses {
            let Ok((c, _)) = ln.accept() else { return };
            c.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
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
            s2.lock().unwrap().push(json!({"method": parts[0], "uri": parts.get(1).copied().unwrap_or(""), "contentType": ctype,
                "body": text(&body)}));
            let resp = match std::str::from_utf8(&resp).ok().and_then(|s| s.strip_prefix("@big:")) {
                Some(n) => {
                    let k: usize = n.parse().unwrap();
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {k}\r\n\r\n{}", " ".repeat(k)).into_bytes()
                }
                None => resp,
            };
            let mut c = c;
            let _ = c.write_all(&resp);
        }
    });
    seen
}

/// A step of the Go test's scripted server (parity.Step): wait, send, fill with 'x', hold until the test ends.
pub enum Step {
    Wait(u64),
    Send(Vec<u8>),
    Fill(usize),
    Hold,
}

/// A script as the fixture holds it.
pub fn steps(v: &Value) -> Vec<Vec<Step>> {
    v.as_array()
        .map(|conns| {
            conns
                .iter()
                .map(|c| {
                    c.as_array()
                        .unwrap()
                        .iter()
                        .map(|s| {
                            if s.get("hold").is_some() {
                                Step::Hold
                            } else if let Some(w) = s.get("wait") {
                                Step::Wait(w.as_u64().unwrap())
                            } else if let Some(f) = s.get("fill") {
                                Step::Fill(f.as_u64().unwrap() as usize)
                            } else {
                                Step::Send(bytes(&s["send"]))
                            }
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The Go test's RawSeq: the n-th connection answered with script[n], each on its own thread, TLS with `tls` when given.
/// The requests seen (method, URI, Content-Type, Authorization, x-api-key, anthropic-version, body; {"none": true} when no
/// request could be read). Dropping the guard ends held connections.
pub struct SeqServer {
    pub addr: String,
    pub seen: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for SeqServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // wake the accept loop so it sees the stop
        let _ = std::net::TcpStream::connect(&self.addr);
    }
}

pub fn raw_seq(script: Vec<Vec<Step>>, tls: Option<std::sync::Arc<rustls::ServerConfig>>) -> SeqServer {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    let ln = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = ln.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (seen2, stop2) = (seen.clone(), stop.clone());
    let script = Arc::new(Mutex::new(script.into_iter().map(Some).collect::<Vec<_>>()));
    std::thread::spawn(move || {
        for n in 0.. {
            let Ok((c, _)) = ln.accept() else { return };
            if stop2.load(Ordering::SeqCst) {
                return;
            }
            let steps = script.lock().unwrap().get_mut(n).and_then(Option::take).unwrap_or_default();
            let (seen, stop, tls) = (seen2.clone(), stop2.clone(), tls.clone());
            std::thread::spawn(move || {
                c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut conn: Box<dyn ReadWrite> = match tls {
                    Some(cfg) => Box::new(rustls::StreamOwned::new(rustls::ServerConnection::new(cfg).unwrap(), c)),
                    None => Box::new(c),
                };
                let mut r = BufReader::new(&mut conn);
                let mut line = String::new();
                let ok = r.read_line(&mut line).map(|n| n > 0).unwrap_or(false);
                let req = if !ok {
                    json!({"none": true})
                } else {
                    let mut h: std::collections::HashMap<String, String> = Default::default();
                    loop {
                        let mut l = String::new();
                        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" {
                            break;
                        }
                        if let Some((k, v)) = l.split_once(':') {
                            h.entry(k.to_ascii_lowercase()).or_insert_with(|| v.trim().to_string());
                        }
                    }
                    let n: usize = h.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let mut body = vec![0u8; n.min(1 << 20)];
                    let _ = r.read_exact(&mut body);
                    let g = |k: &str| h.get(k).cloned().unwrap_or_default();
                    let parts: Vec<&str> = line.trim_end().splitn(3, ' ').collect();
                    json!({"method": parts[0], "uri": parts.get(1).copied().unwrap_or(""), "contentType": g("content-type"),
                        "auth": g("authorization"), "apiKey": g("x-api-key"), "version": g("anthropic-version"), "body": text(&body)})
                };
                drop(r);
                seen.lock().unwrap().push(req.clone());
                if req.get("none").is_some() {
                    return;
                }
                let wait = |d: Duration| {
                    let until = Instant::now() + d;
                    while Instant::now() < until {
                        if stop.load(Ordering::SeqCst) {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    true
                };
                for s in steps {
                    match s {
                        Step::Hold => {
                            while !stop.load(Ordering::SeqCst) {
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                        Step::Wait(ms) => {
                            if !wait(Duration::from_millis(ms)) {
                                return;
                            }
                        }
                        Step::Fill(k) => {
                            let _ = conn.write_all(&vec![b'x'; k]);
                        }
                        Step::Send(b) => {
                            let _ = conn.write_all(&b);
                        }
                    }
                    let _ = conn.flush();
                }
            });
        }
    });
    SeqServer { addr, seen, stop }
}

pub trait ReadWrite: std::io::Read + std::io::Write + Send {}
impl<T: std::io::Read + std::io::Write + Send> ReadWrite for T {}

/// A certificate authority made in the test (P-256 by ring, DER written here), and certificates from it.
pub struct TestCa {
    pub der: Vec<u8>,
    name: String,
    key: ring::signature::EcdsaKeyPair,
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = content.len();
    if n < 128 {
        out.push(n as u8);
    } else {
        let b = n.to_be_bytes();
        let skip = b.iter().take_while(|&&x| x == 0).count();
        out.push(0x80 | (b.len() - skip) as u8);
        out.extend_from_slice(&b[skip..]);
    }
    out.extend_from_slice(content);
    out
}

fn der_seq(parts: &[Vec<u8>]) -> Vec<u8> {
    der(0x30, &parts.concat())
}

const OID_ECDSA_SHA256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

fn der_name(cn: &str) -> Vec<u8> {
    der_seq(&[der(0x31, &der_seq(&[vec![0x06, 0x03, 0x55, 0x04, 0x03], der(0x0c, cn.as_bytes())]))])
}

fn new_key() -> (Vec<u8>, ring::signature::EcdsaKeyPair) {
    use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap();
    (pkcs8.as_ref().to_vec(), key)
}

impl TestCa {
    pub fn new(name: &str) -> TestCa {
        let (_, key) = new_key();
        let basic = der_seq(&[vec![0x06, 0x03, 0x55, 0x1d, 0x13], vec![0x01, 0x01, 0xff], der(0x04, &der_seq(&[vec![0x01, 0x01, 0xff]]))]);
        let der = Self::sign(&key, 1, name, name, &key, &[basic]);
        TestCa { der, name: name.to_string(), key }
    }

    /// A certificate: issuer's key signs subject's public key.
    fn sign(
        issuer_key: &ring::signature::EcdsaKeyPair,
        serial: u8,
        issuer: &str,
        subject: &str,
        subject_key: &ring::signature::EcdsaKeyPair,
        exts: &[Vec<u8>],
    ) -> Vec<u8> {
        use ring::signature::KeyPair;
        let alg = der_seq(&[OID_ECDSA_SHA256.to_vec()]);
        let mut point = vec![0u8];
        point.extend_from_slice(subject_key.public_key().as_ref());
        let spki = der_seq(&[der_seq(&[OID_EC_PUBLIC_KEY.to_vec(), OID_P256.to_vec()]), der(0x03, &point)]);
        let validity = der_seq(&[der(0x17, b"000101000000Z"), der(0x18, b"20991231235959Z")]);
        let tbs = der_seq(&[
            der(0xa0, &der(0x02, &[2])),
            der(0x02, &[serial]),
            alg.clone(),
            der_name(issuer),
            validity,
            der_name(subject),
            spki,
            der(0xa3, &der_seq(exts)),
        ]);
        let sig = issuer_key.sign(&ring::rand::SystemRandom::new(), &tbs).unwrap();
        let mut bits = vec![0u8];
        bits.extend_from_slice(sig.as_ref());
        der_seq(&[tbs, alg, der(0x03, &bits)])
    }

    /// A store trusting only this authority.
    pub fn roots(&self) -> rustls::RootCertStore {
        let mut r = rustls::RootCertStore::empty();
        r.add(rustls::pki_types::CertificateDer::from(self.der.clone())).unwrap();
        r
    }

    /// A server configuration with a certificate from this authority for the IP addresses and DNS names given.
    pub fn server(&self, ips: &[[u8; 4]], names: &[&str]) -> std::sync::Arc<rustls::ServerConfig> {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
        let (pkcs8, key) = new_key();
        let mut alt = Vec::new();
        for ip in ips {
            alt.push(der(0x87, ip));
        }
        for n in names {
            alt.push(der(0x82, n.as_bytes()));
        }
        let san = der_seq(&[vec![0x06, 0x03, 0x55, 0x1d, 0x11], der(0x04, &der_seq(&alt))]);
        let leaf = Self::sign(&self.key, 2, &self.name, "parity server", &key, &[san]);
        let p = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ServerConfig::builder_with_provider(p)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(leaf)], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8)))
            .unwrap();
        std::sync::Arc::new(cfg)
    }
}
