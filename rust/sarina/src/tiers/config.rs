//! Go: `tiers/config.go` and `escalation.go`: the operator's tier setup (`~/.sarina/tiers.json`, or the file named by
//! SARINA_TIERS_FILE; never a workspace's), read as Go 1.27 reads it into its structs, and the original escalation
//! settings folded in. A broken file narrows behaviour (one local tier), never widens it.

use crate::config::EscalationConfig;
use crate::goany::Any;
use crate::goerr;
use crate::gopath;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext;
use std::io::Read;

pub const KIND_OPENAI_COMPAT: &str = "openai_compat";
pub const KIND_CLAUDE: &str = "claude";
/// Hands the task to the local `claude` executable (print mode); opt-in.
pub const KIND_CLAUDE_CLI: &str = "claude_cli";

/// Credential-shaped material is redacted before sending.
pub const CLASS_NO_SECRETS: &str = "no_secrets";
/// RF/NFC capture dumps are never sent (the tier is refused).
pub const CLASS_NO_CAPTURES: &str = "no_captures";

/// Every lower tier was unreachable or unusable.
pub const TRIGGER_BACKEND_ERROR: &str = "backend_error";
/// The first tier is under resource pressure.
pub const TRIGGER_RESOURCE_EXHAUSTED: &str = "resource_exhausted";

pub const DEFAULT_CLI_PATH: &str = "claude";
pub const DEFAULT_CLI_TIMEOUT: f64 = 300.0;
pub const DEFAULT_MAX_ESCALATION: i64 = 1;

/// What may be sent to a tier: `all` is no restriction; otherwise each listed class is enforced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AllowData {
    pub all: bool,
    pub classes: Vec<String>,
}

impl AllowData {
    pub fn all() -> AllowData {
        AllowData { all: true, classes: Vec::new() }
    }

    pub fn of(classes: &[&str]) -> AllowData {
        AllowData { all: false, classes: classes.iter().map(|c| c.to_string()).collect() }
    }

    /// Whether the class restriction is in force.
    pub fn has(&self, class: &str) -> bool {
        !self.all && self.classes.iter().any(|c| c == class)
    }

    /// Go's `String`, for status output.
    pub fn string(&self) -> String {
        if self.all {
            "all".into()
        } else if self.classes.is_empty() {
            "(nothing restricted)".into()
        } else {
            self.classes.join(",")
        }
    }

    /// Go's `MarshalJSON`: "all" or the class list.
    pub fn to_json(&self) -> String {
        if self.all {
            return r#""all""#.into();
        }
        let list = Any::Array(self.classes.iter().map(|c| Any::String(c.clone())).collect());
        crate::gojson::marshal_any(&list).unwrap_or_default()
    }
}

/// Go's `parseAllow` on the value read for allow_data (null or absent: the default).
pub fn parse_allow(raw: &Any, def: &AllowData) -> Result<(AllowData, Vec<String>), String> {
    const BAD: &str = r#"allow_data must be "all" or a list of classes"#;
    match raw {
        Any::Null => Ok((def.clone(), Vec::new())),
        Any::String(one) => {
            if gotext::equal_fold(gotext::trim_space(one), "all") {
                Ok((AllowData::all(), Vec::new()))
            } else {
                Err(format!(r#"allow_data must be "all" or a list of classes, got {}"#, gotext::quote(one)))
            }
        }
        Any::Array(list) => {
            // json.Unmarshal into []string: a null element is "", anything else but a string fails the whole list
            let mut names = Vec::new();
            for v in list {
                match v {
                    Any::String(s) => names.push(s.clone()),
                    Any::Null => names.push(String::new()),
                    _ => return Err(BAD.into()),
                }
            }
            let mut out = AllowData::default();
            let mut warns = Vec::new();
            for c in names {
                let c = gotext::to_lower(gotext::trim_space(&c));
                match c.as_str() {
                    "all" => return Ok((AllowData::all(), Vec::new())),
                    CLASS_NO_SECRETS | CLASS_NO_CAPTURES => out.classes.push(c),
                    _ => warns.push(format!("unknown allow_data class {} ignored", gotext::quote(&c))),
                }
            }
            Ok((out, warns))
        }
        _ => Err(BAD.into()),
    }
}

/// One compute tier.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tier {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    /// the NAME of an environment variable; the key itself is never stored
    pub api_key_env: String,
    pub allow_data: AllowData,
    /// tokens; 0 = unknown
    pub max_context: i64,
    pub wake: String,
    /// seconds to wait for the tier to come up after a wake
    pub wake_wait: f64,
    pub enabled: bool,
    /// false: ask the operator once per session before first use
    pub auto: bool,
    // claude_cli only
    pub cli_path: String,
    pub timeout_seconds: f64,
    pub max_escalations: i64,
    pub triggers: Option<Vec<String>>,
    pub extra_args: Option<Vec<String>>,
    pub extra_dirs: Option<Vec<String>>,
    pub accept_edits: bool,
}

impl Tier {
    /// Go's `Command`: `-p <prompt> --output-format json --cwd <dir> --bare`, then `--add-dir` per extra folder, the
    /// permission flag, the extra arguments. Never a bypass-permissions mode.
    pub fn command(&self, prompt: &str, cwd: &str) -> Vec<String> {
        let mut args: Vec<String> =
            ["-p", prompt, "--output-format", "json", "--cwd", cwd, "--bare"].iter().map(|s| s.to_string()).collect();
        for d in self.extra_dirs.iter().flatten() {
            args.push("--add-dir".into());
            args.push(d.clone());
        }
        if self.accept_edits {
            args.push("--permission-mode".into());
            args.push("acceptEdits".into());
        }
        args.extend(self.extra_args.iter().flatten().cloned());
        args
    }
}

/// The resource-pressure limits on the first tier's device, in percent; zero disables a check.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Thresholds {
    pub max_memory_percent: f64,
    pub max_cpu_percent: f64,
    pub max_gpu_memory_percent: f64,
}

