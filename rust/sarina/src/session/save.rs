//! Go: `session.Save` and `writeAtomic` (session.go) and `MigrateDir` (migrate.go): a session written atomically (a
//! temporary file in the same folder, its mode from SARINA_SESSION_FILE_MODE, then a rename), and every saved session of a
//! folder brought up to date (retired tool names), the original kept as `<name>.bak` the first time.
//!
//! A session id that cannot name a file (a slash, a NUL byte, empty) is refused by `save` before anything is written, and
//! `migrate_dir` reports such a file (and any file it cannot write) and goes on with the rest. As Go: a `.bak` that
//! exists as anything (a folder too) means no new backup, and a session file that is a link is replaced by a regular file.

use super::{file_mode, parse_raw, path, Session};
use crate::goerr;
use crate::goos::{create_temp, mkdir_all, write_file};
use crate::gopath;
use crate::gotext;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

/// Go's `writeAtomic`: `data` to `dest` through a temporary file `<id>.*.tmp` in the same folder.
pub fn write_atomic(dest: &[u8], id: &str, data: &[u8], getenv: &dyn Fn(&str) -> String) -> Result<(), String> {
    let (mut f, tmp) = create_temp(&gopath::dir(dest), &format!("{id}.*.tmp"))?;
    let cleanup = || {
        let _ = std::fs::remove_file(gopath::os_path(&tmp));
    };
    if let Err(e) = f.write_all(data) {
        drop(f);
        cleanup();
        return Err(goerr::path("write", &tmp, &e));
    }
    drop(f);
    let _ = std::fs::set_permissions(gopath::os_path(&tmp), std::fs::Permissions::from_mode(file_mode(getenv)));
    if let Err(e) = std::fs::rename(gopath::os_path(&tmp), gopath::os_path(dest)) {
        cleanup();
        return Err(format!("rename {} {}: {}", String::from_utf8_lossy(&tmp), String::from_utf8_lossy(dest), goerr::text(&e)));
    }
    Ok(())
}

/// Go's `CheckID`: why `id` cannot name a session file (`<id>.json` in the session folder), or Ok.
pub fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("session id is empty".into());
    }
    if id.contains(['/', '\0']) {
        return Err(format!("session id {} cannot name a file: it holds a slash or a NUL byte", gotext::quote(id)));
    }
    Ok(())
}

impl Session {
    /// Go's `Save`: the session written to `<dir>/<id>.json` (the folder made first); the path. An id that cannot name a
    /// file is refused before anything is written.
    pub fn save(&self, dir: &str, getenv: &dyn Fn(&str) -> String) -> Result<String, String> {
        check_id(&self.id)?;
        let data = self.marshal().map_err(|e| e.0)?;
        mkdir_all(dir.as_bytes())?;
        let dest = path(dir, &self.id);
        write_atomic(dest.as_bytes(), &self.id, data.as_bytes(), getenv)?;
        Ok(dest)
    }
}

/// What `migrate_dir` found (Go's `MigrateReport`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MigrateReport {
    pub scanned: i64,
    pub changed: i64,
    pub skipped: i64,
    pub calls: i64,
    pub results: i64,
    /// names of changed files
    pub files: Vec<String>,
    /// files that needed a change but could not be written ("<name>: why"), left as they were
    pub failed: Vec<String>,
}

/// Go's `MigrateDir`: every `*.json` in `dir` brought up to date; with `dry_run` nothing is written. A file that cannot
/// be written is reported in `failed` and left as it was; the error is only for a folder that cannot be read.
pub fn migrate_dir(dir: &str, dry_run: bool, getenv: &dyn Fn(&str) -> String) -> (MigrateReport, Option<String>) {
    let mut r = MigrateReport::default();
    let rd = match std::fs::read_dir(gopath::os_path(dir.as_bytes())) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (r, None),
        Err(e) => return (r, Some(goerr::path("open", dir.as_bytes(), &e))),
    };
    // Go's ReadDir: sorted by name, the type from the folder entry (a link is not a folder)
    let mut entries: Vec<(Vec<u8>, bool)> = Vec::new();
    for e in rd.flatten() {
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        entries.push((e.file_name().as_encoded_bytes().to_vec(), is_dir));
    }
    entries.sort();
    for (name, is_dir) in entries {
        if is_dir || !name.ends_with(b".json") {
            continue;
        }
        let p = gopath::join(&[dir.as_bytes(), &name]);
        let Ok(raw) = super::readfile::read_file(&p) else {
            r.skipped += 1;
            continue;
        };
        let Ok(mut s) = parse_raw(&raw) else {
            r.skipped += 1;
            continue;
        };
        r.scanned += 1;
        let m = s.migrate_retired_tools();
        if !m.changed() {
            continue;
        }
        let name_s = String::from_utf8_lossy(&name).into_owned();
        if let Err(e) = check_id(&s.id) {
            r.failed.push(format!("{name_s}: {e}"));
            continue;
        }
        if !dry_run {
            if let Err(e) = migrate_file(&p, &raw, &s, getenv) {
                r.failed.push(format!("{name_s}: {e}"));
                continue;
            }
        }
        r.changed += 1;
        r.calls += m.calls;
        r.results += m.results;
        r.files.push(name_s);
    }
    (r, None)
}

/// Go's `migrateFile`: the original kept as `<path>.bak` the first time, then the migrated session written over it.
fn migrate_file(p: &[u8], raw: &[u8], s: &Session, getenv: &dyn Fn(&str) -> String) -> Result<(), String> {
    let mut bak = p.to_vec();
    bak.extend_from_slice(b".bak");
    if std::fs::metadata(gopath::os_path(&bak)).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        write_file(&bak, raw, 0o600)?;
    }
    let data = s.marshal().map_err(|e| e.0)?;
    write_atomic(p, &s.id, data.as_bytes(), getenv)
}
