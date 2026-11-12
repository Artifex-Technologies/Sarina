//! Go: `policy`: the operator's tool policy, a deny-list (exact names and prefixes), a trust switch and budget limits.
//! Operator files (~/.sarina/policy.json and the files SARINA_POLICY_FILE names) may set anything; the workspace file is
//! repository-controlled and can only tighten (add denials, lower budgets, turn trust off). An operator file that exists
//! but cannot be used is fatal: silently dropping a deny-list is worse than stopping.
//!
//! Go reads a file's keys in map order (random); this reads them sorted. Only which of two keys meaning the same thing
//! wins, and the order of notices, can differ; Go itself gives either (docs/RUST_PORT.md "Map order").

use crate::config::{go_int, BudgetConfig};
use crate::goany::Any;
use crate::goerr;
use crate::gojson;
use crate::gopath;
use crate::gotext;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;

/// Names an extra operator policy file (a list, as PATH).
pub const ENV_FILE: &str = "SARINA_POLICY_FILE";
/// The only name looked for inside the workspace.
pub const WORKSPACE_FILE: &str = ".sarina-policy.json";
const MAX_FILE_BYTES: u64 = 256 << 10;

/// Where to look (Go's `Options`).
pub struct Options<'a> {
    /// for ~/.sarina/policy.json; empty = none
    pub home: String,
    pub getenv: &'a dyn Fn(&str) -> String,
    /// the workspace folder; empty = no workspace file
    pub workspace: String,
    /// the owner's signed list: why an operator file may not be used, "" when it may
    pub trust: Option<&'a dyn Fn(&str) -> String>,
}

/// The merged result (Go's `Policy`). `Policy::new()` is what `load` starts from.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// false: no MCP server is started and no operator agent profile is loaded
    pub trusted: bool,
    /// host variables the operator exports to tool subprocesses (operator files only)
    pub safe_env: Vec<String>,
    pub sources: Vec<String>,
    pub notices: Vec<String>,
    /// an operator file exists but cannot be used: the caller refuses to start
    pub fatal: Vec<String>,
    exact: HashMap<String, String>,
    prefixes: Vec<(String, String)>,
    set: BudgetConfig,
    cap: BudgetConfig,
}

#[derive(Default)]
struct Parsed {
    trusted: Option<bool>,
    deny: Vec<String>,
    prefixes: Vec<String>,
    budget: BudgetConfig,
    safe_env: Vec<String>,
    has_hooks: bool,
    problems: Vec<String>,
    budget_set: bool,
}

fn norm_key(k: &str) -> String {
    gotext::to_lower(k).replace('_', "")
}

/// Go's `parseFile`: a JSON object, its keys compared without case or underscores.
fn parse_file(raw: &[u8]) -> Result<Parsed, String> {
    let (doc, err) = gojson::unmarshal_any(raw);
    if let Some(e) = err {
        return Err(format!("not a JSON object: {e}"));
    }
    let kind = match &doc {
        Any::Object(_) | Any::Null => "",
        Any::Bool(_) => "bool",
        Any::String(_) => "string",
        Any::Array(_) => "array",
        _ => "number",
    };
    if !kind.is_empty() {
        return Err(format!("not a JSON object: json: cannot unmarshal {kind} into Go value of type map[string]interface {{}}"));
    }
    let top = doc.into_object().unwrap_or_default();
    let mut out = Parsed::default();
    for (k, v) in &top {
        match norm_key(k).as_str() {
            "trusted" => match v {
                Any::Bool(b) => out.trusted = Some(*b),
                _ => out.problems.push("trusted must be true or false".into()),
            },
            "denytools" => {
                let items = string_items(v, "denyTools", &mut out.problems);
                out.deny.extend(items);
            }
            "denytoolprefixes" => {
                let items = string_items(v, "denyToolPrefixes", &mut out.problems);
                out.prefixes.extend(items);
            }
            "safeenv" => {
                let items = string_items(v, "safeEnv", &mut out.problems);
                out.safe_env.extend(items);
            }
            "budget" => match v {
                Any::Object(m) => parse_budget(m, &mut out),
                _ => out.problems.push("budget must be an object".into()),
            },
            "hooks" => out.has_hooks = true,
            _ => {}
        }
    }
    Ok(out)
}