/// The whole tier setup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub tiers: Vec<Tier>,
    pub resource: Thresholds,
    pub warnings: Option<Vec<String>>,
    /// where it was read from; empty for the default
    pub path: String,
}

impl Config {
    /// Whether any escalation is possible.
    pub fn multi(&self) -> bool {
        self.tiers.len() > 1
    }

    fn warn(&mut self, w: String) {
        self.warnings.get_or_insert_with(Vec::new).push(w);
    }
}

fn default_thresholds() -> Thresholds {
    Thresholds { max_memory_percent: 90.0, max_cpu_percent: 0.0, max_gpu_memory_percent: 90.0 }
}

/// The no-file configuration: nothing but the local backend.
pub fn default() -> Config {
    Config {
        tiers: vec![Tier {
            name: "local".into(),
            kind: KIND_OPENAI_COMPAT.into(),
            allow_data: AllowData::all(),
            enabled: true,
            auto: true,
            ..Tier::default()
        }],
        resource: default_thresholds(),
        warnings: None,
        path: String::new(),
    }
}

static STRS: Ty = Ty::Slice(&Ty::Str);
static TIER_FILE: Ty = Ty::Struct(
    "tiers.tierFile",
    "tierFile",
    &[
        Field("name", &Ty::Str),
        Field("kind", &Ty::Str),
        Field("base_url", &Ty::Str),
        Field("model", &Ty::Str),
        Field("api_key_env", &Ty::Str),
        Field("allow_data", &Ty::Any),
        Field("max_context", &Ty::Int),
        Field("wake", &Ty::Str),
        Field("wake_wait_seconds", &Ty::Float),
        Field("enabled", &Ty::Ptr(&Ty::Bool)),
        Field("auto", &Ty::Ptr(&Ty::Bool)),
        Field("cli_path", &Ty::Str),
        Field("timeout_seconds", &Ty::Float),
        Field("max_escalations", &Ty::Ptr(&Ty::Int)),
        Field("trigger_stop_reasons", &STRS),
        Field("extra_cli_args", &STRS),
        Field("add_dirs", &STRS),
        Field("accept_edits", &Ty::Bool),
    ],
);
static RESOURCE: Ty = Ty::Struct(
    r#"struct { MaxMemoryPercent *float64 "json:\"max_memory_percent\""; MaxCPUPercent *float64 "json:\"max_cpu_percent\""; MaxGPUMemoryPercent *float64 "json:\"max_gpu_memory_percent\"" }"#,
    "",
    &[
        Field("max_memory_percent", &Ty::Ptr(&Ty::Float)),
        Field("max_cpu_percent", &Ty::Ptr(&Ty::Float)),
        Field("max_gpu_memory_percent", &Ty::Ptr(&Ty::Float)),
    ],
);
static CONFIG_FILE: Ty =
    Ty::Struct("tiers.configFile", "configFile", &[Field("tiers", &Ty::Slice(&TIER_FILE)), Field("resource", &Ty::Ptr(&RESOURCE))]);

