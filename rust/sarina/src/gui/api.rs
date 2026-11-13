//! Go: `gui/api.go`: the console's read-mostly API: sessions and their file history, the slash commands, the plan and
//! the to-do list, curated memory and its review queue, the instruction files (CLAUDE.md and friends: only the set the
//! console manages, never through a link or out of its folder), compute tiers and the backend probe.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use super::state::{bad, wrong_type, ApiErr, State};
use super::{detail, fail, json_any, json_str, obj, query_get, raw_obj, read_json, s, write_json, write_raw, Server};
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gohttpd::{Exchange, Request, Response};
use crate::gopath::{self, Kind};
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext;
use crate::runkit;
use crate::session::message::Message;

/// Go's `idRe`: `^[A-Za-z0-9_-]{1,64}$`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn join(a: &str, b: &str) -> String {
    gotext::lossy(&gopath::join(&[a.as_bytes(), b.as_bytes()]))
}

fn dir(p: &str) -> String {
    gotext::lossy(&gopath::dir(p.as_bytes()))
}

fn base(p: &str) -> String {
    gotext::lossy(&gopath::base(p.as_bytes()))
}

fn clean(p: &str) -> String {
    gotext::lossy(&gopath::clean(p.as_bytes()))
}

/// Go's `preview`.
fn preview(msgs: &[Message]) -> String {
    for m in msgs {
        if m.role == "user" && !m.is_reminder() && !gotext::trim_space(&m.content).is_empty() {
            return super::run::clip(&gotext::fields(&m.content).join(" "), 120);
        }
    }
    String::new()
}

/// Go's `userText`.
fn user_text(msgs: &[Message]) -> String {
    let mut b = String::new();
    for m in msgs {
        if m.role == "user" && !m.is_reminder() {
            b.push_str(&gotext::to_lower(&m.content));
            b.push('\n');
        }
    }
    b
}

/// The session files of a folder (Go's ReadDir filtered as the handlers do): name and path, in name order.
fn session_files(dir: &str) -> Vec<(String, String)> {
    let Ok(entries) = gopath::read_dir(dir.as_bytes()) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries {
        let name = gotext::lossy(&e.name);
        if matches!(e.kind, Kind::Dir) || !name.ends_with(".json") || !name.strip_suffix(".json").is_some_and(valid_id) {
            continue;
        }
        out.push((name.clone(), join(dir, &name)));
    }
    out
}

/// Go's `os.ReadFile` of a regular file, at most `max` bytes of it, opened without blocking (a named pipe is refused at
/// once): `readHead`.
fn read_head(path: &str, max: u64) -> Result<Vec<u8>, String> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(gopath::os_path(path.as_bytes()))
        .map_err(|e| e.to_string())?;
    let st = f.metadata().map_err(|e| e.to_string())?;
    if !st.file_type().is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    let mut b = Vec::new();
    f.take(max).read_to_end(&mut b).map_err(|e| e.to_string())?;
    Ok(b)
}

