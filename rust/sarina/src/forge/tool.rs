//! Go: `forge/tool.go`: forge_list, forge_write and forge_run (offered only with SARINA_FORGE=on; no tool approves an
//! agent), and the owner's commands forge-list, forge-approve (at a terminal only) and forge-remove.

use super::{hash_of, Files, Manifest, Runner, Store};
use crate::goany::Any;
use crate::gojson;
use crate::gopath;
use crate::tools::defs::tool;
use crate::tools::files::{arg_str, fail};
use crate::tools::registry::{Output, Tool};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Go's `Enabled`.
pub fn enabled() -> bool {
    std::env::var("SARINA_FORGE").is_ok_and(|v| v == "on")
}

/// Go's `DefaultStore`: SARINA_FORGE_DIR and SARINA_FORGE_STATE, or the folders under the home.
pub fn default_store() -> Store {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dir = std::env::var("SARINA_FORGE_DIR").unwrap_or_default();
    let mut state = std::env::var("SARINA_FORGE_STATE").unwrap_or_default();
    if dir.is_empty() {
        dir = crate::gotext::lossy(&gopath::join(&[home.as_bytes(), b".local", b"share", b"sarina", b"forge", b"agents"]));
    }
    if state.is_empty() {
        state = crate::gotext::lossy(&gopath::join(&[home.as_bytes(), b".local", b"state", b"sarina", b"forge"]));
    }
    Store::new(&dir, &state)
}

/// Go's `profile`.
fn profile() -> String {
    match std::env::var("SARINA_FORGE_PROFILE") {
        Ok(p) if !p.is_empty() => p,
        _ => "agent-run".into(),
    }
}

/// Go's `limboRunner`.
struct LimboRunner;

impl Runner for LimboRunner {
    fn run_bundle(&self, profile: &str, command: &str, bundle: &[u8], timeout: i64) -> Result<(String, bool), String> {
        let getenv = |k: &str| std::env::var(k).unwrap_or_default();
        let run = crate::limbo::Client::default().validate_bundle(profile, command, bundle, timeout, &getenv).map_err(|e| e.0)?;
        Ok((run.as_text(), run.infrastructure_failure()))
    }
}

fn md(pairs: Vec<(&str, &str)>) -> BTreeMap<String, Any> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), Any::String(v.to_string()))).collect()
}

/// Go's `unescapeFlat`: a whole file written on one line with `\n` as two characters gets real line breaks (and tabs).
pub fn unescape_flat(content: &str) -> String {
    if content.contains('\n') || content.matches("\\n").count() < 2 {
        return content.to_string();
    }
    crate::insight::replace_once(content, &[("\\n", "\n"), ("\\t", "\t")])
}

/// Go's `Tools`: the three forge tools with SARINA_FORGE=on, sharing one store.
pub fn tools() -> Vec<Tool> {
    if !enabled() {
        return Vec::new();
    }
    let s = Arc::new(default_store());
    let (s1, s2, s3) = (s.clone(), s.clone(), s);
    vec![
        tool(
            "forge_list",
            Arc::new(move |_, _| {
                let names = match s1.list() {
                    Ok(n) => n,
                    Err(e) => return fail(e),
                };
                if names.is_empty() {
                    return Output { content: "No agents yet.".into(), metadata: md(vec![("action", "forge_list")]), error: None };
                }
                let mut b = String::new();
                for n in names {
                    match s1.load(&n) {
                        Err(e) => b += &format!("{n}: unusable ({e})\n"),
                        Ok((m, _)) => b += &format!("{n} [{}]: {} (approved by the owner: {})\n", m.language, m.purpose, s1.approved(&n)),
                    }
                }
                Output { content: b.trim_end_matches('\n').into(), metadata: md(vec![("action", "forge_list")]), error: None }
            }),
        ),
        tool(
            "forge_write",
            Arc::new(move |a, _| {
                let mut files: Files = BTreeMap::new();
                if let Some(Any::Array(list)) = a.get("files") {
                    for f in list {
                        let Any::Object(fm) = f else { continue };
                        let p = arg_str(fm, "path");
                        if !p.is_empty() {
                            files.insert(p, unescape_flat(&arg_str(fm, "content")).into_bytes());
                        }
                    }
                }
                let m = Manifest {
                    name: arg_str(a, "name"),
                    purpose: arg_str(a, "purpose"),
                    language: arg_str(a, "language"),
                    command: arg_str(a, "command"),
                    timeout_seconds: 0,
                };
                match s2.create(&m, &files) {
                    Err(e) => fail(e),
                    Ok(h) => Output {
                        content: format!(
                            "Agent {} written ({} files, hash {}). It has not run anywhere. Run it with forge_run; the owner approves it with forge-approve.",
                            m.name,
                            files.len(),
                            &h[..12]
                        ),
                        metadata: md(vec![("action", "forge_write"), ("agent", &m.name), ("hash", &h)]),
                        error: None,
                    },
                }
            }),
        ),
        tool(
            "forge_run",
            Arc::new(move |a, _| {
                let name = arg_str(a, "name");
                match s3.run_with_input(&LimboRunner, &name, &profile(), &arg_str(a, "input")) {
                    Err(e) => fail(e),
                    Ok(text) => Output { content: text, metadata: md(vec![("action", "forge_run"), ("agent", &name)]), error: None },
                }
            }),
        ),
    ]
}

