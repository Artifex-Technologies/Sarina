//! Go: `go/tools/todo.go`: the agent's own to-do list and plan (task_list, task_create, task_update, todo_write, plan_get,
//! update_plan) in two small JSON files under `<workspace>/.port_sessions/`, resolved so a planted link cannot lead them
//! out of the workspace; the dependency rules (a dependency blocks while it exists and is not completed, an unknown one
//! is reported and ignored), cycles refused, the plan merged into the list by id (testdata/parity/todo.json). Nothing
//! here has to do with background processes (TaskOutput, TaskStop).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::goany::Any;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext::{self, trim_space};
use crate::tools::files::{arg_bool, arg_int, arg_str, fail, ok, sha256_hex};
use crate::tools::fsutil::{preview, rel_path, resolve_for_write, truncate_output};
use crate::tools::registry::{Context, Output, Tool};
use crate::{gojson, goos, gopath};

const TASK_FILE: &str = "task_runtime.json";
const PLAN_FILE: &str = "plan_runtime.json";
const WAITING_PREFIX: &str = "waiting_on:";

/// One process-wide lock: these files are tiny.
static STORE: Mutex<()> = Mutex::new(());

fn lock_store() -> std::sync::MutexGuard<'static, ()> {
    STORE.lock().unwrap_or_else(|p| p.into_inner())
}

/// One to-do item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub active_form: Option<String>,
    pub owner: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
    pub metadata: BTreeMap<String, Any>,
    pub created_at: String,
    pub updated_at: String,
}

/// One step of the plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanStep {
    pub step: String,
    pub status: String,
    pub task_id: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub active_form: Option<String>,
    pub owner: Option<String>,
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct PlanFile {
    explanation: Option<String>,
    updated_at: Option<String>,
    steps: Vec<PlanStep>,
}

// ---- small helpers ---------------------------------------------------------------------------------------------------------

/// Go's `time.Now().UTC().Format(time.RFC3339)`.
fn now_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Howard Hinnant's civil_from_days
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

fn str_ptr(s: &str) -> Option<String> {
    let s = trim_space(s);
    (!s.is_empty()).then(|| s.to_owned())
}

fn deref(p: &Option<String>) -> &str {
    p.as_deref().unwrap_or("")
}

fn norm_status(s: &str) -> String {
    let s = gotext::to_lower(trim_space(s)).replace(['-', ' '], "_");
    match s.as_str() {
        "pending" | "in_progress" | "completed" | "blocked" | "cancelled" => s,
        "done" | "complete" => "completed".into(),
        "todo" | "open" => "pending".into(),
        "blocked_on" => "blocked".into(),
        _ => "pending".into(),
    }
}

fn dedupe<S: AsRef<str>>(ids: &[S]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        let id = trim_space(id.as_ref());
        if !id.is_empty() && seen.insert(id.to_owned()) {
            out.push(id.to_owned());
        }
    }
    out
}

fn rand_hex(n: usize) -> String {
    use ring::rand::SecureRandom;
    let mut b = vec![0u8; n];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    h[..n].to_owned()
}

/// Go's `%v` of a JSON value.
fn fmt_v(v: &Any) -> String {
    match v {
        Any::Null => "<nil>".into(),
        Any::Bool(b) => b.to_string(),
        Any::Number(f) => gotext::float_v(*f),
        Any::Int(i) => i.to_string(),
        Any::Num(s) | Any::String(s) => s.clone(),
        Any::Array(a) => format!("[{}]", a.iter().map(fmt_v).collect::<Vec<_>>().join(" ")),
        Any::Object(m) => format!("map[{}]", m.iter().map(|(k, v)| format!("{k}:{}", fmt_v(v))).collect::<Vec<_>>().join(" ")),
    }
}

// ---- storage ---------------------------------------------------------------------------------------------------------------

/// Go's `storePath`: <workspace>/.port_sessions/<name>, resolved as a file tool's write is.
fn store_path(tc: &Context, name: &str) -> Result<Vec<u8>, String> {
    resolve_for_write(&tc.root, &format!(".port_sessions/{name}"))
}

/// Go's `writeFileAtomic`: a temporary file in the same folder, renamed over the old one.
fn write_file_atomic(path: &[u8], data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let dir = gopath::dir(path);
    goos::mkdir_all(&dir)?;
    let (mut f, name) = goos::create_temp(&dir, ".tmp-*")?;
    let res = f.write_all(data).map_err(|e| crate::goerr::path("write", &name, &e));
    drop(f);
    if let Err(e) = res {
        let _ = std::fs::remove_file(gopath::os_path(&name));
        return Err(e);
    }
    if let Err(e) = std::fs::rename(gopath::os_path(&name), gopath::os_path(path)) {
        let _ = std::fs::remove_file(gopath::os_path(&name));
        return Err(format!("rename {} {}: {}", gotext::lossy(&name), gotext::lossy(path), crate::goerr::text(&e)));
    }
    Ok(())
}

