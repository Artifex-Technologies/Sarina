//! Go: `vault/tools.go` and `vault/command.go`: vault_search, vault_read and vault_write (offered when SARINA_VAULT is
//! set), and the commands vault-init, vault-lint, note and lessons. Recall by meaning (SARINA_SEMANTIC=on) is not ported
//! yet: with it on, a chat goes to the Go build (`cli::chat::needs_go`) and `sarina vault-search` stays there.

use super::write::{local_now, FOLDERS, MAX_NOTE_CHARS};
use super::{backlinks, digest, lint, resolve, search, suggest, Note, Vault};
use crate::goany::Any;
use crate::goflag::{FlagSet, HELP};
use crate::gotext::{self, quote};
use crate::tools::defs::tool;
use crate::tools::files::{arg_str, fail};
use crate::tools::registry::{Context, Output, Tool};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

/// Go's `EnvVar`.
pub const ENV_VAR: &str = "SARINA_VAULT";

/// Go's `vaultFor`: the vault SARINA_VAULT names (the session's own environment last).
fn vault_for(tc: &Context) -> Result<Vault, String> {
    let mut dir = std::env::var(ENV_VAR).unwrap_or_default();
    if let Some(environ) = &tc.environ {
        for kv in environ() {
            if let Some(v) = kv.strip_prefix(format!("{ENV_VAR}=").as_bytes()) {
                dir = gotext::lossy(v);
            }
        }
    }
    if dir.is_empty() {
        return Err(format!("no vault is set up ({ENV_VAR})"));
    }
    Vault::open(&dir)
}

/// Go's `guard.Memory()` as the vault's gate: the local checks, then Heimdall.
fn gate(text: &str) -> Option<String> {
    let getenv = |k: &str| std::env::var(k).unwrap_or_default();
    let v = crate::guard::Gate::memory().scan(text.as_bytes(), &getenv);
    v.blocked.then_some(v.reason)
}

fn md(pairs: Vec<(&str, Any)>) -> BTreeMap<String, Any> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn ok(content: String, metadata: BTreeMap<String, Any>) -> Output {
    Output { content, metadata, error: None }
}

/// Go's `Tools`: the three vault tools when SARINA_VAULT is set (read when the registry is built).
pub fn tools() -> Vec<Tool> {
    if std::env::var(ENV_VAR).unwrap_or_default().is_empty() {
        return Vec::new();
    }
    vec![tool("vault_search", Arc::new(search_tool)), tool("vault_read", Arc::new(read_tool)), tool("vault_write", Arc::new(write_tool))]
}

fn search_tool(a: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let v = match vault_for(tc) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let notes = v.notes();
    let n = match a.get("max_results") {
        Some(Any::Number(f)) if *f >= 1.0 => crate::config::go_int(*f).max(0) as usize,
        _ => 8,
    };
    let hits = search(&notes, &arg_str(a, "query"), n);
    if hits.is_empty() {
        return ok("No notes matched.".into(), md(vec![("action", Any::String("vault_search".into())), ("hits", Any::Int(0))]));
    }
    let mut b = String::new();
    for h in &hits {
        b += &format!("{}  ({})  [{}]\n  {}\n", h.path, h.title, h.why.join(", "), h.snippet);
    }
    b += "(A hit shows only the start of a note: read it with vault_read before you say what it contains.)";
    ok(b, md(vec![("action", Any::String("vault_search".into())), ("hits", Any::Int(hits.len() as i64))]))
}

fn read_tool(a: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let v = match vault_for(tc) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let notes = v.notes();
    let want = arg_str(a, "path");
    let p = resolve(&notes, &want);
    if p.is_empty() {
        let near = near_names(&notes, &want);
        if !near.is_empty() {
            return fail(format!(
                "no note called {}; notes with a name like it: {}. Read the one you meant with vault_read",
                quote(&want),
                near.join(", ")
            ));
        }
        return fail(format!("no note called {} (use vault_search)", quote(&want)));
    }
    let note = notes.iter().rfind(|n| n.path == p).cloned().unwrap_or_default();
    let mut body = note.body.clone();
    if body.chars().count() > 6000 {
        body = body.chars().take(6000).collect::<String>() + "\n[cut: the note is longer]";
    }
    let mut b = format!("{p}\n\n{body}\n");
    let out: Vec<String> = note
        .links
        .iter()
        .map(|l| {
            let t = resolve(&notes, l);
            if t.is_empty() {
                format!("{l} (no such note yet)")
            } else {
                t
            }
        })
        .collect();
    if !out.is_empty() {
        b += &format!("\nLinks to: {}\n", out.join(", "));
    }
    let back = backlinks(&notes, &p);
    if !back.is_empty() {
        b += &format!("Linked from: {}\n", back.join(", "));
    }
    ok(b.trim_end_matches('\n').to_string(), md(vec![("action", Any::String("vault_read".into())), ("path", Any::String(p))]))
}

