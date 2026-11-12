//! Go: `go/skills/discover.go` and `scan.go`: where skills are looked for (operator folders first; the workspace's only
//! with the opt-in, and last, so it can never shadow an operator skill), what is loaded from each (name.md and
//! name/SKILL.md, links never followed, the shared scan gate and the owner's signed list before anything is used), and
//! what could not be used (testdata/parity/skills.json).

use std::collections::HashMap;
use std::sync::Arc;

use super::library::{read_capped, section_files, split_sections, HEADING_RE};
use super::parse::{parse, Skill};
use crate::gopath;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext::{self, trim_space};

pub const SOURCE_OPERATOR: &str = "operator";
pub const SOURCE_WORKSPACE: &str = "workspace";
pub const ENV_DIR: &str = "SARINA_SKILLS_DIR";
pub const DEFAULT_SYSTEM_DIR: &str = "/usr/share/sarina/skills";
/// The most of a skill that goes into the prompt; a longer one is split into sections.
pub const MAX_SKILL_BYTES: usize = 64 << 10;

/// Vets text before it can be used: (blocked, reason). The default is the shared gate's local checks.
pub type Scanner = Arc<dyn Fn(&[u8]) -> (bool, String) + Send + Sync>;
/// Refuses a file the owner did not approve: the reason, or "" when it may be used.
pub type Trust = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// The shared gate without the Heimdall call (Go's BasicScanner).
pub fn basic_scanner() -> Scanner {
    Arc::new(|text: &[u8]| {
        let v = crate::guard::Gate::default_gate().scan_local(text);
        (v.blocked, v.reason)
    })
}

/// Reads an environment variable.
pub type Getenv = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Where to look (Go's `Options`).
#[derive(Clone, Default)]
pub struct Options {
    pub home: String,
    /// None: this process's environment
    pub getenv: Option<Getenv>,
    pub workspace: String,
    pub trust_workspace: bool,
    pub scanner: Option<Scanner>,
    /// "" is the system folder (Linux)
    pub system_dir: String,
    pub trust: Option<Trust>,
}

/// One folder searched.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub dir: String,
    pub source: String,
}

/// A skill file that could not be used, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

/// Two skills of the same name: the earlier is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Collision {
    pub name: String,
    pub kept: String,
    pub shadowed: String,
}

/// What discovery found.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub skills: Vec<Skill>,
    pub problems: Vec<Problem>,
    pub collisions: Vec<Collision>,
    pub notices: Vec<String>,
}

static PATHS: Ty = Ty::Slice(&Ty::Str);
static SKILLS_JSON_FIELDS: [Field; 1] = [Field("paths", &PATHS)];
static SKILLS_JSON: Ty = Ty::Struct("struct { Paths []string }", "", &SKILLS_JSON_FIELDS);