fn string_items(v: &Any, what: &str, problems: &mut Vec<String>) -> Vec<String> {
    let items: Vec<&Any> = match v {
        Any::String(_) => vec![v],
        Any::Array(a) => a.iter().collect(),
        _ => {
            problems.push(format!("{what} must be a list of strings"));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for it in items {
        match it {
            Any::String(s) if !gotext::trim_space(s).is_empty() => out.push(gotext::trim_space(s).to_string()),
            _ => problems.push(format!("{what} has an entry that is not a non-empty string")),
        }
    }
    out
}

/// The budget's fields by their canonical key (lower case, no underscores, no "max" prefix).
const BUDGET_FIELDS: [&str; 9] = [
    "totaltokens",
    "inputtokens",
    "outputtokens",
    "reasoningtokens",
    "totalcostusd",
    "toolcalls",
    "delegatedtasks",
    "modelcalls",
    "sessionturns",
];

enum Slot<'a> {
    I(&'a mut Option<i64>),
    F(&'a mut Option<f64>),
}

fn slot(b: &mut BudgetConfig, i: usize) -> Slot<'_> {
    match i {
        0 => Slot::I(&mut b.max_total_tokens),
        1 => Slot::I(&mut b.max_input_tokens),
        2 => Slot::I(&mut b.max_output_tokens),
        3 => Slot::I(&mut b.max_reasoning_tokens),
        4 => Slot::F(&mut b.max_total_cost_usd),
        5 => Slot::I(&mut b.max_tool_calls),
        6 => Slot::I(&mut b.max_delegated_tasks),
        7 => Slot::I(&mut b.max_model_calls),
        _ => Slot::I(&mut b.max_session_turns),
    }
}

fn parse_budget(m: &BTreeMap<String, Any>, out: &mut Parsed) {
    for (k, v) in m {
        let nk = norm_key(k);
        let name = nk.strip_prefix("max").unwrap_or(&nk);
        let Some(field) = BUDGET_FIELDS.iter().position(|f| *f == name) else {
            out.problems.push(format!("budget key {} is not recognised", gotext::quote(k)));
            continue;
        };
        let n = match v {
            Any::Number(n) if n.is_finite() && *n >= 0.0 => *n,
            _ => {
                out.problems.push(format!("budget {} must be a non-negative number", gotext::quote(k)));
                continue;
            }
        };
        out.budget_set = true;
        match slot(&mut out.budget, field) {
            Slot::I(x) => *x = Some(go_int(n)),
            Slot::F(x) => *x = Some(n),
        }
    }
}

/// Every limit set in `src`, copied into `dst`.
fn overlay(dst: &mut BudgetConfig, src: &BudgetConfig) {
    let mut src = src.clone();
    for i in 0..BUDGET_FIELDS.len() {
        match (slot(dst, i), slot(&mut src, i)) {
            (Slot::I(d), Slot::I(s)) if s.is_some() => *d = *s,
            (Slot::F(d), Slot::F(s)) if s.is_some() => *d = *s,
            _ => {}
        }
    }
}

/// A limit copied into `dst` only when it is lower than what `dst` has.
fn lower(dst: &mut BudgetConfig, src: &BudgetConfig) {
    let mut src = src.clone();
    for i in 0..BUDGET_FIELDS.len() {
        match (slot(dst, i), slot(&mut src, i)) {
            (Slot::I(d), Slot::I(Some(s))) if d.is_none_or(|c| *s < c) => *d = Some(*s),
            (Slot::F(d), Slot::F(Some(s))) if d.is_none_or(|c| *s < c) => *d = Some(*s),
            _ => {}
        }
    }
}

/// Go's `filepath.SplitList`.
fn split_list(s: &str) -> Vec<&str> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split(':').collect()
    }
}

/// Go's `Load`: the operator files, then the workspace file (tighten-only).
pub fn load(o: &Options) -> Policy {
    let mut p = Policy::new();
    let mut files: Vec<Vec<u8>> = Vec::new();
    if !o.home.is_empty() {
        files.push(gopath::join(&[o.home.as_bytes(), b".sarina", b"policy.json"]));
    }
    let list = (o.getenv)(ENV_FILE);
    for f in split_list(&list) {
        if !f.is_empty() {
            files.push(f.as_bytes().to_vec());
        }
    }
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    for f in files {
        let abs = gopath::abs(&f).unwrap_or(f);
        if !seen.insert(abs.clone()) {
            continue;
        }
        let abs_s = gotext::lossy(&abs);
        if let Some(trust) = o.trust {
            if std::fs::metadata(gopath::os_path(&abs)).is_ok() {
                let why = trust(&abs_s);
                if !why.is_empty() {
                    p.fatal.push(format!("policy file {abs_s} is {why}"));
                    continue;
                }
            }
        }
        p.load_file(&abs, true);
    }
    if !o.workspace.is_empty() {
        let ws = gopath::join(&[o.workspace.as_bytes(), WORKSPACE_FILE.as_bytes()]);
        if let Ok(abs) = gopath::abs(&ws) {
            if !seen.contains(&abs) {
                p.load_file(&abs, false);
            }
        }
    }
    p.safe_env.sort();
    p
}