fn write_tool(a: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let v = match vault_for(tc) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let mode = arg_str(a, "mode");
    let (content, relinked) = relink(&v, &arg_str(a, "content"));
    if let Some(reason) = gate(&content) {
        return fail(format!("not saved: {reason}"));
    }
    let path = arg_str(a, "path");
    let p = path.trim_matches('/');
    if (mode == "create" || mode.is_empty()) && !p.contains('/') {
        let names: Vec<String> = FOLDERS.iter().map(|(n, _)| format!("{n}/")).collect();
        return fail(format!(
            "a new note goes in a folder: put the folder in the path (for example Lessons/{}). The folders: {}",
            p.strip_suffix(".md").unwrap_or(p),
            names.join(", ")
        ));
    }
    let res = match v.write(Some(&gate), &path, &content, &mode, &local_now().0) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    v.commit(&format!("note {}", res.path));
    let mut msg = if res.created { format!("Saved {}.", res.path) } else { format!("Updated {}.", res.path) };
    if !relinked.is_empty() {
        msg += &format!(" Linked {} (the notes that exist).", relinked.join(", "));
    }
    if !res.broken_links.is_empty() {
        let notes = v.notes();
        let parts: Vec<String> = res
            .broken_links
            .iter()
            .map(|l| match suggest(&notes, l) {
                s if s.is_empty() => format!("[[{l}]]"),
                s => format!("[[{l}]] (did you mean [[{s}]]? fix it with mode replace)"),
            })
            .collect();
        msg += &format!(" Links to notes that do not exist yet: {}.", parts.join(", "));
    }
    ok(msg, md(vec![("action", Any::String("vault_write".into())), ("path", Any::String(res.path)), ("created", Any::Bool(res.created))]))
}

static WIKI_LINK_RE: LazyLock<regex::Regex> = LazyLock::new(|| crate::goregexp::compile(r"\[\[([^\[\]]+)\]\]").expect("wikiLinkRE"));

/// Go's `relink`: each [[link]] to no note that a note's name nearly matches is pointed at that note (its name only).
fn relink(v: &Vault, content: &str) -> (String, Vec<String>) {
    let notes = v.notes();
    if notes.is_empty() {
        return (content.to_string(), Vec::new());
    }
    let mut changed = Vec::new();
    let out = WIKI_LINK_RE.replace_all(content, |c: &regex::Captures| {
        let m = &c[0];
        let inner = &m[2..m.len() - 2];
        let (target, alias) = match inner.find(['|', '#']) {
            Some(i) => (&inner[..i], &inner[i..]),
            None => (inner, ""),
        };
        if !resolve(&notes, target).is_empty() {
            return m.to_string();
        }
        let sug = suggest(&notes, target);
        if sug.is_empty() {
            return m.to_string();
        }
        let name = &sug[sug.rfind('/').map_or(0, |i| i + 1)..];
        changed.push(format!("[[{target}]] as [[{name}]]"));
        format!("[[{name}{alias}]]")
    });
    (out.into_owned(), changed)
}

/// Go's `path.Base`.
fn path_base(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    if p.is_empty() {
        return ".";
    }
    if t.is_empty() {
        return "/";
    }
    &t[t.rfind('/').map_or(0, |i| i + 1)..]
}

/// Go's `nearNames`: up to five notes whose file name shares four first letters with the name asked for, or holds it.
fn near_names(notes: &[Note], target: &str) -> Vec<String> {
    let b = path_base(gotext::trim_space(target));
    let t = gotext::to_lower(b.strip_suffix(".md").unwrap_or(b));
    if t.chars().count() < 3 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for n in notes {
        let nb = path_base(&n.path);
        let b = gotext::to_lower(nb.strip_suffix(".md").unwrap_or(nb));
        let common = b.bytes().zip(t.bytes()).take_while(|(x, y)| x == y).count();
        if common >= 4 || b.contains(&t) || t.contains(&b) {
            out.push(n.path.clone());
        }
        if out.len() == 5 {
            break;
        }
    }
    out
}