static MAP_ANY: Ty = Ty::Map(&Ty::Any);
static TASK_LIST: Ty = Ty::Slice(&MAP_ANY);
static TASKS: Ty = Ty::Struct(r#"struct { Tasks []map[string]interface {} "json:\"tasks\"" }"#, "", &[Field("tasks", &TASK_LIST)]);
static REVIEW: Ty = Ty::Struct(r#"struct { Decision string "json:\"decision\"" }"#, "", &[Field("decision", &Ty::Str)]);
static STR: Ty = Ty::Ptr(&Ty::Str);
static CONTENT: Ty = Ty::Struct(r#"struct { Content *string "json:\"content\"" }"#, "", &[Field("content", &STR)]);

fn val_any(v: &Val) -> Any {
    match v {
        Val::Any(a) => a.clone(),
        Val::Map(Some(m)) => Any::Object(m.iter().map(|(k, x)| (k.clone(), val_any(x))).collect()),
        _ => Any::Null,
    }
}

/// One file the console reads or writes (Go's `instrFile`).
#[derive(Clone)]
struct InstrFile {
    path: String,
    name: String,
    parent: String,
    size: i64,
    modified: i64,
    writable: bool,
    exists: bool,
}

impl InstrFile {
    fn new(path: &str, writable: bool) -> InstrFile {
        InstrFile { path: path.into(), name: base(path), parent: dir(path), size: 0, modified: 0, writable, exists: false }
    }

    fn json(&self) -> String {
        format!(
            "{{\"path\":{},\"name\":{},\"parent\":{},\"size\":{},\"modified_at\":{},\"writable\":{},\"exists\":{}}}",
            json_str(&self.path),
            json_str(&self.name),
            json_str(&self.parent),
            self.size,
            self.modified,
            self.writable,
            self.exists
        )
    }
}

/// Go's `guidanceNames`.
fn guidance_names(d: &str) -> Vec<String> {
    let mut files = vec![join(d, "CLAUDE.md"), join(&join(d, ".claude"), "CLAUDE.md"), join(d, "CLAUDE.local.md")];
    let rules = join(&join(d, ".claude"), "rules");
    if let Ok(es) = gopath::read_dir(rules.as_bytes()) {
        let mut names: Vec<String> =
            es.iter().filter(|e| !matches!(e.kind, Kind::Dir)).map(|e| gotext::lossy(&e.name)).filter(|n| n.ends_with(".md")).collect();
        names.sort();
        files.extend(names.iter().map(|n| join(&rules, n)));
    }
    files
}

/// Go's `os.Remove`: a file, or else an empty folder.
fn go_remove(p: &str) -> bool {
    let p = gopath::os_path(p.as_bytes());
    std::fs::remove_file(p).is_ok() || std::fs::remove_dir(p).is_ok()
}

fn stat_regular(p: &str) -> Option<std::fs::Metadata> {
    std::fs::metadata(gopath::os_path(p.as_bytes())).ok().filter(|m| m.file_type().is_file())
}

/// Go's `ruleNameRe`: `^[A-Za-z0-9][A-Za-z0-9._-]*\.md$`.
fn rule_name(n: &str) -> bool {
    let b = n.as_bytes();
    n.len() > 3
        && n.ends_with(".md")
        && b[0].is_ascii_alphanumeric()
        && b[..b.len() - 3].iter().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(c))
}

impl Server {
    pub(super) fn list_sessions(&self, req: &Request, w: &mut Response) {
        let st = self.settings();
        let scrub = self.scrubber();
        let q = gotext::to_lower(gotext::trim_space(&query_get(&req.url.raw_query, "q")));
        let mut rows: Vec<(Any, i128)> = Vec::new();
        for (_, path) in session_files(&st.session_dir) {
            let Ok(info) = std::fs::symlink_metadata(gopath::os_path(path.as_bytes())) else { continue };
            let Ok(sess) = crate::session::readfile::load(&path) else { continue };
            if !q.is_empty() && !user_text(&sess.messages).contains(&q) && !gotext::to_lower(&sess.id).contains(&q) {
                continue;
            }
            let mtime = std::os::unix::fs::MetadataExt::mtime(&info);
            let nanos = i128::from(mtime) * 1_000_000_000 + i128::from(std::os::unix::fs::MetadataExt::mtime_nsec(&info));
            let row = obj(vec![
                ("session_id", s(&sess.id)),
                ("turns", Any::Int(sess.turns)),
                ("tool_calls", Any::Int(sess.tool_calls)),
                ("preview", s(&scrub(&preview(&sess.messages)))),
                ("modified_at", Any::Int(mtime)),
                ("model", s(&sess.model.model)),
                ("total_cost_usd", Any::Number(sess.total_cost_usd)),
            ]);
            rows.push((row, nanos));
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.1));
        write_json(w, 200, &Any::Array(rows.into_iter().take(500).map(|r| r.0).collect()));
    }

    pub(super) fn get_session(&self, w: &mut Response, id: &str) {
        if !valid_id(id) {
            return detail(w, 404, "session not found");
        }
        let st = self.settings();
        let Ok(sess) = crate::session::readfile::load_by_id(&st.session_dir, id) else { return detail(w, 404, "session not found") };
        let scrub = self.scrubber();
        let sc = |x: &str| scrub(x);
        let plan = sess.plan();
        let fh: Vec<Any> = sess.file_history.iter().map(|e| runkit::scrub_any(&Any::Object(e.clone()), &sc)).collect();
        let mut m: BTreeMap<&str, String> = BTreeMap::new();
        m.insert("session_id", json_str(&sess.id));
        m.insert("turns", sess.turns.to_string());
        m.insert("tool_calls", sess.tool_calls.to_string());
        m.insert("messages", json_any(&runkit::transcript(&sess.messages, Some(&sc))));
        m.insert("usage", sess.usage.to_json());
        m.insert("total_cost_usd", json_any(&Any::Number(sess.total_cost_usd)));
        m.insert("model", json_str(&sess.model.model));
        m.insert("file_history", json_any(&Any::Array(fh)));
        m.insert("plan", json_any(&obj(vec![("mode", s(&plan.mode)), ("pending", s(&sc(&plan.pending))), ("note", s(&sc(&plan.note)))])));
        write_raw(w, 200, raw_obj(&m));
    }

    pub(super) fn delete_session(&self, w: &mut Response, id: &str) {
        if !valid_id(id) {
            return detail(w, 404, "session not found");
        }
        let busy = self.lock().run.as_ref().is_some_and(|r| r.running() && r.current_session() == id);
        if busy {
            return detail(w, 409, "that session is running; stop the run first");
        }
        let st = self.settings();
        if !go_remove(&crate::session::path(&st.session_dir, id)) {
            return detail(w, 404, "session not found");
        }
        go_remove(&join(&st.session_dir, &format!("{id}.notes.md")));
        {
            let mut g = self.lock();
            if g.st.active_session_id == id {
                g.st.active_session_id.clear();
            }
        }
        write_json(w, 200, &obj(vec![("deleted", s(id))]));
    }

    pub(super) fn file_history(&self, req: &Request, w: &mut Response) {
        let mut limit = 200usize;
        if let Some(n) = gotext::atoi(query_get(&req.url.raw_query, "limit").as_bytes()) {
            if n > 0 && n <= 5000 {
                limit = n as usize;
            }
        }
        let st = self.settings();
        let scrub = self.scrubber();
        let sc = |x: &str| scrub(x);
        let mut all: Vec<BTreeMap<String, Any>> = Vec::new();
        for (_, path) in session_files(&st.session_dir) {
            let Ok(sess) = crate::session::readfile::load(&path) else { continue };
            for h in &sess.file_history {
                if let Ok(mut c) = runkit::scrub_any(&Any::Object(h.clone()), &sc).into_object() {
                    c.insert("session_id".into(), s(&sess.id));
                    all.push(c);
                }
            }
        }
        let ts = |m: &BTreeMap<String, Any>| match m.get("timestamp") {
            Some(Any::String(t)) => t.clone(),
            _ => String::new(),
        };
        all.sort_by_key(|m| std::cmp::Reverse(ts(m)));
        let total = all.len();
        all.truncate(limit);
        let returned = all.len();
        write_json(
            w,
            200,
            &obj(vec![
                ("total", Any::Int(total as i64)),
                ("returned", Any::Int(returned as i64)),
                ("entries", Any::Array(all.into_iter().map(Any::Object).collect())),
            ]),
        );
    }

    pub(super) fn slash_commands(&self, w: &mut Response) {
        let mut list: Vec<(&str, &str)> = crate::cli::chat::SLASH.to_vec();
        list.sort();
        let out =
            list.iter().map(|(n, d)| obj(vec![("names", Any::Array(vec![s(n)])), ("primary", s(n)), ("description", s(d))])).collect();
        write_json(w, 200, &Any::Array(out));
    }

    pub(super) fn get_plan(&self, w: &mut Response) {
        let st = self.settings();
        let scrub = self.scrubber();
        let mut out = BTreeMap::new();
        out.insert("session_id".to_owned(), Any::Null);
        out.insert("mode".to_owned(), s(crate::session::PLAN_NORMAL));
        out.insert("pending".to_owned(), s(""));
        out.insert("note".to_owned(), s(""));
        if !st.active_session_id.is_empty() && valid_id(&st.active_session_id) {
            if let Ok(sess) = crate::session::readfile::load_by_id(&st.session_dir, &st.active_session_id) {
                let p = sess.plan();
                out.insert("session_id".into(), s(&sess.id));
                out.insert("mode".into(), s(&p.mode));
                out.insert("pending".into(), s(&scrub(&p.pending)));
                out.insert("note".into(), s(&scrub(&p.note)));
            }
        }
        // a plan waiting for a decision right now is the live one
        for ap in self.lock().approvals.values() {
            if ap.kind == "plan" {
                out.insert("pending".into(), s(&scrub(&ap.plan)));
                out.insert("mode".into(), s(crate::session::PLAN_ACTIVE));
                out.insert("approval_id".into(), s(&ap.id));
            }
        }
        write_json(w, 200, &Any::Object(out));
    }

    pub(super) fn get_tasks(&self, w: &mut Response) {
        let st = self.settings();
        let scrub = self.scrubber();
        let sc = |x: &str| scrub(x);
        let path = join(&join(&st.cwd, ".port_sessions"), "task_runtime.json");
        let empty = obj(vec![("tasks", Any::Array(Vec::new())), ("counts", Any::Object(BTreeMap::new()))]);
        // a regular file of bounded size: never a pipe planted under the name
        let Ok(raw) = crate::session::readfile::read_file(path.as_bytes()) else { return write_json(w, 200, &empty) };
        let (v, err) = gostruct::unmarshal(&TASKS, &raw);
        if err.is_some() {
            return write_json(w, 200, &empty);
        }
        let mut counts: BTreeMap<String, Any> = BTreeMap::new();
        let mut tasks = Vec::new();
        if let Some(Val::Slice(Some(items))) = gostruct::field(&TASKS, &v, "tasks") {
            for t in items {
                let t = val_any(t);
                let status = match &t {
                    Any::Object(m) => match m.get("status") {
                        Some(Any::String(x)) => x.clone(),
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                let c = counts.entry(status).or_insert(Any::Int(0));
                if let Any::Int(n) = c {
                    *n += 1;
                }
                tasks.push(runkit::scrub_any(&t, &sc));
            }
        }
        write_json(w, 200, &obj(vec![("tasks", Any::Array(tasks)), ("counts", Any::Object(counts))]));
    }

    pub(super) fn get_memory(&self, w: &mut Response) {
        let st = self.settings();
        let scrub = self.scrubber();
        let rt = crate::memory::Runtime::from_workspace(&st.cwd, &self.cfg.home);
        let mut scopes = BTreeMap::new();
        for sc in crate::memory::SCOPES {
            let (es, err) = match rt.entries(sc) {
                Ok(es) => (es, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            let mut entry = BTreeMap::new();
            entry.insert("entries".to_owned(), Any::Array(es.iter().map(|e| s(&scrub(e))).collect()));
            if let Some(e) = err {
                entry.insert("error".to_owned(), s(&scrub(&e)));
            }
            scopes.insert(sc.to_string(), Any::Object(entry));
        }
        let opt = |p: &Option<String>| p.as_ref().map_or(Any::Null, |t| s(&scrub(t)));
        let pending: Vec<Any> = crate::memory::review::Queue::from_workspace(&st.cwd)
            .pending()
            .iter()
            .map(|p| {
                obj(vec![
                    ("entry_id", s(&p.entry_id)),
                    ("session_id", s(&p.session_id)),
                    ("scope", s(&p.scope)),
                    ("action", s(&p.action)),
                    ("old_text", opt(&p.old_text)),
                    ("content", opt(&p.content)),
                    ("rationale", s(&scrub(&p.rationale))),
                    ("created_at", s(&p.created_at)),
                ])
            })
            .collect();
        let limits = obj(vec![
            ("entry_chars", Any::Int(crate::memory::MAX_ENTRY_CHARS as i64)),
            ("scope_chars", Any::Int(crate::memory::MAX_SCOPE_CHARS as i64)),
        ]);
        write_json(w, 200, &obj(vec![("scopes", Any::Object(scopes)), ("pending_review", Any::Array(pending)), ("limits", limits)]));
    }

    /// Go's `reviewMemory`: approving applies the proposal through the same scanned entry points a tool call uses.
    pub(super) fn review_memory(&self, ex: &mut Exchange, w: &mut Response, id: &str) {
        let Some(body) = read_json(ex, w) else { return };
        let Some(v) = super::decode(&body, &REVIEW, w) else { return };
        let st = self.settings();
        let q = crate::memory::review::Queue::from_workspace(&st.cwd);
        match gostruct::str_field(&REVIEW, &v, "decision") {
            "approve" => match q.approve(id, &crate::memory::Runtime::from_workspace(&st.cwd, &self.cfg.home)) {
                Err(e) => detail(w, 409, &self.scrubber()(&e)),
                Ok(m) => write_json(
                    w,
                    200,
                    &obj(vec![("entry_id", s(id)), ("status", s("approved")), ("scope", s(&m.scope)), ("action", s(&m.action))]),
                ),
            },
            "reject" => match q.reject(id) {
                Err(e) => detail(w, 409, &self.scrubber()(&e)),
                Ok(()) => write_json(w, 200, &obj(vec![("entry_id", s(id)), ("status", s("rejected"))])),
            },
            _ => detail(w, 400, "decision must be approve or reject"),
        }
    }

    /// Go's `instrSet`: the instruction files of the workspace, the extra folders and the global folder (writable),
    /// and those of the workspace's ancestors that exist (read only); with the order they were added in.
    fn instr_set(&self, st: &State) -> (BTreeMap<String, InstrFile>, Vec<String>) {
        let mut set = BTreeMap::new();
        let mut order = Vec::new();
        let mut add = |p: &str, writable: bool| {
            let p = clean(p);
            if set.contains_key(&p) {
                return;
            }
            let mut f = InstrFile::new(&p, writable);
            if let Some(m) = stat_regular(&p) {
                f.exists = true;
                f.size = m.len() as i64;
                f.modified = std::os::unix::fs::MetadataExt::mtime(&m);
            }
            set.insert(p.clone(), f);
            order.push(p);
        };
        for d in std::iter::once(&st.cwd).chain(st.extra_dirs.iter()) {
            for p in guidance_names(d) {
                add(&p, true);
            }
        }
        let global = join(&self.home_dir(), ".claude");
        add(&join(&global, "CLAUDE.md"), true);
        let mut d = dir(&st.cwd);
        while d != dir(&d) {
            for p in guidance_names(&d) {
                if std::fs::metadata(gopath::os_path(p.as_bytes())).is_ok() {
                    add(&p, false);
                }
            }
            d = dir(&d);
        }
        (set, order)
    }

    pub(super) fn list_instructions(&self, w: &mut Response) {
        let st = self.settings();
        let (set, order) = self.instr_set(&st);
        let global = join(&self.home_dir(), ".claude");
        let mut files = Vec::new();
        for p in &order {
            let f = &set[p];
            if f.exists || (f.writable && f.name == "CLAUDE.md" && (f.parent == st.cwd || f.parent == global)) {
                files.push(f.json());
            }
        }
        let extra =
            if st.extra_dirs.is_empty() { "null".to_owned() } else { json_any(&Any::Array(st.extra_dirs.iter().map(|d| s(d)).collect())) };
        let mut m: BTreeMap<&str, String> = BTreeMap::new();
        m.insert("cwd", json_str(&st.cwd));
        m.insert("additional_working_directories", extra);
        m.insert("global_dir", json_str(&global));
        m.insert("files", format!("[{}]", files.join(",")));
        write_raw(w, 200, raw_obj(&m));
    }

    /// Go's `newRuleFile`: a .claude/rules/<name>.md not there yet, in the workspace or an extra folder.
    fn new_rule_file(st: &State, p: &str) -> Option<InstrFile> {
        if !rule_name(&base(p)) {
            return None;
        }
        let d = dir(p);
        for root in std::iter::once(&st.cwd).chain(st.extra_dirs.iter()) {
            if d == join(&join(root, ".claude"), "rules") {
                return Some(InstrFile::new(p, true));
            }
        }
        None
    }

    /// Go's `resolveInstr`: ?path= mapped onto the managed set; for a write, also refused through a link or out of its
    /// folder.
    fn resolve_instr(&self, req: &Request, write: bool) -> Result<InstrFile, ApiErr> {
        let p = query_get(&req.url.raw_query, "path");
        if p.is_empty() || p.contains('\0') {
            return Err(bad("path is required"));
        }
        let st = self.settings();
        let (set, _) = self.instr_set(&st);
        let cp = clean(&p);
        let mut f = set.get(&cp).cloned();
        if f.is_none() && write {
            f = Server::new_rule_file(&st, &cp);
        }
        let Some(f) = f else {
            return Err(ApiErr { code: 403, msg: "that path is not one of the instruction files this console manages".into() });
        };
        if write {
            if !f.writable {
                return Err(ApiErr {
                    code: 403,
                    msg: "that file is outside the workspace, extra directories and global config directory".into(),
                });
            }
            let mut root = f.parent.clone();
            if base(&root) == "rules" {
                root = dir(&dir(&root));
            } else if base(&root) == ".claude" {
                root = dir(&root);
            }
            if std::fs::symlink_metadata(gopath::os_path(f.path.as_bytes())).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(ApiErr { code: 403, msg: "refusing to write through a symbolic link".into() });
            }
            let global = join(&self.home_dir(), ".claude");
            if !crate::service::inside(&root, &f.path) && !crate::service::inside(&global, &f.path) {
                return Err(ApiErr { code: 403, msg: "the path resolves outside its directory".into() });
            }
        }
        Ok(f)
    }

    pub(super) fn read_instruction(&self, req: &Request, w: &mut Response) {
        let f = match self.resolve_instr(req, false) {
            Ok(f) => f,
            Err(e) => return fail(w, &e),
        };
        let Ok(raw) = read_head(&f.path, 400 << 10) else { return detail(w, 404, "file not found") };
        let text = gotext::lossy(&raw);
        let clean = self.scrubber()(&text);
        let redacted = clean != text;
        write_json(
            w,
            200,
            &obj(vec![
                ("path", s(&f.path)),
                ("content", s(&clean)),
                ("redacted", Any::Bool(redacted)),
                ("writable", Any::Bool(f.writable)),
            ]),
        );
    }

    pub(super) fn write_instruction(&self, ex: &mut Exchange, w: &mut Response) {
        let f = match self.resolve_instr(&ex.req, true) {
            Ok(f) => f,
            Err(e) => return fail(w, &e),
        };
        let Some(body) = read_json(ex, w) else { return };
        let Some(v) = super::decode(&body, &CONTENT, w) else { return };
        let content = match gostruct::field(&CONTENT, &v, "content") {
            Some(Val::Ptr(Some(b))) => match &**b {
                Val::Str(t) => t.clone(),
                _ => String::new(),
            },
            _ => return fail(w, &wrong_type("content", "a string")),
        };
        if content.len() > 200 << 10 {
            return detail(w, 413, "file too large");
        }
        if content.contains("[REDACTED") {
            return detail(w, 400, "the text contains a [REDACTED] marker; saving would overwrite the real value");
        }
        let d = dir(&f.path);
        if crate::goos::mkdir_all(d.as_bytes()).is_err() {
            return detail(w, 500, "cannot create the directory");
        }
        let Ok((mut tmp, name)) = crate::goos::create_temp(d.as_bytes(), ".sarina-*.tmp") else {
            return detail(w, 500, "cannot write the file");
        };
        let wrote = std::io::Write::write_all(&mut tmp, content.as_bytes()).is_ok();
        drop(tmp);
        if !wrote || std::fs::rename(gopath::os_path(&name), gopath::os_path(f.path.as_bytes())).is_err() {
            let _ = std::fs::remove_file(gopath::os_path(&name));
            return detail(w, 500, "cannot write the file");
        }
        write_json(w, 200, &obj(vec![("path", s(&f.path)), ("size", Any::Int(content.len() as i64))]));
    }

    pub(super) fn delete_instruction(&self, req: &Request, w: &mut Response) {
        let f = match self.resolve_instr(req, true) {
            Ok(f) => f,
            Err(e) => return fail(w, &e),
        };
        if !go_remove(&f.path) {
            return detail(w, 404, "file not found");
        }
        write_json(w, 200, &obj(vec![("deleted", s(&f.path))]));
    }

    pub(super) fn tier_status(&self, w: &mut Response) {
        use crate::tiers::config as tc;
        let scrub = self.scrubber();
        let st = self.settings();
        let getenv = self.cfg.getenv.clone();
        let (mut cfg, err) = tc::load(&self.cfg.home, Some(&*getenv));
        if err.is_none() {
            if let Some(mut o) = self.cfg.escalation.clone() {
                o.accept_edits = st.allow_write || st.allow_shell;
                cfg = tc::from_escalation(&cfg, &crate::config::EscalationConfig::default(), &o, Some(&*getenv));
            }
        }
        let warnings = cfg.warnings.as_ref().map_or(Any::Null, |ws| Any::Array(ws.iter().map(|x| s(x)).collect()));
        let mut out = BTreeMap::new();
        out.insert("configured".to_owned(), Any::Bool(false));
        out.insert("config_path".to_owned(), s(&cfg.path));
        out.insert("tiers".to_owned(), Any::Array(Vec::new()));
        out.insert("warnings".to_owned(), warnings);
        if let Some(e) = err {
            out.insert("error".into(), s(&scrub(&e)));
            return write_json(w, 200, &Any::Object(out));
        }
        let (m, _) = st.model_runtime();
        let multi = cfg.multi();
        let g = getenv.clone();
        let router = match crate::tiers::router::Router::new(crate::tiers::router::Options {
            config: cfg,
            base: m,
            getenv: Box::new(move |k: &str| g(k)),
            ..crate::tiers::router::Options::default()
        }) {
            Ok(r) => r,
            Err(e) => {
                out.insert("error".into(), s(&scrub(&e)));
                return write_json(w, 200, &Any::Object(out));
            }
        };
        let ctx = Ctx::with_deadline(Instant::now() + Duration::from_secs(15));
        let rows: Vec<Any> = router
            .status(&ctx)
            .iter()
            .map(|t| {
                let mut row = BTreeMap::new();
                for (k, v) in [
                    ("name", s(&t.name)),
                    ("kind", s(&t.kind)),
                    ("base_url", s(&scrub(&t.base_url))),
                    ("model", s(&t.model)),
                    ("api_key_env", s(&t.api_key_env)),
                    ("api_key_set", Any::Bool(t.key_set)),
                    ("allow_data", s(&t.allow_data)),
                    ("max_context", Any::Int(t.max_context)),
                    ("enabled", Any::Bool(t.enabled)),
                    ("auto", Any::Bool(t.auto)),
                    ("healthy", Any::Bool(t.healthy)),
                    ("why", s(&scrub(&t.why))),
                    ("current", Any::Bool(t.current)),
                ] {
                    row.insert(k.to_owned(), v);
                }
                if t.kind == tc::KIND_CLAUDE_CLI {
                    row.insert("cli_path".into(), s(&scrub(&t.cli_path)));
                    row.insert("cli_found".into(), Any::Bool(t.cli_found));
                }
                Any::Object(row)
            })
            .collect();
        out.insert("configured".into(), Any::Bool(multi));
        out.insert("tiers".into(), Any::Array(rows));
        let (_, why) = router.pressure(&ctx);
        if !why.is_empty() {
            out.insert("pressure".into(), s(&why));
        }
        write_json(w, 200, &Any::Object(out));
    }

    /// Go's `backend`: whether the model backend answers (GET <base_url>/models, 4 s).
    pub(super) fn backend(&self, w: &mut Response) {
        let st = self.settings();
        let scrub = self.scrubber();
        let mut out = BTreeMap::new();
        out.insert("base_url".to_owned(), s(&scrub(&st.base_url)));
        out.insert("model".to_owned(), s(&st.model));
        out.insert("ok".to_owned(), Any::Bool(false));
        let target = format!("{}/models", st.base_url.trim_end_matches('/'));
        if crate::gourl::parse(&target).is_none() {
            out.insert("error".into(), s("the base URL is not valid"));
            return write_json(w, 200, &Any::Object(out));
        }
        let auth = format!("Bearer {}", st.api_key);
        let headers: Vec<(&str, &str)> = if st.api_key.is_empty() { Vec::new() } else { vec![("Authorization", auth.as_str())] };
        let o =
            crate::gohttp::Opts { dial: crate::gohttp::Dial::Tcp, method: "GET", url: &target, headers: &headers, body: None, tls: None };
        let ctx = Ctx::with_deadline(Instant::now() + Duration::from_secs(4));
        let deadline: crate::gohttp::Deadline = std::rc::Rc::new(std::cell::Cell::new(Instant::now() + Duration::from_secs(4)));
        let mut resp = match crate::gohttp::open(&o, &ctx, &deadline) {
            Ok(r) => r,
            Err(f) => {
                let what = match f {
                    crate::gohttp::Fail::Other(t) => t,
                    _ => "context deadline exceeded".into(),
                };
                out.insert("error".into(), s(&scrub(&crate::gohttp::go_url_error("GET", &target, &what))));
                return write_json(w, 200, &Any::Object(out));
            }
        };
        let mut buf = [0u8; 16384];
        let mut n = 0usize;
        while n < 1 << 20 {
            let want = buf.len().min((1 << 20) - n);
            match resp.body.read(&mut buf[..want]) {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        let code = resp.status;
        let ok = (200..300).contains(&code);
        out.insert("status".into(), Any::Int(i64::from(code)));
        out.insert("ok".into(), Any::Bool(ok));
        if !ok {
            out.insert("error".into(), s(&format!("the backend answered HTTP {code}")));
        }
        write_json(w, 200, &Any::Object(out));
    }
}
