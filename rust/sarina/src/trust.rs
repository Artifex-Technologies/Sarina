//! Go: `trust` (`trust.go`, `contained.go`): Sarina's behaviour files checked against Heimdall's signed list of approved
//! files (ed25519 over `manifest.json`, the key in `owner.pub`, both in Heimdall's root-owned folder), and Heimdall's
//! containment lock. Only in "enforce" mode is anything refused; files under /usr belong to packages and are not checked.

use crate::gopath;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::OnceLock;

/// Heimdall's trust folder.
pub const DEFAULT_DIR: &str = "/etc/heimdall/trust";
/// Overrides the folder (tests).
pub const ENV_DIR: &str = "SARINA_TRUST_DIR";
/// Heimdall's lock: while it exists Sarina does not start.
pub const CONTAINED_PATH: &str = "/etc/heimdall/contained";
/// Overrides the lock's path (tests).
pub const ENV_CONTAINED: &str = "SARINA_CONTAINED_PATH";

/// Go's `base64.StdEncoding.DecodeString`: padding required, newlines (\r, \n) skipped, anything else outside the alphabet
/// refused; bits left over in the last group are ignored. From Heimdall's `trust.rs`.
pub fn std_base64(s: &[u8]) -> Option<Vec<u8>> {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b: Vec<u8> = s.iter().copied().filter(|c| *c != b'\r' && *c != b'\n').collect();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, q) in b.chunks(4).enumerate() {
        let last = i == b.len() / 4 - 1;
        let pad = q.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (j, c) in q.iter().enumerate() {
            let v = if j >= 4 - pad { 0 } else { A.iter().position(|a| a == c)? as u32 };
            n = n << 6 | v;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

/// Go's `base64.StdEncoding.EncodeToString`.
pub fn std_base64_encode(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for j in 0..4 {
            out.push(if j <= c.len() { A[(n >> (18 - 6 * j)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

/// Go's `ed25519.Verify` with Go's length check of the key: true only for a 32-byte key and a valid 64-byte signature.
pub fn verify(public: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    public.len() == 32
        && sig.len() == 64
        && ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public).verify(msg, sig).is_ok()
}

static MANIFEST: Ty = Ty::Struct(r#"struct { Files map[string]string "json:\"files\"" }"#, "", &[Field("files", &Ty::Map(&Ty::Str))]);

struct Loaded {
    mode: String,
    files: BTreeMap<String, String>,
    load_err: &'static str,
}

/// Go's `Checker`: reads the mode and the signed list once, on first use.
pub struct Checker {
    dir: Vec<u8>,
    loaded: OnceLock<Loaded>,
}

/// Go's `Default`'s folder: Heimdall's, or SARINA_TRUST_DIR.
pub fn default_dir(getenv: &dyn Fn(&str) -> String) -> String {
    let d = getenv(ENV_DIR);
    if d.is_empty() {
        DEFAULT_DIR.into()
    } else {
        d
    }
}

impl Checker {
    pub fn new(dir: &[u8]) -> Checker {
        Checker { dir: dir.to_vec(), loaded: OnceLock::new() }
    }

    fn read(&self, name: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(gopath::os_path(&gopath::join(&[&self.dir, name.as_bytes()])))
    }

    fn load(&self) -> &Loaded {
        self.loaded.get_or_init(|| {
            let mut l = Loaded { mode: "off".into(), files: BTreeMap::new(), load_err: "" };
            let Ok(b) = self.read("mode") else { return l };
            l.mode = gotext::lossy(gotext::trim_space_bytes(&b));
            if l.mode != "enforce" {
                return l;
            }
            let (Ok(pub_text), Ok(body), Ok(sig_text)) = (self.read("owner.pub"), self.read("manifest.json"), self.read("manifest.sig"))
            else {
                l.load_err = "the signed list of approved files cannot be read";
                return l;
            };
            let public = std_base64(gotext::trim_space_bytes(&pub_text));
            let sig = std_base64(gotext::trim_space_bytes(&sig_text));
            let ok = matches!((&public, &sig), (Some(p), Some(s)) if verify(p, &body, s));
            if !ok {
                l.load_err = "the signed list of approved files does not match its signature";
                return l;
            }
            let (v, err) = gostruct::unmarshal(&MANIFEST, &body);
            if err.is_some() {
                l.load_err = "the signed list of approved files cannot be read";
                return l;
            }
            if let Some(Val::Map(Some(m))) = gostruct::field(&MANIFEST, &v, "files") {
                for (k, x) in m {
                    if let Val::Str(s) = x {
                        l.files.insert(k.clone(), s.clone());
                    }
                }
            }
            l
        })
    }

    /// Whether unsigned files are refused.
    pub fn enforcing(&self) -> bool {
        self.load().mode == "enforce"
    }

    /// Why a behaviour file must not be used, or "" when it may (always "" unless enforcing).
    pub fn refuse(&self, path: &[u8]) -> String {
        if !self.enforcing() {
            return String::new();
        }
        let abs = gopath::abs(path).unwrap_or_else(|_| path.to_vec());
        if abs.starts_with(b"/usr/") {
            return String::new();
        }
        let l = self.load();
        if !l.load_err.is_empty() {
            return format!("{} (heimdalld trust status)", l.load_err);
        }
        // the list's keys are Go strings read from JSON: always UTF-8, so a path that is not cannot be on it
        let key = std::str::from_utf8(&abs).ok();
        let Some(want) = key.and_then(|k| l.files.get(k)) else {
            return format!(
                "not approved: it is not on the signed list (the owner approves it with: sudo heimdalld trust sign {})",
                gotext::lossy(&abs)
            );
        };
        let mut f = match std::fs::File::open(gopath::os_path(&abs)) {
            Ok(f) => f,
            Err(e) => return format!("cannot be checked: {}", crate::goerr::path("open", &abs, &e)),
        };
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => h.update(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return format!("cannot be checked: {}", crate::goerr::path("read", &abs, &e)),
            }
        }
        let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if &got != want {
            return "not approved: it changed since it was signed (sudo heimdalld trust restore puts back the approved version)".into();
        }
        String::new()
    }
}

static LOCK: Ty = Ty::Struct(r#"struct { Reason string "json:\"reason\"" }"#, "", &[Field("reason", &Ty::Str)]);

/// Go's `Contained`: whether Heimdall has contained Sarina, and why (SARINA_CONTAINED_PATH, else Heimdall's lock).
pub fn contained(getenv: &dyn Fn(&str) -> String) -> (bool, String) {
    let mut p = getenv(ENV_CONTAINED);
    if p.is_empty() {
        p = CONTAINED_PATH.into();
    }
    let Ok(b) = std::fs::read(&p) else { return (false, String::new()) };
    let (v, err) = gostruct::unmarshal(&LOCK, &b);
    let reason = gostruct::str_field(&LOCK, &v, "reason");
    if err.is_some() || reason.is_empty() {
        return (true, "no reason given".into());
    }
    (true, reason.to_string())
}
