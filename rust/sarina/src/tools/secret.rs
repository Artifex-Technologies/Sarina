//! Go: `go/tools/secret.go`. What the file tools refuse to read or write because it looks like it holds credentials: .env and
//! .env.* (including .env.example), *.pem and *.key, default SSH identities, credentials/secrets files, .netrc and friends,
//! by name, on either path separator, without regard to case; and a hard link to such a file in the same folder.

use std::os::unix::fs::MetadataExt;
use std::sync::LazyLock;

use regex::Regex;

use crate::gopath;
use crate::gotext::to_lower_bytes;

static SECRET_CREDENTIALS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^credentials(\.(json|ya?ml))?$").expect("regex"));
static SECRET_SECRETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^secrets?(\.(json|ya?ml|toml))?$").expect("regex"));
static SECRET_SSH_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^id_(rsa|ed25519|ecdsa|dsa)(\.pub)?$").expect("regex"));

/// Whether a single file name looks like it holds credentials.
pub fn secret_name(name: &[u8]) -> bool {
    let lower = to_lower_bytes(name);
    // Windows quietly ignores trailing dots/spaces and addresses alternate data streams as "name:stream", so ".env. " and
    // ".env::$DATA" are the same file.
    let mut b = lower.trim_end_matches(['.', ' ']);
    if let Some(i) = b.find(':') {
        if i > 0 {
            b = b[..i].trim_end_matches(['.', ' ']);
        }
    }
    if b.is_empty() {
        return false;
    }
    b == ".env"
        || b.starts_with(".env.")
        || b == ".envrc"
        || [".pem", ".key", ".p12", ".pfx", ".ppk"].iter().any(|s| b.ends_with(s))
        || SECRET_SSH_KEY.is_match(b)
        || SECRET_CREDENTIALS.is_match(b)
        || SECRET_SECRETS.is_match(b)
        || matches!(b, ".netrc" | "_netrc" | ".git-credentials" | ".pgpass")
}

/// Go's `IsSecretPath`: whether p names a secret-bearing file. It looks only at the name; callers check both the path the
/// model gave and the symlink-resolved path.
pub fn is_secret_path(p: &[u8]) -> bool {
    let last = p.rsplit(|&c| c == b'/' || c == b'\\').next().unwrap_or(p);
    secret_name(last)
}

/// Go's `secretGuard`: refuses access to secret-bearing files. given is the path as the model wrote it, resolved the fully
/// resolved one: a harmless-looking name that is a symlink to a .env is caught through resolved, and a link named .env that
/// points at something innocent is still refused through given. A hard link to a secret file in the same folder is caught
/// by identity.
pub fn secret_guard(given: &str, resolved: &[u8], verb: &str) -> Result<(), String> {
    if is_secret_path(given.as_bytes()) || is_secret_path(resolved) || hard_linked_to_secret(resolved) {
        return Err(format!(
            "refusing to {verb} {given}: it looks like a credentials file (.env, key, certificate or similar). File tools never put \
             secret files into the conversation or change them. If the operator really wants this, they can do it themselves or \
             tell you to use bash explicitly"
        ));
    }
    Ok(())
}

/// Go's `hardLinkedToSecret`: a regular file that is a hard link to a secret file in the same folder.
pub fn hard_linked_to_secret(resolved: &[u8]) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let path = |b: &[u8]| std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b));
    let Ok(st) = std::fs::metadata(path(resolved)) else { return false };
    if !st.is_file() {
        return false;
    }
    let dir = gopath::dir(resolved);
    let Ok(entries) = std::fs::read_dir(path(&dir)) else { return false };
    entries.flatten().any(|e| {
        let name = e.file_name();
        secret_name(name.as_bytes())
            && std::fs::metadata(path(&gopath::join(&[&dir, name.as_bytes()]))).is_ok_and(|o| o.dev() == st.dev() && o.ino() == st.ino())
    })
}