enum Opened {
    Missing,
    Failed(String),
    Folder,
    /// what was read (at most one byte over the limit), or the read's error
    Read(Result<Vec<u8>, String>),
}

/// Go's `os.Open`, `Stat` and `io.ReadAll(io.LimitReader(f, max+1))`, with Go's error texts.
fn open_limited(path: &[u8]) -> Opened {
    let f = match std::fs::File::open(gopath::os_path(path)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Opened::Missing,
        Err(e) => return Opened::Failed(goerr::path("open", path, &e)),
    };
    if f.metadata().is_ok_and(|m| m.is_dir()) {
        return Opened::Folder;
    }
    let mut raw = Vec::new();
    match f.take(MAX_FILE_BYTES + 1).read_to_end(&mut raw) {
        Ok(_) => Opened::Read(Ok(raw)),
        Err(e) => Opened::Read(Err(goerr::path("read", path, &e))),
    }
}

impl Policy {
    /// Nothing read yet: trusted, no rules, no limits.
    pub fn new() -> Policy {
        Policy { trusted: true, ..Policy::default() }
    }

    fn load_file(&mut self, path: &[u8], operator: bool) {
        let path_s = gotext::lossy(path);
        let read = match open_limited(path) {
            Opened::Missing | Opened::Folder => return,
            Opened::Failed(e) => {
                if operator {
                    self.fatal.push(format!("cannot read policy file {path_s}: {e}"));
                }
                return;
            }
            Opened::Read(r) => r,
        };
        self.apply_file(&path_s, read, operator);
    }

    /// A policy file's content (or the error reading it gave) merged in, as Go's `loadFile` does after opening it.
    pub fn apply_file(&mut self, path_s: &str, read: Result<Vec<u8>, String>, operator: bool) {
        let parsed = read.and_then(|raw| {
            if raw.len() as u64 > MAX_FILE_BYTES {
                Err(format!("larger than {MAX_FILE_BYTES} bytes"))
            } else {
                parse_file(&raw)
            }
        });
        let pf = match parsed {
            Ok(pf) => pf,
            Err(e) => {
                if operator {
                    self.fatal.push(format!("policy file {path_s} is unusable ({e}); fix or remove it"));
                } else {
                    self.notices.push(format!("workspace policy file {path_s} ignored: {e}"));
                }
                return;
            }
        };
        self.sources.push(path_s.to_string());
        for pr in &pf.problems {
            self.notices.push(format!("policy file {path_s}: {pr}"));
        }
        if pf.has_hooks {
            self.notices.push(format!("policy file {path_s}: hooks are not supported and were ignored"));
        }
        for n in &pf.deny {
            self.exact.entry(gotext::to_lower(n)).or_insert_with(|| path_s.to_string());
        }
        for pre in &pf.prefixes {
            self.prefixes.push((gotext::to_lower(pre), path_s.to_string()));
        }
        if operator {
            if let Some(t) = pf.trusted {
                self.trusted = t;
            }
            self.safe_env.extend(pf.safe_env.iter().cloned());
            overlay(&mut self.set, &pf.budget);
            return;
        }
        // the workspace file: tighten only
        match pf.trusted {
            Some(true) => {
                self.notices.push(format!("workspace policy file {path_s}: trusted=true ignored (a workspace file can only tighten)"))
            }
            Some(false) => self.trusted = false,
            None => {}
        }
        if !pf.safe_env.is_empty() {
            self.notices
                .push(format!("workspace policy file {path_s}: safeEnv ignored (only operator files may export environment variables)"));
        }
        lower(&mut self.cap, &pf.budget);
        let n = pf.deny.len() + pf.prefixes.len();
        if n > 0 || pf.budget_set || pf.trusted == Some(false) {
            self.notices.push(format!(
                "workspace policy file {path_s} tightens this session ({n} denial rule(s), budget limits: {})",
                pf.budget_set
            ));
        }
    }

    /// Go's `ApplyBudget`: operator limits replace the value, then workspace limits lower it.
    pub fn apply_budget(&self, b: &mut BudgetConfig) {
        overlay(b, &self.set);
        lower(b, &self.cap);
    }

    /// Why a tool may not be used, or "" when it may (case-insensitive).
    pub fn denied(&self, tool: &str) -> String {
        let key = gotext::to_lower(tool);
        if let Some(src) = self.exact.get(&key) {
            return format!("tool {tool} is denied by the operator policy ({src})");
        }
        for (prefix, source) in &self.prefixes {
            if key.starts_with(prefix.as_str()) {
                return format!("tool {tool} is denied by the operator policy (prefix {} in {source})", gotext::quote(prefix));
            }
        }
        String::new()
    }

    /// The number of exact and prefix rules.
    pub fn deny_rules(&self) -> usize {
        self.exact.len() + self.prefixes.len()
    }
}