/// `sarina forge-list [--json]`.
pub fn list_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let s = default_store();
    let listed = s.list();
    if args.len() == 1 && args[0] == "--json" {
        let mut rows = Vec::new();
        for n in listed.as_deref().unwrap_or_default() {
            let js = |v: &str| {
                let mut o = String::new();
                gojson::write_str(&mut o, v, true);
                o
            };
            let fields: Vec<(&str, String)> = match s.load(n) {
                Err(e) => vec![
                    ("name", js(n)),
                    ("language", js("")),
                    ("purpose", js("")),
                    ("command", js("")),
                    ("files", "0".into()),
                    ("hash", js("")),
                    ("approved", "false".into()),
                    ("problem", js(&e)),
                ],
                Ok((m, files)) => vec![
                    ("name", js(n)),
                    ("language", js(&m.language)),
                    ("purpose", js(&m.purpose)),
                    ("command", js(&m.command)),
                    ("files", files.len().to_string()),
                    ("hash", js(&hash_of(&m, &files)[..12])),
                    ("approved", s.approved(n).to_string()),
                ],
            };
            let mut row = String::from("{");
            row += &fields.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect::<Vec<_>>().join(",");
            row.push('}');
            rows.push(row);
        }
        env.println(&format!("[{}]", rows.join(",")));
        return 0;
    }
    let names = match listed {
        Ok(n) => n,
        Err(e) => {
            env.println(&e);
            return 1;
        }
    };
    if names.is_empty() {
        env.println("No agents yet.");
    }
    for n in names {
        match s.load(&n) {
            Err(e) => env.println(&format!("{n}: unusable ({e})")),
            Ok((m, _)) => env.println(&format!("{n} [{}] approved={}: {}", m.language, s.approved(&n), m.purpose)),
        }
    }
    0
}

/// Whether standard input is a character device (Go's `os.ModeCharDevice` on `os.Stdin`).
fn stdin_is_char_device() -> bool {
    // SAFETY: fstat on descriptor 0 into a zeroed stat buffer of the right type.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(0, &mut st) }) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFCHR
}

/// `sarina forge-approve NAME`: everything the agent is, then the owner types yes at a terminal.
pub fn approve_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    if args.len() != 1 {
        env.eprintln("usage: sarina forge-approve NAME");
        return 2;
    }
    if !stdin_is_char_device() {
        env.eprintln("forge-approve needs you at a terminal: it cannot be given its answer by a program.");
        return 2;
    }
    let s = default_store();
    let (m, files) = match s.load(&args[0]) {
        Ok(x) => x,
        Err(e) => {
            env.eprintln(&e);
            return 1;
        }
    };
    let hash = hash_of(&m, &files);
    let mut out = format!("Agent {} [{}]\nPurpose: {}\nStarts with: {}\n\n", m.name, m.language, m.purpose, m.command).into_bytes();
    for (k, body) in &files {
        out.extend_from_slice(format!("===== {k} =====\n").as_bytes());
        out.extend_from_slice(body);
        out.push(b'\n');
    }
    out.extend_from_slice(format!("\nVersion {}\nApprove exactly this version? Type yes: ", &hash[..12]).as_bytes());
    env.stdout.extend_from_slice(&out);
    env.flush();
    let mut line = Vec::new();
    {
        use std::io::Read;
        let mut byte = [0u8; 1];
        let mut stdin = std::io::stdin().lock();
        while let Ok(1) = stdin.read(&mut byte) {
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
    }
    if crate::gotext::trim_space(&crate::gotext::lossy(&line)) != "yes" {
        env.println("Not approved.");
        return 1;
    }
    if let Err(e) = s.approve(&m.name, &hash) {
        env.eprintln(&e);
        return 1;
    }
    env.println("Approved. Any later change to the agent voids this.");
    0
}

/// `sarina forge-remove NAME`.
pub fn remove_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    if args.len() != 1 {
        env.eprintln("usage: sarina forge-remove NAME");
        return 2;
    }
    if let Err(e) = default_store().remove(&args[0]) {
        env.eprintln(&e);
        return 1;
    }
    env.println(&format!("Removed {}.", args[0]));
    0
}
