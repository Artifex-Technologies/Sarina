//! Go: `go/mcp/config.go`: where MCP server definitions come from, under the trust gate: operator files only
//! (~/.sarina/mcp.json and SARINA_MCP_FILE's), the workspace's (in that folder only, never an ancestor) only with
//! --trust-workspace-mcp and a visible notice, each file read to at most 256 KiB, the owner's signed list first, every
//! problem recorded (testdata/parity/mcpmanager.json).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use super::client::ServerDef;
use crate::goany::Any;
use crate::gostruct::{self, Ty, Val};
use crate::gotext::{self, trim_space};
use crate::{goerr, gopath};

pub const ENV_FILE: &str = "SARINA_MCP_FILE";
pub const WORKSPACE_FILES: &[&str] = &[".sarina-mcp.json", ".mcp.json", "mcp.json"];
const MAX_CONFIG_BYTES: u64 = 256 << 10;
pub const SOURCE_OPERATOR: &str = "operator";
pub const SOURCE_WORKSPACE: &str = "workspace";

/// Refuses a file the owner did not approve: the reason, or "".
pub type Trust = Arc<dyn Fn(&str) -> String + Send + Sync>;
/// Reads an environment variable.
pub type Getenv = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Where definitions may come from (Go's `LoadOptions`).
#[derive(Clone, Default)]
pub struct LoadOptions {
    pub home: String,
    /// None: this process's environment
    pub getenv: Option<Getenv>,
    pub workspace: String,
    pub trust_workspace: bool,
    pub trust: Option<Trust>,
}

/// What loading found (Go's `Config`).
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub servers: Vec<ServerDef>,
    pub notices: Vec<String>,
    pub problems: Vec<String>,
    /// every file read
    pub files: Vec<String>,
}

static MAP_ANY: Ty = Ty::Map(&Ty::Any);

/// Go's `Load`.
pub fn load(o: &LoadOptions) -> Config {
    let getenv = |k: &str| match &o.getenv {
        Some(f) => f(k),
        None => std::env::var(k).unwrap_or_default(),
    };
    let mut c = Config::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut by_name: HashMap<String, String> = HashMap::new();
    let mut read = |c: &mut Config, path: &str, source: &str| {
        let abs = gopath::abs(path.as_bytes()).map(|a| gotext::lossy(&a)).unwrap_or_else(|_| path.to_owned());
        if !seen.insert(abs.clone()) {
            return;
        }
        if let (Some(tr), SOURCE_OPERATOR) = (&o.trust, source) {
            if std::fs::metadata(&abs).is_ok() {
                let why = tr(&abs);
                if !why.is_empty() {
                    c.notices.push(format!("tool connections in {abs} not used: {why}"));
                    return;
                }
            }
        }
        load_file(c, &abs, source, &mut by_name);
    };
    if !o.home.is_empty() {
        read(&mut c, &format!("{}/.sarina/mcp.json", o.home), SOURCE_OPERATOR);
    }
    let env = getenv(ENV_FILE);
    if !env.is_empty() {
        for f in env.split(':').filter(|f| !f.is_empty()) {
            read(&mut c, f, SOURCE_OPERATOR);
        }
    }
    if !o.workspace.is_empty() {
        for name in WORKSPACE_FILES {
            let p = gotext::lossy(&gopath::join(&[o.workspace.as_bytes(), name.as_bytes()]));
            match std::fs::symlink_metadata(&p) {
                Ok(m) if !m.is_dir() => {}
                _ => continue,
            }
            if !o.trust_workspace {
                c.notices.push(format!(
                    "ignored MCP definition file {p}: a workspace file can start programs, so it is only used with --trust-workspace-mcp"
                ));
                continue;
            }
            c.notices.push(format!("workspace MCP definitions are enabled (--trust-workspace-mcp): {p} may start programs"));
            read(&mut c, &p, SOURCE_WORKSPACE);
        }
    }
    c
}