fn f_str(t: &Ty, v: &Val, name: &str) -> String {
    gostruct::str_field(t, v, name).to_string()
}

fn f_int(t: &Ty, v: &Val, name: &str) -> i64 {
    match gostruct::field(t, v, name) {
        Some(Val::Int(n)) => *n,
        _ => 0,
    }
}

fn f_float(t: &Ty, v: &Val, name: &str) -> f64 {
    match gostruct::field(t, v, name) {
        Some(Val::Float(n)) => *n,
        _ => 0.0,
    }
}

fn f_ptr<'a>(t: &Ty, v: &'a Val, name: &str) -> Option<&'a Val> {
    match gostruct::field(t, v, name) {
        Some(Val::Ptr(Some(b))) => Some(b),
        _ => None,
    }
}

fn f_strs(t: &Ty, v: &Val, name: &str) -> Option<Vec<String>> {
    match gostruct::field(t, v, name) {
        Some(Val::Slice(Some(items))) => Some(
            items
                .iter()
                .map(|i| match i {
                    Val::Str(s) => s.clone(),
                    _ => String::new(),
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Go's `Parse`: validates a tiers.json document.
pub fn parse(data: &[u8]) -> Result<Config, String> {
    let (f, err) = gostruct::unmarshal(&CONFIG_FILE, data);
    if let Some(e) = err {
        return Err(format!("invalid JSON: {e}"));
    }
    let mut cfg = Config { resource: default_thresholds(), ..Config::default() };
    if let Some(r) = f_ptr(&CONFIG_FILE, &f, "resource") {
        let get = |name| match f_ptr(&RESOURCE, r, name) {
            Some(Val::Float(x)) => Some(*x),
            _ => None,
        };
        if let Some(x) = get("max_memory_percent") {
            cfg.resource.max_memory_percent = x;
        }
        if let Some(x) = get("max_cpu_percent") {
            cfg.resource.max_cpu_percent = x;
        }
        if let Some(x) = get("max_gpu_memory_percent") {
            cfg.resource.max_gpu_memory_percent = x;
        }
    }
    let tfs = match gostruct::field(&CONFIG_FILE, &f, "tiers") {
        Some(Val::Slice(Some(t))) if !t.is_empty() => t,
        _ => return Ok(default()),
    };
    let mut seen = std::collections::HashSet::new();
    let ty = &TIER_FILE;
    for (i, tf) in tfs.iter().enumerate() {
        let trim = |name: &str| gotext::trim_space(gostruct::str_field(ty, tf, name)).to_string();
        let mut t = Tier {
            name: trim("name"),
            kind: trim("kind"),
            base_url: trim("base_url"),
            model: trim("model"),
            api_key_env: trim("api_key_env"),
            max_context: f_int(ty, tf, "max_context"),
            wake: trim("wake"),
            wake_wait: f_float(ty, tf, "wake_wait_seconds"),
            enabled: true,
            auto: true,
            ..Tier::default()
        };
        if t.kind.is_empty() {
            t.kind = KIND_OPENAI_COMPAT.into();
        }
        if t.kind != KIND_OPENAI_COMPAT && t.kind != KIND_CLAUDE && t.kind != KIND_CLAUDE_CLI {
            return Err(format!(
                "tier {}: unknown kind {} (use {}, {} or {})",
                i + 1,
                gotext::quote(&t.kind),
                gotext::quote(KIND_OPENAI_COMPAT),
                gotext::quote(KIND_CLAUDE),
                gotext::quote(KIND_CLAUDE_CLI)
            ));
        }
        if t.name.is_empty() {
            t.name = format!("tier{}", i + 1);
        }
        let q = gotext::quote(&t.name);
        if !seen.insert(gotext::to_lower(&t.name)) {
            return Err(format!("duplicate tier name {q}"));
        }
        if let Some(Val::Bool(b)) = f_ptr(ty, tf, "enabled") {
            t.enabled = *b;
        }
        if let Some(Val::Bool(b)) = f_ptr(ty, tf, "auto") {
            t.auto = *b;
        }
        if t.max_context < 0 {
            return Err(format!("tier {q}: max_context must not be negative"));
        }
        match t.kind.as_str() {
            KIND_CLAUDE => {
                if t.model.is_empty() {
                    return Err(format!("tier {q}: a claude tier needs a model"));
                }
                if t.api_key_env.is_empty() {
                    return Err(format!(
                        "tier {q}: a claude tier needs api_key_env (the NAME of the environment variable holding the key)"
                    ));
                }
            }
            KIND_CLAUDE_CLI => {
                if i == 0 {
                    return Err(format!("tier {q}: a claude_cli tier is a fallback and cannot be the first tier"));
                }
                t.cli_path = f_str(ty, tf, "cli_path");
                t.timeout_seconds = f_float(ty, tf, "timeout_seconds");
                if t.cli_path.is_empty() {
                    t.cli_path = DEFAULT_CLI_PATH.into();
                }
                if t.timeout_seconds < 0.0 {
                    return Err(format!("tier {q}: timeout_seconds must not be negative"));
                }
                if t.timeout_seconds == 0.0 {
                    t.timeout_seconds = DEFAULT_CLI_TIMEOUT;
                }
                t.max_escalations = DEFAULT_MAX_ESCALATION;
                if let Some(Val::Int(n)) = f_ptr(ty, tf, "max_escalations") {
                    t.max_escalations = *n;
                }
                t.triggers = f_strs(ty, tf, "trigger_stop_reasons");
                if t.triggers.is_none() {
                    t.triggers = Some(vec![TRIGGER_BACKEND_ERROR.into(), TRIGGER_RESOURCE_EXHAUSTED.into()]);
                }
                for tr in t.triggers.clone().into_iter().flatten() {
                    if tr != TRIGGER_BACKEND_ERROR && tr != TRIGGER_RESOURCE_EXHAUSTED {
                        cfg.warn(format!(
                            "tier {q}: trigger {} is decided by the agent loop, not the tier router, and is ignored here",
                            gotext::quote(&tr)
                        ));
                    }
                }
                t.extra_args = f_strs(ty, tf, "extra_cli_args");
                t.extra_dirs = f_strs(ty, tf, "add_dirs");
                t.accept_edits = matches!(gostruct::field(ty, tf, "accept_edits"), Some(Val::Bool(true)));
            }
            _ => {
                if i > 0 && t.base_url.is_empty() {
                    return Err(format!("tier {q}: base_url is required"));
                }
            }
        }
        if looks_like_key(&t.api_key_env) {
            return Err(format!("tier {q}: api_key_env must be the NAME of an environment variable, not a key"));
        }
        // safe defaults: the first tier is the device itself; every later tier gets secrets redacted, and Claude also
        // never receives captures
        let mut def = AllowData::all();
        if i > 0 {
            def = AllowData::of(&[CLASS_NO_SECRETS]);
            if t.kind == KIND_CLAUDE || t.kind == KIND_CLAUDE_CLI {
                def.classes.push(CLASS_NO_CAPTURES.into());
            }
        }
        let raw = match gostruct::field(ty, tf, "allow_data") {
            Some(Val::Any(a)) => a.clone(),
            _ => Any::Null,
        };
        let (allow, warns) = parse_allow(&raw, &def).map_err(|e| format!("tier {q}: {e}"))?;
        t.allow_data = allow;
        for w in warns {
            cfg.warn(format!("tier {q}: {w}"));
        }
        cfg.tiers.push(t);
    }
    Ok(cfg)
}

/// Go's `looksLikeKey`: a key pasted where a variable's name belongs.
pub fn looks_like_key(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.contains([' ', '=', ':', '/', '.']) || s.starts_with("sk-") {
        return true;
    }
    !s.chars().all(|r| r == '_' || r.is_ascii_alphanumeric())
}

/// Go's `Path`: SARINA_TIERS_FILE, else `<home>/.sarina/tiers.json` (home empty: the user's own; "" when there is none).
pub fn path(home: &str, getenv: Option<&dyn Fn(&str) -> String>) -> String {
    if let Some(g) = getenv {
        let p = g("SARINA_TIERS_FILE");
        let p = gotext::trim_space(&p);
        if !p.is_empty() {
            return p.to_string();
        }
    }
    let home = if home.is_empty() { std::env::var("HOME").unwrap_or_default() } else { home.to_string() };
    if home.is_empty() {
        return String::new();
    }
    String::from_utf8_lossy(&gopath::join(&[home.as_bytes(), b".sarina", b"tiers.json"])).into_owned()
}

/// Go's `Load`: a missing file is the default (one local tier); a file that exists but does not read or is invalid gives
/// the error AND the default, so a caller that only warns stays local.
pub fn load(home: &str, getenv: Option<&dyn Fn(&str) -> String>) -> (Config, Option<String>) {
    let p = path(home, getenv);
    if p.is_empty() {
        return (default(), None);
    }
    let read = || -> Result<Vec<u8>, (bool, String)> {
        let mut f =
            std::fs::File::open(&p).map_err(|e| (e.kind() == std::io::ErrorKind::NotFound, goerr::path("open", p.as_bytes(), &e)))?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(|e| (false, goerr::path("read", p.as_bytes(), &e)))?;
        Ok(b)
    };
    let b = match read() {
        Ok(b) => b,
        Err((true, _)) => return (default(), None),
        Err((false, e)) => return (default(), Some(format!("cannot read {p}: {e}"))),
    };
    match parse(&b) {
        Ok(mut c) => {
            c.path = p;
            (c, None)
        }
        Err(e) => (default(), Some(format!("{p}: {e}"))),
    }
}

/// The parts of the original escalation setup that come from the command line and permissions (Go's
/// `EscalationOptions`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EscalationOptions {
    /// --escalate-to-claude-code, OR-ed with escalation_config.enabled
    pub enabled: bool,
    pub cli_path: String,
    pub triggers: Vec<String>,
    pub disable_resource_check: bool,
    /// --permission-mode acceptEdits for the CLI (never a bypass mode)
    pub accept_edits: bool,
    pub extra_dirs: Option<Vec<String>>,
}

/// Go's `FromEscalation`: with escalation on, a claude_cli tier named "claude-cli" is appended (unless the file already
/// has one) and the escalation's resource thresholds replace the tier defaults.
pub fn from_escalation(cfg: &Config, esc: &EscalationConfig, o: &EscalationOptions, getenv: Option<&dyn Fn(&str) -> String>) -> Config {
    if !(esc.enabled || o.enabled) || cfg.tiers.iter().any(|t| t.kind == KIND_CLAUDE_CLI) {
        return cfg.clone();
    }
    let mut out = cfg.clone();
    out.warnings = Some(cfg.warnings.clone().unwrap_or_default());
    let mut path = gotext::trim_space(&o.cli_path).to_string();
    if path.is_empty() {
        path = gotext::trim_space(&esc.cli_path).to_string();
    }
    if path.is_empty() || path == DEFAULT_CLI_PATH {
        if let Some(g) = getenv {
            let v = g("CLAUDE_CODE_CLI_PATH");
            let v = gotext::trim_space(&v);
            if !v.is_empty() {
                path = v.to_string();
            }
        }
    }
    if path.is_empty() {
        path = DEFAULT_CLI_PATH.into();
    }
    let max = if esc.max_escalations <= 0 { DEFAULT_MAX_ESCALATION } else { esc.max_escalations };
    let timeout = if esc.timeout_seconds <= 0.0 { DEFAULT_CLI_TIMEOUT } else { esc.timeout_seconds };
    let mut triggers: Option<Vec<String>> = None;
    for tr in esc.trigger_stop_reasons.iter().chain(o.triggers.iter()) {
        if tr == TRIGGER_BACKEND_ERROR || tr == TRIGGER_RESOURCE_EXHAUSTED {
            triggers.get_or_insert_with(Vec::new).push(tr.clone());
        } else {
            out.warn(format!("escalation trigger {tr} is decided by the agent loop, not the tier router, and is ignored here"));
        }
    }
    if esc.trigger_stop_reasons.is_empty() && triggers.is_none() {
        triggers = Some(vec![TRIGGER_BACKEND_ERROR.into(), TRIGGER_RESOURCE_EXHAUSTED.into()]);
    }
    out.tiers.push(Tier {
        name: "claude-cli".into(),
        kind: KIND_CLAUDE_CLI.into(),
        enabled: true,
        auto: true,
        allow_data: AllowData::of(&[CLASS_NO_SECRETS, CLASS_NO_CAPTURES]),
        cli_path: path,
        timeout_seconds: timeout,
        max_escalations: max,
        triggers,
        extra_args: Some(esc.extra_cli_args.clone()),
        extra_dirs: o.extra_dirs.clone(),
        accept_edits: o.accept_edits,
        ..Tier::default()
    });
    let rt = &esc.resource_thresholds;
    if let Some(x) = rt.max_memory_percent {
        out.resource.max_memory_percent = x;
    }
    if let Some(x) = rt.max_cpu_percent {
        out.resource.max_cpu_percent = x;
    }
    if let Some(x) = rt.max_gpu_memory_percent {
        out.resource.max_gpu_memory_percent = x;
    }
    if o.disable_resource_check {
        out.resource = Thresholds::default();
    }
    out
}