static STR: Ty = Ty::Str;
static PSTR: Ty = Ty::Ptr(&STR);
static STRS: Ty = Ty::Slice(&STR);
static ANY: Ty = Ty::Any;
static MAP: Ty = Ty::Map(&ANY);
static TASK_FIELDS: [Field; 13] = [
    Field("task_id", &STR),
    Field("title", &STR),
    Field("status", &STR),
    Field("description", &PSTR),
    Field("priority", &PSTR),
    Field("active_form", &PSTR),
    Field("owner", &PSTR),
    Field("blocks", &STRS),
    Field("blocked_by", &STRS),
    Field("metadata", &MAP),
    Field("created_at", &STR),
    Field("updated_at", &STR),
    Field("id", &STR),
];
static TASK_TY: Ty = Ty::Struct("struct", "", &TASK_FIELDS);
static TASKS_TY: Ty = Ty::Slice(&TASK_TY);
static TASK_FILE_FIELDS: [Field; 1] = [Field("tasks", &TASKS_TY)];
static TASK_FILE_TY: Ty = Ty::Struct("struct { Tasks []struct }", "", &TASK_FILE_FIELDS);
static STEP_FIELDS: [Field; 8] = [
    Field("step", &STR),
    Field("status", &STR),
    Field("task_id", &STR),
    Field("description", &PSTR),
    Field("priority", &PSTR),
    Field("active_form", &PSTR),
    Field("owner", &PSTR),
    Field("depends_on", &STRS),
];
static STEP_TY: Ty = Ty::Struct("tools.PlanStep", "PlanStep", &STEP_FIELDS);
static STEPS_TY: Ty = Ty::Slice(&STEP_TY);
static PLAN_FIELDS: [Field; 3] = [Field("explanation", &PSTR), Field("updated_at", &PSTR), Field("steps", &STEPS_TY)];
static PLAN_TY: Ty = Ty::Struct("tools.planFile", "planFile", &PLAN_FIELDS);

fn v_str(v: &Val) -> String {
    match v {
        Val::Str(s) => s.clone(),
        _ => String::new(),
    }
}

fn v_ptr(v: &Val) -> Option<String> {
    match v {
        Val::Ptr(Some(b)) => Some(v_str(b)),
        _ => None,
    }
}

fn v_strs(v: &Val) -> Vec<String> {
    match v {
        Val::Slice(Some(items)) => items.iter().map(v_str).collect(),
        _ => Vec::new(),
    }
}

/// Go's `loadTasks`: the list, tolerantly read (a task without an id or a title is dropped, statuses normalised); a file
/// that is missing, damaged or of the wrong shape reads as empty.
fn load_tasks(path: &[u8]) -> Vec<Task> {
    let Ok(data) = std::fs::read(gopath::os_path(path)) else { return Vec::new() };
    let (val, err) = gostruct::unmarshal(&TASK_FILE_TY, &data);
    if err.is_some() {
        return Vec::new();
    }
    let Val::Struct(top) = val else { return Vec::new() };
    let Val::Slice(Some(items)) = &top[0] else { return Vec::new() };
    let mut out = Vec::new();
    for it in items {
        let Val::Struct(f) = it else { continue };
        let mut id = v_str(&f[0]);
        if id.is_empty() {
            id = v_str(&f[12]);
        }
        let (id, title) = (trim_space(&id).to_owned(), trim_space(&v_str(&f[1])).to_owned());
        if id.is_empty() || title.is_empty() {
            continue;
        }
        let metadata = match &f[9] {
            Val::Map(Some(m)) => m
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        match v {
                            Val::Any(a) => a.clone(),
                            _ => Any::Null,
                        },
                    )
                })
                .collect(),
            _ => BTreeMap::new(),
        };
        out.push(Task {
            task_id: id,
            title,
            status: norm_status(&v_str(&f[2])),
            description: str_ptr(deref(&v_ptr(&f[3]))),
            priority: str_ptr(deref(&v_ptr(&f[4]))),
            active_form: str_ptr(deref(&v_ptr(&f[5]))),
            owner: str_ptr(deref(&v_ptr(&f[6]))),
            blocks: dedupe(&v_strs(&f[7])),
            blocked_by: dedupe(&v_strs(&f[8])),
            metadata,
            created_at: v_str(&f[10]),
            updated_at: v_str(&f[11]),
        });
    }
    out
}

fn push_key(o: &mut String, k: &str) {
    gojson::write_str(o, k, true);
    o.push(':');
}

fn push_opt(o: &mut String, v: &Option<String>) {
    match v {
        Some(s) => gojson::write_str(o, s, true),
        None => o.push_str("null"),
    }
}

fn push_list(o: &mut String, l: &[String]) {
    o.push('[');
    for (i, s) in l.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        gojson::write_str(o, s, true);
    }
    o.push(']');
}

/// Go's `encodeTasks`: json.MarshalIndent of {"tasks": [...]} with two spaces, and a line break.
fn encode_tasks(tasks: &[Task]) -> Vec<u8> {
    let mut o = String::from("{\"tasks\":[");
    for (i, t) in tasks.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push('{');
        push_key(&mut o, "task_id");
        gojson::write_str(&mut o, &t.task_id, true);
        o.push(',');
        push_key(&mut o, "title");
        gojson::write_str(&mut o, &t.title, true);
        o.push(',');
        push_key(&mut o, "status");
        gojson::write_str(&mut o, &t.status, true);
        for (k, v) in [("description", &t.description), ("priority", &t.priority), ("active_form", &t.active_form), ("owner", &t.owner)] {
            o.push(',');
            push_key(&mut o, k);
            push_opt(&mut o, v);
        }
        o.push(',');
        push_key(&mut o, "blocks");
        push_list(&mut o, &t.blocks);
        o.push(',');
        push_key(&mut o, "blocked_by");
        push_list(&mut o, &t.blocked_by);
        o.push(',');
        push_key(&mut o, "metadata");
        // a value JSON cannot hold (none reaches here from JSON arguments) is written as null rather than lost
        if gojson::write_any(&mut o, &Any::Object(t.metadata.clone()), true).is_err() {
            o.push_str("null");
        }
        o.push(',');
        push_key(&mut o, "created_at");
        gojson::write_str(&mut o, &t.created_at, true);
        o.push(',');
        push_key(&mut o, "updated_at");
        gojson::write_str(&mut o, &t.updated_at, true);
        o.push('}');
    }
    o.push_str("]}");
    let mut out = gojson::indent(&o, "  ").into_bytes();
    out.push(b'\n');
    out
}

