//! testdata/parity/save.json: Session.Save (the folder made, the temporary file and the rename, the mode from
//! SARINA_SESSION_FILE_MODE, ids that are not file names) and MigrateDir (dry and real runs, backups, skipped files, a
//! session id with a slash) on made-up folders.

mod common;

use sarina::session::parse;
use sarina::session::save::migrate_dir;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>, temp: &regex::Regex) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let rel = p.strip_prefix(root).unwrap().to_str().unwrap().to_string();
        let md = std::fs::symlink_metadata(&p).unwrap();
        if md.file_type().is_symlink() {
            let target = std::fs::read_link(&p).unwrap();
            out.push(format!("{rel} -> {}", target.to_str().unwrap().replace(root.to_str().unwrap(), "{DIR}")));
        } else if md.is_dir() {
            out.push(format!("{rel}/"));
            walk(root, &p, out, temp);
        } else {
            let b = std::fs::read(&p).unwrap();
            out.push(format!("{} {:o} {} {:016x}", temp.replace_all(&rel, ".N.tmp"), md.permissions().mode() & 0o777, b.len(), fnv(&b)));
        }
    }
}

fn listing(root: &Path, temp: &regex::Regex) -> Value {
    let mut out = Vec::new();
    walk(root, root, &mut out, temp);
    out.sort();
    if out.is_empty() {
        Value::Null
    } else {
        json!(out)
    }
}

#[test]
fn save_matches_go() {
    let mut check = common::Check::new();
    let base = std::env::temp_dir().join(format!("sarina-save-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let temp = regex::Regex::new(r"\.[0-9]+\.tmp").unwrap();
    let cases = common::cases("save");
    let docs = cases.iter().find(|c| c["fn"] == "docs").unwrap()["in"].clone();
    let doc = |name: &str| common::bytes(&docs[name]);
    for (n, c) in cases.iter().enumerate() {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "docs" => continue,
            "save" => {
                let b = base.join(format!("s{n}"));
                std::fs::create_dir_all(b.join("exists")).unwrap();
                std::fs::write(b.join("file"), b"").unwrap();
                std::fs::create_dir_all(b.join("ro")).unwrap();
                std::fs::set_permissions(b.join("ro"), std::fs::Permissions::from_mode(0o555)).unwrap();
                let mode = input[1].as_str().unwrap().to_string();
                let getenv = |k: &str| if k == "SARINA_SESSION_FILE_MODE" { mode.clone() } else { String::new() };
                let s = parse(&doc(input[0].as_str().unwrap())).unwrap();
                let r = s.save(b.join(input[2].as_str().unwrap()).to_str().unwrap(), &getenv);
                std::fs::set_permissions(b.join("ro"), std::fs::Permissions::from_mode(0o755)).unwrap();
                let bs = b.to_str().unwrap();
                let clean = |x: &str| temp.replace_all(&x.replace(bs, "{DIR}"), ".N.tmp").into_owned();
                let (p, e) = match r {
                    Ok(p) => (p, String::new()),
                    Err(e) => (String::new(), e),
                };
                json!([clean(&p), clean(&e), listing(&b, &temp)])
            }
            "migrateDir" => {
                let dir = base.join(format!("m{n}"));
                std::fs::create_dir_all(&dir).unwrap();
                let plain = doc("plain");
                let retired = doc("retired");
                let slash = String::from_utf8(retired.clone())
                    .unwrap()
                    .replacen(r#""session_id": "retired-session""#, r#""session_id": "x/y""#, 1)
                    .into_bytes();
                let files: Vec<(&str, Vec<u8>)> = match input[0].as_str().unwrap() {
                    "mixed" => vec![
                        ("retired-session.json", retired.clone()),
                        ("fixed-session.json", plain.clone()),
                        ("junk.json", b"nope".to_vec()),
                        ("x.notes.md", b"notes".to_vec()),
                        ("sub.json/", vec![]),
                        ("link.json", b"-> retired-session.json".to_vec()),
                        ("empty.json", vec![]),
                        ("other-name.json", retired.clone()),
                    ],
                    "backup" => vec![("retired-session.json", retired.clone()), ("retired-session.json.bak", b"kept".to_vec())],
                    "slash" => vec![("a.json", retired.clone()), ("b.json", slash), ("c.json", retired.clone())],
                    "bakdir" => vec![("retired-session.json", retired.clone()), ("retired-session.json.bak/", vec![])],
                    _ => vec![],
                };
                for (name, body) in files {
                    let p = dir.join(name.trim_end_matches('/'));
                    if name.ends_with('/') {
                        std::fs::create_dir(&p).unwrap();
                    } else if let Some(t) = body.strip_prefix(b"-> ") {
                        std::os::unix::fs::symlink(dir.join(std::str::from_utf8(t).unwrap()), &p).unwrap();
                    } else {
                        std::fs::write(&p, &body).unwrap();
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
                    }
                }
                let getenv = |k: &str| if k == "SARINA_SESSION_FILE_MODE" { "0600".to_string() } else { String::new() };
                let mut runs = Vec::new();
                for _ in 0..2 {
                    let (r, err) = migrate_dir(dir.to_str().unwrap(), input[1] == true, &getenv);
                    let files = if r.files.is_empty() { Value::Null } else { json!(r.files) };
                    let err = err.unwrap_or_default().replace(dir.to_str().unwrap(), "{DIR}");
                    runs.push(json!([r.scanned, r.changed, r.skipped, r.calls, r.results, files, err, listing(&dir, &temp)]));
                }
                json!(runs)
            }
            "migrateOdd" => {
                let b = base.join(format!("o{n}"));
                std::fs::create_dir_all(&b).unwrap();
                std::fs::write(b.join("file"), b"").unwrap();
                let (r, err) = migrate_dir(b.join(input.as_str().unwrap()).to_str().unwrap(), false, &|_| String::new());
                let files = if r.files.is_empty() { Value::Null } else { json!(r.files) };
                json!([r.scanned, files, err.unwrap_or_default().replace(b.to_str().unwrap(), "{DIR}")])
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&base);
    check.done("save");
}
