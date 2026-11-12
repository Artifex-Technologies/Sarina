//! Go: `go/testguard`. Keeps an eval run out of the owner's own notes. Under SARINA_TEST=1 (the eval runner sets it for every
//! case) the places Sarina keeps things between conversations (the vault and its Daily note, Talks and Jokes, mood history,
//! reflections, remembered facts) refuse a write outside the test's own folders: SARINA_TEST_ROOT (colon-separated; the
//! runner's work folder) and the system's temporary folder. A case that writes there works as before; a case that would
//! have written into the real vault gets an error instead, and the owner's notes stay as they were.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use crate::gopath;

pub const ENV: &str = "SARINA_TEST";
pub const ROOT_ENV: &str = "SARINA_TEST_ROOT";

fn getenv(name: &str) -> Vec<u8> {
    std::env::var_os(name).map(OsString::into_vec).unwrap_or_default()
}

/// An eval run.
pub fn active() -> bool {
    getenv(ENV) == b"1"
}

/// Go's `os.TempDir` on Linux: $TMPDIR, else /tmp.
pub fn temp_dir() -> Vec<u8> {
    let t = getenv("TMPDIR");
    if t.is_empty() {
        b"/tmp".to_vec()
    } else {
        t
    }
}

/// Ok when not under test, or when path lies inside a test root; otherwise an error naming the refusal.
pub fn allow(path: &[u8]) -> Result<(), String> {
    if !active() {
        return Ok(());
    }
    let p = resolve(path);
    let mut roots = vec![temp_dir()];
    roots.extend(getenv(ROOT_ENV).split(|&c| c == b':').filter(|r| !r.is_empty()).map(<[u8]>::to_vec));
    for r in roots {
        let r = resolve(&r);
        if p == r || p.len() > r.len() && p.starts_with(&r) && p[r.len()] == b'/' {
            return Ok(());
        }
    }
    Err(format!("not written: this is a test run ({ENV}=1), and {} is outside the test's folders", crate::gotext::lossy(path)))
}

/// Makes path absolute and follows links in the part that exists (a link must not lead a test out of its folder).
pub fn resolve(path: &[u8]) -> Vec<u8> {
    let p = match gopath::abs(path) {
        Ok(p) => p,
        Err(_) => return gopath::clean(path),
    };
    let mut rest: Vec<Vec<u8>> = Vec::new();
    let mut q = p.clone();
    loop {
        if let Ok(r) = gopath::eval_symlinks(&q) {
            let mut parts: Vec<&[u8]> = vec![&r];
            parts.extend(rest.iter().map(Vec::as_slice));
            return gopath::join(&parts);
        }
        let d = gopath::dir(&q);
        if q == d {
            return p;
        }
        rest.insert(0, gopath::base(&q));
        q = d;
    }
}
