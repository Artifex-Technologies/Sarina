//! Go: `analysiscache`: a content-hash-keyed cache for expensive, deterministic analysis, `{sha256(content): value}` as
//! JSON on disk under a namespace the caller gives; a missing, damaged or other-namespace file is an empty cache.
//!
//! As Go: a value Go's encoder refuses (NaN, an infinity) stays in memory once set, so every later `set` of that cache
//! fails with the same error and the file is not written again.

use crate::goany::Any;
use crate::goerr;
use crate::gojson;
use crate::goos::{create_temp, mkdir_all};
use crate::gopath;
use crate::gostruct::{self, Field, Ty, Val};
use crate::session;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Mutex;

static FILE: Ty = Ty::Struct(
    r#"struct { Namespace string "json:\"namespace\""; Entries map[string]interface {} "json:\"entries\"" }"#,
    "",
    &[Field("namespace", &Ty::Str), Field("entries", &Ty::Map(&Ty::Any))],
);

/// A cache file's entries when it reads and is of `namespace`, else none (Go's `New` after reading the file).
pub fn read_entries(raw: &[u8], namespace: &str) -> BTreeMap<String, Any> {
    let mut entries = BTreeMap::new();
    let (v, err) = gostruct::unmarshal(&FILE, raw);
    if err.is_none() && gostruct::str_field(&FILE, &v, "namespace") == namespace {
        if let Some(Val::Map(Some(m))) = gostruct::field(&FILE, &v, "entries") {
            for (k, x) in m {
                if let Val::Any(a) = x {
                    entries.insert(k.clone(), a.clone());
                }
            }
        }
    }
    entries
}

/// The file `set` writes for these entries (Go's `session.EncodeJSON` of {"namespace", "entries"}).
pub fn encode_file(namespace: &str, entries: &BTreeMap<String, Any>) -> Result<String, String> {
    let doc = Any::Object(BTreeMap::from([
        ("entries".to_string(), Any::Object(entries.clone())),
        ("namespace".to_string(), Any::String(namespace.to_string())),
    ]));
    Ok(session::encode_json(&gojson::any_json(&doc, false).map_err(|e| e.0)?))
}

/// One cache file (Go's `Cache`). Safe for concurrent use.
pub struct Cache {
    path: Vec<u8>,
    namespace: String,
    entries: Mutex<BTreeMap<String, Any>>,
}

/// Go's `DefaultDir`: ~/.sarina/cache, or "" when the home folder is unknown (`os.UserHomeDir`: $HOME).
pub fn default_dir(getenv: &dyn Fn(&str) -> String) -> String {
    let home = getenv("HOME");
    if home.is_empty() {
        return String::new();
    }
    String::from_utf8_lossy(&gopath::join(&[home.as_bytes(), b".sarina", b"cache"])).into_owned()
}

fn key(content: &[u8]) -> String {
    Sha256::digest(content).iter().map(|b| format!("{b:02x}")).collect()
}

impl Cache {
    /// Go's `New`: the cache at `path`; a missing, damaged or other-namespace file is empty, never an error.
    pub fn new(path: &[u8], namespace: &str) -> Cache {
        let entries = std::fs::read(gopath::os_path(path)).map(|raw| read_entries(&raw, namespace)).unwrap_or_default();
        Cache { path: path.to_vec(), namespace: namespace.to_string(), entries: Mutex::new(entries) }
    }

    /// The cached value for `content`.
    pub fn get(&self, content: &[u8]) -> Option<Any> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(&key(content)).cloned()
    }

    /// Stores a value and writes the cache (to a temporary file in the same folder, then moved into place).
    pub fn set(&self, content: &[u8], value: Any) -> Result<(), String> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(key(content), value);
        let data = encode_file(&self.namespace, &entries)?;
        let dir = gopath::dir(&self.path);
        mkdir_all(&dir)?;
        let (mut f, name) = create_temp(&dir, ".cache-*.tmp")?;
        let werr = f.write_all(data.as_bytes()).map_err(|e| goerr::path("write", &name, &e));
        drop(f);
        if let Err(e) = werr {
            let _ = std::fs::remove_file(gopath::os_path(&name));
            return Err(e);
        }
        std::fs::rename(gopath::os_path(&name), gopath::os_path(&self.path))
            .map_err(|e| format!("rename {} {}: {}", String::from_utf8_lossy(&name), String::from_utf8_lossy(&self.path), goerr::text(&e)))
    }
}