// ---- the commands ------------------------------------------------------------------------------------------------------

/// Go's `open`: the --path flag (default $SARINA_VAULT) and the vault; Err is the exit status.
fn open(env: &mut crate::cli::Env, name: &'static str, args: &[String]) -> Result<(Vault, Vec<String>), i32> {
    let mut fs = FlagSet::new(name);
    let usage: &'static str = "the vault folder (default $SARINA_VAULT)";
    fs.string("path", &env.getenv(ENV_VAR), usage);
    let rest = match fs.parse(args, &mut env.stderr) {
        Ok(r) => r,
        Err(e) => {
            env.flush();
            return Err(if e == HELP { 0 } else { 2 });
        }
    };
    let path = fs.str("path");
    if path.is_empty() {
        env.eprintln(&format!("sarina {name}: no vault: give --path or set {ENV_VAR}"));
        return Err(2);
    }
    match Vault::open(&path) {
        Ok(v) => Ok((v, rest)),
        Err(e) => {
            env.eprintln(&format!("sarina {name}: {e}"));
            Err(1)
        }
    }
}

/// `sarina vault-init [--path DIR]`.
pub fn init_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let v = match open(env, "vault-init", args) {
        Ok((v, _)) => v,
        Err(rc) => return rc,
    };
    match v.init(&local_now().0) {
        Err(e) => {
            env.eprintln(&format!("sarina vault-init: {e}"));
            1
        }
        Ok(made) if made.is_empty() => {
            env.println("the vault is already set up; nothing changed");
            0
        }
        Ok(made) => {
            env.println(&format!("made: {}", made.join(", ")));
            0
        }
    }
}

/// `sarina vault-lint [--path DIR]`.
pub fn lint_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let v = match open(env, "vault-lint", args) {
        Ok((v, _)) => v,
        Err(rc) => return rc,
    };
    let notes = v.notes();
    let problems = lint(&notes);
    for p in &problems {
        env.println(&format!("{}: {}", p.path, p.what));
    }
    env.println(&format!("{} notes, {} things to look at", notes.len(), problems.len()));
    i32::from(!problems.is_empty())
}

/// `sarina note [--path DIR] TEXT...` (or the text on standard input): a line with the time in today's Inbox note.
pub fn note_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let (v, rest) = match open(env, "note", args) {
        Ok(x) => x,
        Err(rc) => return rc,
    };
    let mut text = gotext::trim_space(&rest.join(" ")).to_string();
    if text.is_empty() {
        use std::io::Read;
        let mut b = Vec::new();
        let _ = std::io::stdin().lock().take(MAX_NOTE_CHARS as u64).read_to_end(&mut b);
        text = gotext::trim_space(&gotext::lossy(&b)).to_string();
    }
    if text.is_empty() {
        env.eprintln("sarina note: nothing to note (give the text, or pipe it in)");
        return 2;
    }
    let (today, hm) = local_now();
    let name = format!("Inbox/Notes {today}");
    let mut line = format!("- {hm} {}", text.replace('\n', "\n  "));
    let mut mode = "append";
    if let Ok((_, abs)) = v.safe_path(&name) {
        if std::fs::metadata(crate::gopath::os_path(abs.as_bytes())).is_err() {
            mode = "create";
            line = format!("# Notes {today}\n\n{line}");
        }
    }
    match v.write(None, &name, &line, mode, &today) {
        Ok(res) => {
            env.println(&format!("noted in {}", res.path));
            0
        }
        Err(e) => {
            env.eprintln(&format!("sarina note: {e}"));
            1
        }
    }
}

/// `sarina lessons [--path DIR] list | digest`.
pub fn lessons_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let (v, rest) = match open(env, "lessons", args) {
        Ok(x) => x,
        Err(rc) => return rc,
    };
    let ls = v.lessons();
    match rest.first().map_or("list", String::as_str) {
        "digest" => {
            env.stdout.extend_from_slice(digest(&ls).as_bytes());
            env.flush();
        }
        "list" => {
            for l in &ls {
                let name = l.note.strip_prefix("Lessons/").unwrap_or(&l.note);
                env.stdout.extend_from_slice(
                    format!("{:<9} {}\n          {}\n", l.status, name.strip_suffix(".md").unwrap_or(name), l.rule).as_bytes(),
                );
            }
            env.flush();
        }
        _ => {
            env.eprintln("sarina lessons: list or digest");
            return 2;
        }
    }
    0
}
