//! testdata/parity/vault.json: reading a vault (ParseNote, Notes on a made-up tree with hidden folders, a big file, links
//! in and out and a named pipe, Resolve, SafePath), Search, Backlinks, Suggest, Lessons with Digest, Lint, and Go's
//! sort.Slice order for keys with ties (go/vault/vault_parity_test.go). The tree is made from the fixture's list.

mod common;
use common::Check;
use sarina::vault::{self, Note, Vault};
use serde_json::{json, Value};

fn note(n: &Note) -> Value {
    let front: Vec<Value> = n.front.iter().map(|(k, v)| json!([k, v])).collect();
    let list = |v: &[String]| if v.is_empty() { Value::Null } else { json!(v) };
    json!([n.path, n.title, list(&n.tags), list(&n.links), n.body, if front.is_empty() { Value::Null } else { Value::Array(front) }])
}

#[test]
fn vault_matches_go() {
    let mut d = Check::new();
    let base = std::env::temp_dir().join(format!("parity-vault-{}", std::process::id()));
    let (root, outside) = (base.join("root"), base.join("outside"));
    let mut notes: Vec<Note> = Vec::new();
    let mut v: Option<Vault> = None;
    for c in common::cases("vault") {
        let rows = c["out"].as_array().cloned().unwrap_or_default();
        match c["fn"].as_str().unwrap() {
            "parse" => {
                for r in &rows {
                    let n = vault::parse_note(r[0].as_str().unwrap(), r[1].as_str().unwrap(), 0);
                    d.eq(format!("parse {:.60}", r[1].to_string()), &json!([r[0], r[1], note(&n)]), r);
                }
            }
            "notes" => {
                for e in c["in"].as_array().unwrap() {
                    let p = root.join(e["Path"].as_str().unwrap());
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(&p, e["Text"].as_str().unwrap()).unwrap();
                    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(e["Mtime"].as_u64().unwrap());
                    std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
                }
                std::fs::create_dir_all(&outside).unwrap();
                std::fs::write(outside.join("key"), "PRIVATE").unwrap();
                std::os::unix::fs::symlink(outside.join("key"), root.join("Leak.md")).unwrap();
                std::os::unix::fs::symlink(root.join("Entities/Proton.md"), root.join("Alias.md")).unwrap();
                std::os::unix::fs::symlink(&outside, root.join("Out")).unwrap();
                let fifo = std::ffi::CString::new(root.join("Pipe.md").to_str().unwrap()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
                let vv = Vault::open(root.to_str().unwrap()).unwrap();
                notes = vv.notes();
                let got: Vec<Value> = notes
                    .iter()
                    .map(|n| {
                        let link = std::fs::symlink_metadata(root.join(&n.path)).unwrap().file_type().is_symlink();
                        json!([note(n), if link { -1 } else { (n.modified / 1_000_000_000) as i64 }])
                    })
                    .collect();
                d.eq("notes", &json!(got), &c["out"]);
                v = Some(vv);
            }
            "resolve" => {
                for r in &rows {
                    d.eq(format!("resolve {}", r[0]), &json!([r[0], vault::resolve(&notes, r[0].as_str().unwrap())]), r);
                }
            }
            "safepath" => {
                let vv = v.as_ref().unwrap();
                for r in &rows {
                    let name = String::from_utf8(common::bytes(&r[0])).unwrap();
                    let got = match vv.safe_path(&name) {
                        Ok((rel, abs)) => json!([r[0], rel, abs.strip_prefix(&vv.root).unwrap_or(&abs), ""]),
                        Err(e) => json!([r[0], "", "", e]),
                    };
                    d.eq(format!("safepath {}", r[0]), &got, r);
                }
            }
            "search" => {
                for r in &rows {
                    let hits: Vec<Value> = vault::search(&notes, r[0].as_str().unwrap(), r[1].as_u64().unwrap() as usize)
                        .iter()
                        .map(|h| json!([h.path, h.title, h.score, h.snippet, h.why]))
                        .collect();
                    let hits = if hits.is_empty() { Value::Null } else { json!(hits) };
                    d.eq(format!("search {} {}", r[0], r[1]), &json!([r[0], r[1], hits]), r);
                }
            }
            "backlinks" => {
                for r in &rows {
                    let b = vault::backlinks(&notes, r[0].as_str().unwrap());
                    d.eq(format!("backlinks {}", r[0]), &json!([r[0], if b.is_empty() { Value::Null } else { json!(b) }]), r);
                }
            }
            "suggest" => {
                for r in &rows {
                    d.eq(format!("suggest {}", r[0]), &json!([r[0], vault::suggest(&notes, r[0].as_str().unwrap())]), r);
                }
            }
            "lessons" => {
                let ls = v.as_ref().unwrap().lessons();
                let rows: Vec<Value> = ls.iter().map(|l| json!([l.note, l.rule, l.status])).collect();
                d.eq("lessons", &json!([rows, vault::digest(&ls)]), &c["out"]);
            }
            "lint" => {
                let got: Vec<Value> = vault::lint(&notes).iter().map(|p| json!([p.path, p.what])).collect();
                d.eq("lint", &json!(got), &c["out"]);
            }
            "lint2" => {
                let ns = [vault::parse_note("Home.md", "# Home\n", 0), vault::parse_note("A.md", "---\nx: y\n---\n[[B]]", 0)];
                let got: Vec<Value> = vault::lint(&ns).iter().map(|p| json!([p.path, p.what])).collect();
                d.eq("lint2", &json!(got), &c["out"]);
            }
            "sortslice" => {
                for r in &rows {
                    let keys: Vec<i64> = r[0].as_array().map(|a| a.iter().map(|k| k.as_i64().unwrap()).collect()).unwrap_or_default();
                    let mut idx: Vec<usize> = (0..keys.len()).collect();
                    sarina::gosort::slice(&mut idx, |&a, &b| keys[a] < keys[b]);
                    let got = json!([keys, idx]);
                    d.eq(format!("sortslice {}", keys.len()), &got, r);
                }
            }
            other => panic!("unknown case {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    d.done("vault");
}