/// Go's `Roots`: ~/.sarina/skills, SARINA_SKILLS_DIR's folders, ~/.sarina/skills.json's paths, the system folder, and with
/// the opt-in <workspace>/.sarina/skills last.
pub fn roots(o: &Options) -> Vec<Root> {
    let env_of = |k: &str| match &o.getenv {
        Some(f) => f(k),
        None => std::env::var(k).unwrap_or_default(),
    };
    let mut out: Vec<Root> = Vec::new();
    let mut add = |dir: &str, source: &str| {
        if dir.is_empty() {
            return;
        }
        let abs = gopath::abs(dir.as_bytes()).unwrap_or_else(|_| dir.as_bytes().to_vec());
        let key = gotext::lossy(&gopath::clean(&abs));
        if !out.iter().any(|r| r.dir == key) {
            out.push(Root { dir: key, source: source.to_owned() });
        }
    };
    if !o.home.is_empty() {
        add(&format!("{}/.sarina/skills", o.home), SOURCE_OPERATOR);
    }
    let env = env_of(ENV_DIR);
    if !env.is_empty() {
        for d in env.split(':') {
            add(d, SOURCE_OPERATOR);
        }
    }
    if !o.home.is_empty() {
        let sj = format!("{}/.sarina/skills.json", o.home);
        if let Ok(raw) = read_capped(&sj) {
            if o.trust.as_ref().is_none_or(|t| t(&sj).is_empty()) {
                let (v, err) = gostruct::unmarshal(&SKILLS_JSON, &raw);
                if err.is_none() {
                    if let Val::Struct(f) = v {
                        if let Val::Slice(Some(paths)) = &f[0] {
                            for p in paths {
                                if let Val::Str(p) = p {
                                    add(p, SOURCE_OPERATOR);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let sys = if o.system_dir.is_empty() { DEFAULT_SYSTEM_DIR.to_owned() } else { o.system_dir.clone() };
    add(&sys, SOURCE_OPERATOR);
    if o.trust_workspace && !o.workspace.is_empty() {
        add(&format!("{}/.sarina/skills", o.workspace), SOURCE_WORKSPACE);
    }
    out
}

/// Go's `Discover`: every usable skill; failures recorded, never fatal.
pub fn discover(o: &Options) -> Catalog {
    let sc = o.scanner.clone().unwrap_or_else(basic_scanner);
    let mut c = Catalog::default();
    let mut by_name: HashMap<String, String> = HashMap::new();
    for root in roots(o) {
        if root.source == SOURCE_WORKSPACE {
            c.notices.push(format!("workspace skills are enabled (--trust-workspace-skills): loading skills from {}", root.dir));
        }
        for (path, fallback) in candidates(&root.dir, &mut c) {
            if let Some(tr) = &o.trust {
                let why = tr(&path);
                if !why.is_empty() {
                    c.problems.push(Problem { path, message: why });
                    continue;
                }
            }
            let (s, probs) = load(&path, &fallback, &sc, o.trust.as_ref());
            c.problems.extend(probs);
            let mut s = match s {
                Ok(s) => s,
                Err(e) => {
                    c.problems.push(Problem { path, message: e });
                    continue;
                }
            };
            s.source = root.source.clone();
            let key = s.name.to_lowercase();
            if let Some(kept) = by_name.get(&key) {
                c.collisions.push(Collision { name: s.name.clone(), kept: kept.clone(), shadowed: s.path.clone() });
                continue;
            }
            by_name.insert(key, s.path.clone());
            c.skills.push(s);
        }
    }
    c
}

fn ext_of(name: &str) -> &str {
    name.rfind('.').map_or("", |i| &name[i..])
}

/// Go's `candidates`: name.md files and name/SKILL.md in one folder; links never followed.
fn candidates(dir: &str, c: &mut Catalog) -> Vec<(String, String)> {
    let Ok(entries) = gopath::read_dir(dir.as_bytes()) else { return Vec::new() };
    let mut out = Vec::new();
    for e in &entries {
        let name = gotext::lossy(&e.name);
        let full = format!("{dir}/{name}");
        if name.starts_with('.') {
            continue;
        }
        match e.kind {
            gopath::Kind::Symlink => c.problems.push(Problem { path: full, message: "symbolic links are not followed".into() }),
            gopath::Kind::Dir => {
                let p = format!("{full}/SKILL.md");
                if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file()) {
                    out.push((p, name));
                }
            }
            gopath::Kind::Regular if ext_of(&name).eq_ignore_ascii_case(".md") && !name.eq_ignore_ascii_case("README.md") => {
                let stem = name[..name.len() - ext_of(&name).len()].to_owned();
                out.push((full, stem));
            }
            _ => {}
        }
    }
    out
}

fn base_is_skill_md(path: &str) -> bool {
    gotext::lossy(&gopath::base(path.as_bytes())).eq_ignore_ascii_case("SKILL.md")
}

/// Go's `load`.
fn load(path: &str, fallback: &str, sc: &Scanner, tr: Option<&Trust>) -> (Result<Skill, String>, Vec<Problem>) {
    let data = match read_capped(path) {
        Ok(d) => d,
        Err(e) => return (Err(e), Vec::new()),
    };
    let (blocked, reason) = sc(&data);
    if blocked {
        return (Err(format!("blocked by the security scan: {reason}")), Vec::new());
    }
    let mut res = parse(&data, fallback);
    if base_is_skill_md(path) && res.as_ref().is_err_and(|e| e.starts_with("no front matter")) {
        // a skill folder's SKILL.md written without the header: the folder names it, its first heading describes it
        let text = gotext::lossy(&data);
        let mut with = format!("---\ndescription: {}\n---\n", headerless(&text)).into_bytes();
        with.extend_from_slice(&data);
        res = parse(&with, fallback);
    }
    let mut s = match res {
        Ok(s) => s,
        Err(e) => return (Err(e), Vec::new()),
    };
    s.path = path.to_owned();
    let mut probs = Vec::new();
    if base_is_skill_md(path) {
        let dir = gotext::lossy(&gopath::dir(path.as_bytes()));
        let (secs, p) = section_files(&dir, sc, tr);
        probs = p;
        if !secs.is_empty() {
            if s.body.len() > MAX_SKILL_BYTES {
                let (intro, mut more) = split_sections(&s.body);
                more.extend(secs);
                s.make_library(&intro, more, false);
            } else {
                let body = s.body.clone();
                s.make_library(&body, secs, true);
            }
            return (Ok(s), probs);
        }
    }
    if s.body.len() > MAX_SKILL_BYTES {
        let (intro, secs) = split_sections(&s.body);
        s.make_library(&intro, secs, false);
    }
    (Ok(s), probs)
}

/// Go's `headerless`: a skill without front matter described by its first heading, else its first line.
pub fn headerless(text: &str) -> String {
    let mut desc = String::new();
    if let Some(m) = HEADING_RE.captures(text) {
        desc = m[2].to_owned();
    } else if let Some(l) = text.split('\n').map(trim_space).find(|t| !t.is_empty()) {
        desc = l.to_owned();
    }
    let desc = trim_space(&desc).replace(['"', '\'', '#'], "").replace(':', " -");
    let mut desc: String = desc.chars().take(120).collect();
    if desc.is_empty() {
        desc = "(no description)".into();
    }
    desc
}

impl Catalog {
    /// A skill by name, ignoring case (Go's strings.EqualFold).
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| equal_fold(&s.name, name))
    }

    /// The skills the model may call.
    pub fn model_invocable(&self) -> Vec<&Skill> {
        self.skills.iter().filter(|s| s.model_invocable).collect()
    }
}

/// Go's `strings.EqualFold` (Unicode simple folding).
pub fn equal_fold(a: &str, b: &str) -> bool {
    let fold = |c: char| -> char {
        match c {
            '\u{17F}' => 's',
            '\u{212A}' => 'k',
            c => {
                let mut l = c.to_lowercase();
                match (l.next(), l.next()) {
                    (Some(x), None) => x,
                    _ => c,
                }
            }
        }
    };
    let (mut x, mut y) = (a.chars(), b.chars());
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) if fold(p) == fold(q) => {}
            _ => return false,
        }
    }
}