/// Go's `loadPlan`.
fn load_plan(path: &[u8]) -> PlanFile {
    let Ok(data) = std::fs::read(gopath::os_path(path)) else { return PlanFile::default() };
    let (val, err) = gostruct::unmarshal(&PLAN_TY, &data);
    if err.is_some() {
        return PlanFile::default();
    }
    let Val::Struct(top) = val else { return PlanFile::default() };
    let mut p = PlanFile { explanation: v_ptr(&top[0]), updated_at: v_ptr(&top[1]), steps: Vec::new() };
    if let Val::Slice(Some(items)) = &top[2] {
        for it in items {
            let Val::Struct(f) = it else { continue };
            let step = trim_space(&v_str(&f[0])).to_owned();
            if step.is_empty() {
                continue;
            }
            p.steps.push(PlanStep {
                step,
                status: norm_status(&v_str(&f[1])),
                task_id: v_str(&f[2]),
                description: v_ptr(&f[3]),
                priority: v_ptr(&f[4]),
                active_form: v_ptr(&f[5]),
                owner: v_ptr(&f[6]),
                depends_on: dedupe(&v_strs(&f[7])),
            });
        }
    }
    p
}

/// Go's `encodePlan`.
fn encode_plan(p: &PlanFile) -> Vec<u8> {
    let mut o = String::from("{");
    push_key(&mut o, "explanation");
    push_opt(&mut o, &p.explanation);
    o.push(',');
    push_key(&mut o, "updated_at");
    push_opt(&mut o, &p.updated_at);
    o.push(',');
    push_key(&mut o, "steps");
    o.push('[');
    for (i, s) in p.steps.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push('{');
        for (j, (k, v)) in [("step", &s.step), ("status", &s.status), ("task_id", &s.task_id)].into_iter().enumerate() {
            if j > 0 {
                o.push(',');
            }
            push_key(&mut o, k);
            gojson::write_str(&mut o, v, true);
        }
        for (k, v) in [("description", &s.description), ("priority", &s.priority), ("active_form", &s.active_form), ("owner", &s.owner)] {
            o.push(',');
            push_key(&mut o, k);
            push_opt(&mut o, v);
        }
        o.push(',');
        push_key(&mut o, "depends_on");
        push_list(&mut o, &s.depends_on);
        o.push('}');
    }
    o.push_str("]}");
    let mut out = gojson::indent(&o, "  ").into_bytes();
    out.push(b'\n');
    out
}

// ---- ordering and dependency rules --------------------------------------------------------------------------------------

fn status_rank(s: &str) -> u8 {
    match s {
        "in_progress" => 0,
        "pending" => 1,
        "blocked" => 2,
        "completed" => 3,
        _ => 4,
    }
}