fn load_file(c: &mut Config, path: &str, source: &str, by_name: &mut HashMap<String, String>) {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if e.raw_os_error() != Some(2) {
                c.problems.push(format!("{path}: {}", goerr::path("open", path.as_bytes(), &e)));
            }
            return;
        }
    };
    let mut raw = Vec::new();
    if f.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut raw).is_err() || raw.len() as u64 > MAX_CONFIG_BYTES {
        c.problems.push(format!("{path}: unreadable or larger than {MAX_CONFIG_BYTES} bytes"));
        return;
    }
    let (top, err) = gostruct::unmarshal(&MAP_ANY, &raw);
    if let Some(e) = err {
        c.problems.push(format!("{path}: not a JSON object: {e}"));
        return;
    }
    c.files.push(path.to_owned());
    let dir = gotext::lossy(&gopath::dir(path.as_bytes()));
    let top: BTreeMap<String, Any> = match top {
        Val::Map(Some(m)) => m
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    match v {
                        Val::Any(a) => a,
                        _ => Any::Null,
                    },
                )
            })
            .collect(),
        _ => BTreeMap::new(),
    };
    let mut raws: Vec<BTreeMap<String, Any>> = Vec::new();
    if let Some(Any::Object(m)) = top.get("mcpServers") {
        for (n, v) in m {
            if let Any::Object(sm) = v {
                let mut sm = sm.clone();
                sm.insert("name".into(), Any::String(n.clone()));
                raws.push(sm);
            }
        }
    }
    if let Some(Any::Array(l)) = top.get("servers") {
        for it in l {
            if let Any::Object(sm) = it {
                raws.push(sm.clone());
            }
        }
    }
    for r in raws {
        let mut def = match parse_server(&r, &dir) {
            Ok(d) => d,
            Err(why) => {
                c.problems.push(format!("{path}: {why}"));
                continue;
            }
        };
        def.source = source.to_owned();
        def.file = path.to_owned();
        let key = def.name.to_lowercase();
        if let Some(first) = by_name.get(&key) {
            c.problems.push(format!("{path}: server {} is already defined in {first}; this one is ignored", gotext::quote(&def.name)));
            continue;
        }
        by_name.insert(key, path.to_owned());
        c.servers.push(def);
    }
}

fn str_of<'a>(m: &'a BTreeMap<String, Any>, k: &str) -> Option<&'a str> {
    match m.get(k) {
        Some(Any::String(s)) => Some(s),
        _ => None,
    }
}

/// Go's `parseServer`.
fn parse_server(m: &BTreeMap<String, Any>, base_dir: &str) -> Result<ServerDef, String> {
    let mut d = ServerDef { name: trim_space(str_of(m, "name").unwrap_or("")).to_owned(), ..ServerDef::default() };
    if d.name.is_empty() {
        return Err("a server has no name".into());
    }
    if let Some(t) = str_of(m, "transport") {
        if !t.is_empty() && !gotext::equal_fold(t, "stdio") {
            return Err(format!("server {} uses transport {}; only stdio is supported", gotext::quote(&d.name), gotext::quote(t)));
        }
    }
    d.command = str_of(m, "command").unwrap_or("").to_owned();
    if trim_space(&d.command).is_empty() {
        return Err(format!("server {} has no command", gotext::quote(&d.name)));
    }
    if let Some(Any::Array(args)) = m.get("args") {
        d.args = args
            .iter()
            .filter_map(|a| match a {
                Any::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
    }
    if let Some(Any::Object(env)) = m.get("env") {
        for (k, v) in env {
            if let (Any::String(s), false) = (v, k.is_empty()) {
                d.env.insert(k.clone(), s.clone());
            }
        }
    }
    if let Some(cwd) = str_of(m, "cwd").filter(|c| !c.is_empty()) {
        let mut cwd = cwd.to_owned();
        if let Some(rest) = cwd.strip_prefix('~') {
            let h = std::env::var("HOME").unwrap_or_default();
            if !h.is_empty() {
                cwd = gotext::lossy(&gopath::join(&[h.as_bytes(), rest.as_bytes()]));
            }
        }
        if !cwd.starts_with('/') {
            cwd = gotext::lossy(&gopath::join(&[base_dir.as_bytes(), cwd.as_bytes()]));
        }
        d.cwd = cwd;
    }
    d.description = str_of(m, "description").unwrap_or("").to_owned();
    if let Some(Any::Bool(b)) = m.get("read_only") {
        d.read_only = *b;
    }
    if let Some(Any::Array(l)) = m.get("read_only_tools") {
        for it in l {
            if let Any::String(s) = it {
                d.read_only_tools.insert(s.clone(), true);
            }
        }
    }
    if let Some(Any::Object(tm)) = m.get("tools") {
        for (name, v) in tm {
            if let Any::Object(o) = v {
                if o.get("read_only") == Some(&Any::Bool(true)) {
                    d.read_only_tools.insert(name.clone(), true);
                }
            }
        }
    }
    if let Some(Any::Number(n)) = m.get("timeout_seconds") {
        if *n > 0.0 {
            d.timeout_seconds = *n;
        }
    }
    Ok(d)
}
