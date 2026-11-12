//! Go: `go/tools/private.go`. Folders the owner keeps from Sarina: the credential stores of the home folder (always) and
//! SARINA_PRIVATE_DIRS (separated by ":", "~" allowed). No file tool reads, lists, searches or writes inside them.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;

use crate::gopath;
use crate::gotext::trim_space_bytes;

/// The places in a home folder where logins, keys, tokens and browser sessions live, relative to it; a "*" is matched once
/// per home folder.
pub const CREDENTIAL_STORES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".password-store",
    ".local/share/keyrings",
    ".pki",
    ".aws",
    ".azure",
    ".kube",
    ".docker/config.json",
    ".claude",
    ".claude.json",
    ".config/gh",
    ".config/hub",
    ".config/rclone",
    ".config/gcloud",
    ".npmrc",
    ".pypirc",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".config/labyrinth/sarina.env",
    ".sarina/sarina.env",
    ".config/Proton*",
    ".config/proton*",
    ".config/me.proton.*",
    ".local/share/Proton*",
    ".mozilla",
    ".thunderbird",
    ".config/vivaldi*",
    ".config/chromium",
    ".config/google-chrome*",
    ".config/BraveSoftware",
    ".config/Signal",
    ".config/discord",
    ".config/Bitwarden",
    ".config/KeePassXC",
    ".var/app",
];

/// Go's `os.UserHomeDir` on Linux: $HOME, or "" (Go's error) when it is empty.
pub fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// The stores found for a home folder, kept until the home folder changes (as Go's cache keeps them).
static STORES: Mutex<Option<(String, Vec<Vec<u8>>)>> = Mutex::new(None);

fn credential_store_dirs(home: &str) -> Vec<Vec<u8>> {
    let mut cache = STORES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((h, s)) = cache.as_ref() {
        if h == home {
            return s.clone();
        }
    }
    let mut stores = Vec::new();
    for rel in CREDENTIAL_STORES {
        let p = gopath::join(&[home.as_bytes(), rel.as_bytes()]);
        let matches = if rel.contains('*') { glob_last(&p) } else { vec![p] };
        for m in matches {
            stores.push(gopath::eval_symlinks(&m).unwrap_or(m));
        }
    }
    *cache = Some((home.to_owned(), stores.clone()));
    stores
}

/// Go's `filepath.Glob` for a pattern whose only metacharacter is `*` in its last element (the stores above): the folder's
/// entries that match, in sorted order.
fn glob_last(pattern: &[u8]) -> Vec<Vec<u8>> {
    let dir = gopath::dir(pattern);
    let file = gopath::base(pattern);
    let Ok(md) = std::fs::metadata(OsStr::from_bytes(&dir)) else { return vec![] };
    if !md.is_dir() {
        return vec![];
    }
    let Ok(rd) = std::fs::read_dir(OsStr::from_bytes(&dir)) else { return vec![] };
    let mut names: Vec<Vec<u8>> = rd.flatten().map(|e| e.file_name().as_bytes().to_vec()).collect();
    names.sort();
    names.into_iter().filter(|n| star_match(&file, n)).map(|n| gopath::join(&[&dir, &n])).collect()
}

/// `*` matches any run of bytes (an entry name holds no '/'); everything else matches itself.
fn star_match(pat: &[u8], name: &[u8]) -> bool {
    match pat.iter().position(|&c| c == b'*') {
        None => pat == name,
        Some(i) => {
            let (head, rest) = (&pat[..i], &pat[i + 1..]);
            name.starts_with(head) && (head.len()..=name.len()).any(|j| star_match(rest, &name[j..]))
        }
    }
}

fn private_dirs() -> Vec<Vec<u8>> {
    let home = home_dir();
    let mut out = Vec::new();
    if !home.is_empty() {
        out.extend(credential_store_dirs(&home));
    }
    let env = std::env::var_os("SARINA_PRIVATE_DIRS").unwrap_or_default();
    for d in env.as_bytes().split(|&c| c == b':') {
        let d = trim_space_bytes(d);
        if d.is_empty() {
            continue;
        }
        let d = match d.strip_prefix(b"~/") {
            Some(rest) => gopath::join(&[home.as_bytes(), rest]),
            None => d.to_vec(),
        };
        if let Ok(abs) = gopath::abs(&d) {
            out.push(gopath::eval_symlinks(&abs).unwrap_or(abs));
        }
    }
    out
}

/// Go's `IsPrivate`: whether path is inside one of the private folders.
pub fn is_private(path: &[u8]) -> bool {
    let dirs = private_dirs();
    if dirs.is_empty() {
        return false;
    }
    let Ok(abs) = gopath::abs(path) else { return true };
    let abs = gopath::eval_symlinks(&abs).unwrap_or(abs);
    dirs.iter().any(|d| abs == *d || abs.len() > d.len() && abs.starts_with(d) && abs[d.len()] == b'/')
}

/// Go's `privateGuard`.
pub fn private_guard(given: &str, resolved: &[u8], verb: &str) -> Result<(), String> {
    if is_private(resolved) {
        return Err(format!(
            "refusing to {verb} {given}: the owner keeps that folder private from me (credential stores, browser profiles and the \
             folders in SARINA_PRIVATE_DIRS are never read)"
        ));
    }
    Ok(())
}