fn priority_rank(p: &Option<String>) -> u8 {
    match gotext::to_lower(deref(p)).as_str() {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}

fn sort_tasks(ts: &mut [Task]) {
    ts.sort_by(|a, b| {
        status_rank(&a.status)
            .cmp(&status_rank(&b.status))
            .then_with(|| priority_rank(&a.priority).cmp(&priority_rank(&b.priority)))
            .then_with(|| gotext::to_lower(&a.title).cmp(&gotext::to_lower(&b.title)))
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
}

fn find_task(ts: &[Task], id: &str) -> Option<usize> {
    ts.iter().position(|t| t.task_id == id)
}

fn is_finished(status: &str) -> bool {
    status == "completed" || status == "cancelled"
}

/// The dependencies that exist and are not completed (a cancelled one still blocks).
fn unresolved(ts: &[Task], t: &Task) -> Vec<String> {
    t.blocked_by.iter().filter(|id| find_task(ts, id).is_some_and(|i| ts[i].status != "completed")).cloned().collect()
}

/// The dependencies that name no task.
fn unknown_deps(ts: &[Task], t: &Task) -> Vec<String> {
    t.blocked_by.iter().filter(|id| find_task(ts, id).is_none()).cloned().collect()
}

fn actionable(ts: &[Task], t: &Task) -> bool {
    t.status == "in_progress" || (t.status == "pending" && unresolved(ts, t).is_empty())
}

fn blocked_reason(t: &Task) -> String {
    match t.metadata.get("blocked_reason") {
        Some(Any::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Go's `normalizeTasks`: the dependency rules applied to the whole list; whether anything changed. Idempotent.
fn normalize_tasks(ts: &mut [Task]) -> bool {
    let mut changed = false;
    for i in 0..ts.len() {
        let un = unresolved(ts, &ts[i]);
        let unknown = unknown_deps(ts, &ts[i]).len();
        let t = &mut ts[i];
        let reason = blocked_reason(t);
        if t.status == "in_progress" && !un.is_empty() {
            t.status = "blocked".into();
            t.metadata.insert("blocked_reason".into(), Any::String(format!("{WAITING_PREFIX}{}", un.join(","))));
        } else if t.status == "blocked"
            && un.is_empty()
            && (reason.starts_with(WAITING_PREFIX) || (!t.blocked_by.is_empty() && unknown < t.blocked_by.len()))
        {
            t.status = "pending".into();
            t.metadata.remove("blocked_reason");
        } else if t.status == "blocked" && reason.starts_with(WAITING_PREFIX) {
            let want = format!("{WAITING_PREFIX}{}", un.join(","));
            if reason == want {
                continue;
            }
            t.metadata.insert("blocked_reason".into(), Any::String(want));
        } else {
            continue;
        }
        t.updated_at = now_stamp();
        changed = true;
    }
    changed
}

/// Go's `cyclePath`: the dependency path from start back to start (depth first, in dependency order), or None. A loop,
/// not recursion: a planted list could chain thousands of tasks.
fn cycle_path(start: &str, deps: &dyn Fn(&str) -> Vec<String>) -> Option<Vec<String>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut path = vec![start.to_owned()];
    let mut frames: Vec<(Vec<String>, usize)> = vec![(deps(start), 0)];
    while let Some((ds, idx)) = frames.last_mut() {
        if *idx >= ds.len() {
            frames.pop();
            path.pop();
            continue;
        }
        let d = ds[*idx].clone();
        *idx += 1;
        if d == start {
            let mut p = path.clone();
            p.push(d);
            return Some(p);
        }
        if seen.insert(d.clone()) {
            let next = deps(&d);
            path.push(d);
            frames.push((next, 0));
        }
    }
    None
}

fn cycle_error(p: &[String]) -> String {
    if p.len() == 2 {
        return format!("task {} cannot depend on itself; nothing was changed", p[0]);
    }
    format!("dependency cycle {}; nothing was changed. Remove one of those dependencies first", p.join(" -> "))
}

fn check_cycles(ts: &[Task], ids: &[String]) -> Result<(), String> {
    let deps = |id: &str| find_task(ts, id).map(|i| ts[i].blocked_by.clone()).unwrap_or_default();
    for id in ids {
        if let Some(p) = cycle_path(id, &deps) {
            return Err(cycle_error(&p));
        }
    }
    Ok(())
}

/// Go's `depNotes`.
fn dep_notes(ts: &[Task], t: &Task) -> Vec<String> {
    let mut notes = Vec::new();
    if !is_finished(&t.status) {
        let parts: Vec<String> = unresolved(ts, t)
            .iter()
            .map(|id| {
                let st = &ts[find_task(ts, id).unwrap_or(0)].status;
                if st == "cancelled" {
                    format!("{id} (cancelled - remove or replace it)")
                } else {
                    format!("{id} ({st})")
                }
            })
            .collect();
        if !parts.is_empty() {
            notes.push(format!("waiting_on: {}", parts.join(", ")));
        }
    }
    for id in unknown_deps(ts, t) {
        notes.push(format!("depends on unknown task {id} (ignored, not blocking)"));
    }
    let r = blocked_reason(t);
    if t.status == "blocked" && !r.is_empty() && !r.starts_with(WAITING_PREFIX) {
        notes.push(format!("blocked_reason: {r}"));
    }
    notes
}

// ---- rendering ---------------------------------------------------------------------------------------------------------------

fn render_task_line(b: &mut String, t: &Task) {
    b.push_str(&format!("- {} [{}]", t.task_id, t.status));
    if let Some(p) = &t.priority {
        b.push_str(&format!(" ({p})"));
    }
    if let Some(o) = &t.owner {
        b.push_str(&format!(" @{o}"));
    }
    b.push_str(&format!(" {}\n", t.title));
}

fn render_task_details(b: &mut String, ts: &[Task], t: &Task) {
    if let Some(d) = &t.description {
        b.push_str(&format!("    description: {d}\n"));
    }
    if let Some(a) = &t.active_form {
        b.push_str(&format!("    active: {a}\n"));
    }
    if !t.blocked_by.is_empty() {
        b.push_str(&format!("    blocked_by: {}\n", t.blocked_by.join(", ")));
    }
    for n in dep_notes(ts, t) {
        b.push_str(&format!("    {n}\n"));
    }
    if !t.blocks.is_empty() {
        b.push_str(&format!("    blocks: {}\n", t.blocks.join(", ")));
    }
}

fn render_task_full(ts: &[Task], t: &Task) -> String {
    let mut b = format!("id: {}\nstatus: {}\ntitle: {}\n", t.task_id, t.status, t.title);
    for (k, v) in [("priority", &t.priority), ("owner", &t.owner), ("active form", &t.active_form), ("description", &t.description)] {
        if !deref(v).is_empty() {
            b.push_str(&format!("{k}: {}\n", deref(v)));
        }
    }
    if !t.blocked_by.is_empty() {
        b.push_str(&format!("blocked_by: {}\n", t.blocked_by.join(", ")));
    }
    for n in dep_notes(ts, t) {
        b.push_str(&format!("{n}\n"));
    }
    if !t.blocks.is_empty() {
        b.push_str(&format!("blocks: {}\n", t.blocks.join(", ")));
    }
    for (k, v) in &t.metadata {
        b.push_str(&format!("{k}={}\n", fmt_v(v)));
    }
    b.push_str(&format!("updated_at: {}", t.updated_at));
    b
}

fn int(n: usize) -> Any {
    Any::Int(n as i64)
}

fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

fn file_md(md: &mut BTreeMap<String, Any>, before: &Option<Vec<u8>>, after: &[u8]) {
    md.insert("after_sha256".into(), s(&sha256_hex(after)));
    md.insert("after_preview".into(), s(&preview(&gotext::lossy(after), 140)));
    match before {
        None => {
            md.insert("before_sha256".into(), Any::Null);
            md.insert("before_preview".into(), Any::Null);
        }
        Some(b) => {
            md.insert("before_sha256".into(), s(&sha256_hex(b)));
            md.insert("before_preview".into(), s(&preview(&gotext::lossy(b), 140)));
        }
    }
}

/// How a mutation ends: a guide delivered as a normal result (nothing changed), or a failure.
enum MutErr {
    Guide(String),
    Fail(String),
}

/// Go's `mutateTasks`: load the list, run f, apply the rules, save, and the mutation's metadata. A failed f leaves the
/// file as it was.
fn mutate_tasks(tc: &Context, action: &str, f: impl FnOnce(&mut Vec<Task>) -> Result<(String, Option<String>), MutErr>) -> Output {
    let _g = lock_store();
    let path = match store_path(tc, TASK_FILE) {
        Ok(p) => p,
        Err(e) => return fail(format!("cannot use the task list: {e}")),
    };
    let before = std::fs::read(gopath::os_path(&path)).ok();
    let mut ts = load_tasks(&path);
    let nb = ts.len();
    let (msg, touched) = match f(&mut ts) {
        Ok(r) => r,
        Err(MutErr::Guide(m)) => {
            return Output {
                content: m,
                metadata: BTreeMap::from([("guide".to_owned(), Any::Bool(true)), ("changed".to_owned(), Any::Bool(false))]),
                error: None,
            }
        }
        Err(MutErr::Fail(e)) => return fail(e),
    };
    normalize_tasks(&mut ts);
    let data = encode_tasks(&ts);
    if let Err(e) = write_file_atomic(&path, &data) {
        return fail(format!("cannot save the task list: {e}"));
    }
    let mut md = BTreeMap::from([
        ("action".to_owned(), s(action)),
        ("path".to_owned(), s(&gotext::lossy(&rel_path(&tc.root, &path)))),
        ("before_task_count".to_owned(), int(nb)),
        ("after_task_count".to_owned(), int(ts.len())),
        ("total_tasks".to_owned(), int(ts.len())),
    ]);
    file_md(&mut md, &before, &data);
    if let Some(id) = touched {
        if let Some(i) = find_task(&ts, &id) {
            md.insert("task_id".into(), s(&ts[i].task_id));
            md.insert("task_status".into(), s(&ts[i].status));
        }
    }
    Output { content: msg, metadata: md, error: None }
}

// ---- argument handling ------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct TaskPatch {
    id: String,
    title: Option<String>,
    status: Option<String>,
    description: Option<String>,
    priority: Option<String>,
    active_form: Option<String>,
    owner: Option<String>,
    blocks: Option<Vec<String>>,
    blocked_by: Option<Vec<String>>,
    metadata: Option<BTreeMap<String, Any>>,
}

fn arg_list(m: &BTreeMap<String, Any>, k: &str) -> Option<Vec<String>> {
    match m.get(k) {
        Some(Any::Array(l)) => Some(
            l.iter()
                .filter_map(|x| match x {
                    Any::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
        ),
        _ => None,
    }
}

fn patch_of(m: &BTreeMap<String, Any>) -> TaskPatch {
    let st = |k: &str| match m.get(k) {
        Some(Any::String(s)) => Some(s.clone()),
        _ => None,
    };
    let mut id = trim_space(&arg_str(m, "task_id")).to_owned();
    if id.is_empty() {
        id = trim_space(&arg_str(m, "id")).to_owned();
    }
    TaskPatch {
        id,
        title: st("title"),
        status: st("status"),
        description: st("description"),
        priority: st("priority"),
        active_form: st("active_form"),
        owner: st("owner"),
        blocks: arg_list(m, "blocks").map(|l| dedupe(&l)),
        blocked_by: arg_list(m, "blocked_by").map(|l| dedupe(&l)),
        metadata: match m.get("metadata") {
            Some(Any::Object(o)) => Some(o.clone()),
            _ => None,
        },
    }
}

impl TaskPatch {
    fn apply(&self, t: &mut Task) {
        if let Some(title) = &self.title {
            if !trim_space(title).is_empty() {
                t.title = trim_space(title).to_owned();
            }
        }
        if let Some(st) = &self.status {
            t.status = norm_status(st);
        }
        if let Some(d) = &self.description {
            t.description = str_ptr(d);
        }
        if let Some(p) = &self.priority {
            t.priority = str_ptr(p);
        }
        if let Some(a) = &self.active_form {
            t.active_form = str_ptr(a);
        }
        if let Some(o) = &self.owner {
            t.owner = str_ptr(o);
        }
        if let Some(b) = &self.blocks {
            t.blocks = b.clone();
        }
        if let Some(b) = &self.blocked_by {
            t.blocked_by = b.clone();
        }
        if let Some(m) = &self.metadata {
            t.metadata = m.clone();
        }
        t.updated_at = now_stamp();
    }
}

fn with_notes(msg: String, notes: &[String]) -> String {
    let mut msg = msg;
    for n in notes {
        msg.push('\n');
        msg.push_str(n);
    }
    msg
}

/// Go's `notesFor`.
fn notes_for(ts: &[Task], t: &Task, waiting: &str) -> Vec<String> {
    let mut notes: Vec<String> = dep_notes(ts, t).into_iter().filter(|n| waiting.is_empty() || !n.starts_with("waiting_on:")).collect();
    if t.status == "cancelled" {
        let dependants: Vec<String> = ts
            .iter()
            .filter(|o| !is_finished(&o.status))
            .flat_map(|o| o.blocked_by.iter().filter(|id| **id == t.task_id).map(|_| o.task_id.clone()))
            .collect();
        if !dependants.is_empty() {
            notes.push(format!(
                "Still waiting on this cancelled task: {}. Remove or replace the dependency on each (task_update remove_blocked_by / \
                 add_blocked_by).",
                dependants.join(", ")
            ));
        }
    }
    notes
}

const TASK_CHANGE_FIELDS: &[&str] = &[
    "title",
    "description",
    "status",
    "priority",
    "active_form",
    "owner",
    "reason",
    "blocks",
    "blocked_by",
    "add_blocked_by",
    "remove_blocked_by",
    "metadata",
];

fn has_any_arg(args: &BTreeMap<String, Any>, keys: &[&str]) -> bool {
    keys.iter().any(|k| args.get(*k).is_some_and(|v| *v != Any::Null))
}

fn task_update_guide(current: &str) -> String {
    format!(
        "task_update changes one task; you gave only task_id, so nothing was changed. Add at least one field to change:\n\
         - status: pending, in_progress, completed, blocked or cancelled (reason explains blocked/cancelled). \
         Example: {{\"task_id\":\"t1\",\"status\":\"in_progress\"}}\n\
         - dependencies: add_blocked_by, remove_blocked_by (or blocked_by to replace the list). \
         Example: {{\"task_id\":\"t1\",\"add_blocked_by\":[\"t0\"]}}\n\
         - other fields: title, description, priority, owner, active_form, blocks, metadata.\n\n\
         Current state of the task:\n{current}"
    )
}

fn render_plan(p: &PlanFile) -> String {
    let mut b = String::new();
    if let Some(e) = &p.explanation {
        b.push_str(&format!("{e}\n"));
    }
    for (i, s) in p.steps.iter().enumerate() {
        b.push_str(&format!("{}. [{}] {} ({})", i + 1, s.status, s.step, s.task_id));
        if let Some(pr) = &s.priority {
            b.push_str(&format!(" priority={pr}"));
        }
        b.push('\n');
        if let Some(d) = &s.description {
            b.push_str(&format!("    {d}\n"));
        }
        if let Some(a) = &s.active_form {
            b.push_str(&format!("    active: {a}\n"));
        }
        if let Some(o) = &s.owner {
            b.push_str(&format!("    owner: {o}\n"));
        }
        if !s.depends_on.is_empty() {
            b.push_str(&format!("    depends_on: {}\n", s.depends_on.join(", ")));
        }
    }
    b.trim_end_matches('\n').to_owned()
}

fn update_plan_guide(note: &str, current: &PlanFile) -> String {
    let note = if note.is_empty() { "update_plan needs items or clear; nothing was changed." } else { note };
    let mut b = format!(
        "{note}\nTwo modes:\n\
         1. items: replace the plan with these steps (they are merged into the to-do list by id). \
         Example: {{\"items\":[{{\"step\":\"Write tests\",\"status\":\"in_progress\"}},{{\"step\":\"Fix the bug\",\"depends_on\":[\"plan_1\"]}}]}}\n\
         2. clear: true resets the plan; the to-do list is left alone unless prune_tasks is true, which also removes the tasks earlier plans created. \
         Example: {{\"clear\":true}} or {{\"clear\":true,\"prune_tasks\":true}}\n"
    );
    if current.steps.is_empty() {
        b.push_str("Current plan: none.");
    } else {
        b.push_str(&format!("Current plan:\n{}", render_plan(current)));
    }
    b
}

// ---- the tools ------------------------------------------------------------------------------------------------------------

/// Go's `readTasks`: the list with the rules applied once, written back only when they changed something and the session
/// may write.
fn read_tasks(tc: &Context) -> Vec<Task> {
    let _g = lock_store();
    let Ok(path) = store_path(tc, TASK_FILE) else { return Vec::new() };
    let mut ts = load_tasks(&path);
    if normalize_tasks(&mut ts) && tc.permissions.allow_file_write {
        let _ = write_file_atomic(&path, &encode_tasks(&ts));
    }
    ts
}

/// Go's `task_list` handler.
pub fn task_list(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let mut all = read_tasks(tc);
    let total = BTreeMap::from([("total_tasks".to_owned(), int(all.len()))]);
    let id = trim_space(&arg_str(args, "task_id")).to_owned();
    if !id.is_empty() {
        let Some(i) = find_task(&all, &id) else { return fail(format!("unknown task id {}", gotext::quote(&id))) };
        return Output { content: render_task_full(&all, &all[i]), metadata: total, error: None };
    }
    sort_tasks(&mut all);
    let (status, owner, only) = (arg_str(args, "status"), arg_str(args, "owner"), arg_bool(args, "actionable_only", false));
    let show: Vec<&Task> = all
        .iter()
        .filter(|t| status.is_empty() || t.status == norm_status(&status))
        .filter(|t| owner.is_empty() || deref(&t.owner) == owner)
        .filter(|t| !only || actionable(&all, t))
        .collect();
    if show.is_empty() {
        let msg = if only { "Nothing is ready to work on." } else { "No tasks." };
        return Output { content: msg.into(), metadata: total, error: None };
    }
    let limit = arg_int(args, "max_tasks", 50);
    let mut b = String::from("Tasks:\n");
    for (i, t) in show.iter().enumerate() {
        if i as i64 >= limit {
            b.push_str(&format!("(+{} more)\n", show.len() as i64 - limit));
            break;
        }
        render_task_line(&mut b, t);
        render_task_details(&mut b, &all, t);
    }
    Output { content: truncate_output(b.trim_end_matches('\n'), tc.max_output_chars), metadata: total, error: None }
}

/// Go's `task_create` handler.
pub fn task_create(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    mutate_tasks(tc, "task_create", |ts| {
        let mut p = patch_of(args);
        if p.title.as_deref().is_none_or(|t| trim_space(t).is_empty()) {
            return Err(MutErr::Fail("title is required".into()));
        }
        if p.id.is_empty() {
            p.id = format!("task_{}", rand_hex(10));
        }
        if find_task(ts, &p.id).is_some() {
            return Err(MutErr::Fail(format!("a task with id {} already exists; use task_update to change it", gotext::quote(&p.id))));
        }
        let mut t = Task { task_id: p.id.clone(), status: "pending".into(), created_at: now_stamp(), ..Task::default() };
        p.apply(&mut t);
        ts.push(t.clone());
        check_cycles(ts, &[t.task_id.clone()]).map_err(MutErr::Fail)?;
        let msg = format!(
            "Created task {}: {} [{}]. This is your own session list: no file changed. If the user asked to change a file (a todo.md, \
             a list in a document), edit that file with the file tools.",
            t.task_id, t.title, t.status
        );
        Ok((with_notes(msg, &notes_for(ts, &t, "")), Some(t.task_id)))
    })
}

/// Go's `task_update` handler.
pub fn task_update(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    mutate_tasks(tc, "task_update", |ts| {
        let p = patch_of(args);
        let Some(i) = find_task(ts, &p.id) else { return Err(MutErr::Fail(format!("unknown task id {}", gotext::quote(&p.id)))) };
        if !has_any_arg(args, TASK_CHANGE_FIELDS) {
            return Err(MutErr::Guide(task_update_guide(&render_task_full(ts, &ts[i]))));
        }
        p.apply(&mut ts[i]);
        let add = arg_list(args, "add_blocked_by").unwrap_or_default();
        let rm = arg_list(args, "remove_blocked_by").unwrap_or_default();
        if !add.is_empty() {
            let mut all = ts[i].blocked_by.clone();
            all.extend(add.iter().cloned());
            ts[i].blocked_by = dedupe(&all);
        }
        if !rm.is_empty() {
            let drop: HashSet<&str> = rm.iter().map(|id| trim_space(id)).collect();
            ts[i].blocked_by.retain(|id| !drop.contains(id.as_str()));
        }
        if p.blocked_by.is_some() || !add.is_empty() {
            check_cycles(ts, &[ts[i].task_id.clone()]).map_err(MutErr::Fail)?;
        }
        let mut waiting = String::new();
        if p.status.is_some() && ts[i].status == "in_progress" {
            let un = unresolved(ts, &ts[i]);
            if !un.is_empty() {
                ts[i].status = "blocked".into();
                ts[i].metadata.insert("blocked_reason".into(), Any::String(format!("{WAITING_PREFIX}{}", un.join(","))));
                waiting = un.join(", ");
            } else {
                ts[i].metadata.remove("blocked_reason");
            }
        }
        let reason = trim_space(&arg_str(args, "reason")).to_owned();
        if !reason.is_empty() {
            match ts[i].status.as_str() {
                "blocked" => {
                    ts[i].metadata.insert("blocked_reason".into(), Any::String(reason));
                }
                "cancelled" => {
                    ts[i].metadata.insert("cancel_reason".into(), Any::String(reason));
                }
                _ => {}
            }
        }
        normalize_tasks(ts);
        let out = ts[i].clone();
        if !waiting.is_empty() {
            let msg = format!("Task {} is blocked, waiting on {waiting}: {} [{}]", out.task_id, out.title, out.status);
            return Ok((with_notes(msg, &notes_for(ts, &out, &waiting)), Some(out.task_id)));
        }
        let msg = format!("Updated task {}: {} [{}]", out.task_id, out.title, out.status);
        Ok((with_notes(msg, &notes_for(ts, &out, "")), Some(out.task_id)))
    })
}

/// Go's `todo_write` handler.
pub fn todo_write(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    mutate_tasks(tc, "todo_write", |ts| {
        let items = match args.get("items") {
            Some(Any::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let old = ts.clone();
        let mut next: Vec<Task> = Vec::new();
        let mut used: HashSet<String> = HashSet::new();
        for (idx, raw) in items.iter().enumerate() {
            let Any::Object(m) = raw else { continue };
            let p = patch_of(m);
            let Some(title) = p.title.as_deref().map(trim_space).filter(|t| !t.is_empty()) else { continue };
            let mut t = Task { status: "pending".into(), created_at: now_stamp(), ..Task::default() };
            let mut id = p.id.clone();
            if id.is_empty() {
                // keep the id of an existing task with the same title
                if let Some(o) = old.iter().find(|o| o.title == title && !used.contains(&o.task_id)) {
                    id = o.task_id.clone();
                    t.created_at = o.created_at.clone();
                }
            } else if let Some(i) = find_task(&old, &id) {
                t.created_at = old[i].created_at.clone();
            }
            if id.is_empty() {
                id = format!("task_{}_{}", idx + 1, rand_hex(6));
            }
            if !used.insert(id.clone()) {
                return Err(MutErr::Fail(format!("task id {} appears twice in items", gotext::quote(&id))));
            }
            t.task_id = id;
            p.apply(&mut t);
            next.push(t);
        }
        *ts = next;
        let ids: Vec<String> = ts.iter().map(|t| t.task_id.clone()).collect();
        check_cycles(ts, &ids).map_err(MutErr::Fail)?;
        Ok((format!("Replaced the to-do list with {} tasks.", ts.len()), None))
    })
}

/// Go's `plan_get` handler.
pub fn plan_get(_: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let p = {
        let _g = lock_store();
        match store_path(tc, PLAN_FILE) {
            Ok(path) => load_plan(&path),
            Err(_) => PlanFile::default(),
        }
    };
    if p.steps.is_empty() {
        return ok("There is no plan.".into());
    }
    ok(render_plan(&p))
}

/// Go's `mergePlan`: the steps upserted into the list by id; tasks it creates are marked from_plan.
fn merge_plan(ts: &mut Vec<Task>, steps: &[PlanStep]) {
    let mut dependents: HashMap<&str, Vec<String>> = HashMap::new();
    for st in steps {
        for d in &st.depends_on {
            dependents.entry(d.as_str()).or_default().push(st.task_id.clone());
        }
    }
    let now = now_stamp();
    for st in steps {
        let mut blocks = dedupe(dependents.get(st.task_id.as_str()).map(Vec::as_slice).unwrap_or(&[]));
        blocks.sort();
        match find_task(ts, &st.task_id) {
            None => ts.push(Task {
                task_id: st.task_id.clone(),
                title: st.step.clone(),
                status: st.status.clone(),
                description: st.description.clone(),
                priority: st.priority.clone(),
                active_form: st.active_form.clone(),
                owner: st.owner.clone(),
                blocks,
                blocked_by: st.depends_on.clone(),
                metadata: BTreeMap::from([("from_plan".to_owned(), Any::Bool(true))]),
                created_at: now.clone(),
                updated_at: now.clone(),
            }),
            Some(i) => {
                let t = &mut ts[i];
                t.title = st.step.clone();
                t.status = st.status.clone();
                t.description = st.description.clone();
                t.priority = st.priority.clone();
                t.active_form = st.active_form.clone();
                t.owner = st.owner.clone();
                t.blocks = blocks;
                t.blocked_by = st.depends_on.clone();
                t.updated_at = now.clone();
            }
        }
    }
}

/// Go's `update_plan` handler.
pub fn update_plan(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let _g = lock_store();
    let ppath = match store_path(tc, PLAN_FILE) {
        Ok(p) => p,
        Err(e) => return fail(format!("cannot use the plan: {e}")),
    };
    let tpath = match store_path(tc, TASK_FILE) {
        Ok(p) => p,
        Err(e) => return fail(format!("cannot use the task list: {e}")),
    };
    let before = std::fs::read(gopath::os_path(&ppath)).ok();
    let old = load_plan(&ppath);
    let mut plan = PlanFile::default();
    let items = match args.get("items") {
        Some(Any::Array(a)) => Some(a.clone()),
        _ => None,
    };
    let clearing = arg_bool(args, "clear", false);
    let guide = |text: String| Output {
        content: text,
        metadata: BTreeMap::from([("guide".to_owned(), Any::Bool(true)), ("changed".to_owned(), Any::Bool(false))]),
        error: None,
    };
    if clearing && items.as_ref().is_some_and(|i| !i.is_empty()) {
        return guide(update_plan_guide("items and clear cannot be combined; nothing was changed.", &old));
    }
    if !clearing && items.is_none() {
        return guide(update_plan_guide("", &old));
    }
    let mut ids: HashSet<String> = HashSet::new();
    for raw in items.unwrap_or_default() {
        let Any::Object(m) = &raw else { continue };
        let step = trim_space(&arg_str(m, "step")).to_owned();
        if step.is_empty() {
            continue;
        }
        let mut id = trim_space(&arg_str(m, "task_id")).to_owned();
        if id.is_empty() {
            id = format!("plan_{}", plan.steps.len() + 1);
        }
        if !ids.insert(id.clone()) {
            return fail(format!("step id {} appears twice", gotext::quote(&id)));
        }
        plan.steps.push(PlanStep {
            step,
            status: norm_status(&arg_str(m, "status")),
            task_id: id,
            description: str_ptr(&arg_str(m, "description")),
            priority: str_ptr(&arg_str(m, "priority")),
            active_form: str_ptr(&arg_str(m, "active_form")),
            owner: str_ptr(&arg_str(m, "owner")),
            depends_on: dedupe(&arg_list(m, "depends_on").unwrap_or_default()),
        });
    }
    let step_deps: HashMap<String, Vec<String>> = plan.steps.iter().map(|st| (st.task_id.clone(), st.depends_on.clone())).collect();
    let deps = |id: &str| step_deps.get(id).cloned().unwrap_or_default();
    for st in &plan.steps {
        if let Some(cp) = cycle_path(&st.task_id, &deps) {
            return fail(cycle_error(&cp));
        }
    }
    if !clearing {
        plan.updated_at = Some(now_stamp());
        plan.explanation = str_ptr(&arg_str(args, "explanation"));
    }
    let data = encode_plan(&plan);
    let action = if clearing { "plan_clear" } else { "update_plan" };
    let mut md = BTreeMap::from([
        ("action".to_owned(), s(action)),
        ("path".to_owned(), s(&gotext::lossy(&rel_path(&tc.root, &ppath)))),
        ("before_plan_count".to_owned(), int(old.steps.len())),
        ("after_plan_count".to_owned(), int(plan.steps.len())),
        ("total_steps".to_owned(), int(plan.steps.len())),
    ]);
    file_md(&mut md, &before, &data);
    let sync = arg_bool(args, "sync_tasks", true);
    md.insert("sync_tasks".into(), Any::Bool(sync));
    if let Some(e) = &plan.explanation {
        md.insert("explanation".into(), s(e));
    }
    let prune = arg_bool(args, "prune_tasks", false);
    if sync && (!clearing || prune) {
        let mut tasks = load_tasks(&tpath);
        merge_plan(&mut tasks, &plan.steps);
        let step_ids: Vec<String> = plan.steps.iter().map(|st| st.task_id.clone()).collect();
        if let Err(e) = check_cycles(&tasks, &step_ids) {
            return fail(e);
        }
        if prune {
            tasks.retain(|t| !(t.metadata.get("from_plan") == Some(&Any::Bool(true)) && !ids.contains(&t.task_id)));
        }
        normalize_tasks(&mut tasks);
        let tdata = encode_tasks(&tasks);
        if let Err(e) = write_file_atomic(&tpath, &tdata) {
            return fail(format!("cannot save the task list: {e}"));
        }
        md.insert("synced_task_store_path".into(), s(&gotext::lossy(&rel_path(&tc.root, &tpath))));
        md.insert("synced_task_sha256".into(), s(&sha256_hex(&tdata)));
        md.insert("synced_tasks".into(), int(plan.steps.len()));
    }
    if let Err(e) = write_file_atomic(&ppath, &data) {
        return fail(format!("cannot save the plan: {e}"));
    }
    let content = if clearing { "Cleared the plan.".to_owned() } else { format!("The plan now has {} steps.", plan.steps.len()) };
    Output { content, metadata: md, error: None }
}

/// The to-do and plan tools with the Go build's definitions, in Go's order.
pub fn todo_tools() -> Vec<Tool> {
    use crate::tools::defs::tool;
    vec![
        tool("task_list", Arc::new(task_list)),
        tool("task_create", Arc::new(task_create)),
        tool("task_update", Arc::new(task_update)),
        tool("todo_write", Arc::new(todo_write)),
        tool("plan_get", Arc::new(plan_get)),
        tool("update_plan", Arc::new(update_plan)),
    ]
}
