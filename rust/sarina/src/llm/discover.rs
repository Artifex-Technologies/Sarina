//! Go: `llm/discover.go`: how large a backend's context window is, asked in a short round of probes (Ollama's `/api/ps`
//! and `/api/show`, llama.cpp's `/props`, the OpenAI-compatible `/models`), each answer read to at most 4 MiB (a larger
//! one counts as none), a transport failure ending the round, the result (also "the server would not say") cached per
//! base URL and model for a TTL, a cancelled caller's half answer not cached.

use crate::config::{as_int, as_map, ModelConfig};
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gohttp::{self, Dial, Opts};
use crate::gostruct::{self, Field, Ty, Val};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The whole round's budget when none is set.
pub const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
/// How long an answer is kept when no TTL is set.
pub const DEFAULT_DISCOVERY_TTL: Duration = Duration::from_secs(300);
pub const MIN_PLAUSIBLE_WINDOW: i64 = 256;
pub const MAX_PLAUSIBLE_WINDOW: i64 = 100_000_000;
/// An answer past this is not a model description: as if the server had none.
const MAX_ANSWER: usize = 4 << 20;

/// Go's `Discoverer`.
#[derive(Default)]
pub struct Discoverer {
    /// the authorities for https; None = the system's
    pub tls: Option<Arc<rustls::ClientConfig>>,
    /// zero = 3 s
    pub timeout: Duration,
    /// zero = 5 min
    pub ttl: Duration,
    /// the clock (tests set it)
    pub now: Option<Box<dyn Fn() -> Instant + Send + Sync>>,
    cache: Mutex<HashMap<String, (i64, String, Instant)>>,
}

fn plausible(n: i64) -> bool {
    (MIN_PLAUSIBLE_WINDOW..=MAX_PLAUSIBLE_WINDOW).contains(&n)
}

/// Go's `sameModel`: without regard to case or surrounding space, an Ollama ":latest" optional.
pub fn same_model(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = crate::gotext::to_lower(crate::gotext::trim_space(s));
        s.strip_suffix(":latest").map(str::to_string).unwrap_or(s)
    };
    !a.is_empty() && !b.is_empty() && norm(a) == norm(b)
}

/// Go's `serverRoot`: the base URL without its `/v1` (and what follows), query and fragment, so native endpoints can be
/// addressed; the base as it is (without trailing slashes) when it does not parse or names no host.
pub fn server_root(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    let Some(mut u) = crate::gourl::parse(trimmed) else { return trimmed.to_string() };
    if u.opaque || u.host.is_empty() {
        return trimmed.to_string();
    }
    if let Some(i) = u.path.windows(3).position(|w| w == b"/v1") {
        u.path.truncate(i);
    }
    u.string().trim_end_matches('/').to_string()
}

static PS: Ty = Ty::Struct(
    "struct { Models []struct { Name string; Model string; ContextLength int \"json:\\\"context_length\\\"\" } \"json:\\\"models\\\"\" }",
    "",
    &[Field("models", &PS_MODELS)],
);
static PS_MODELS: Ty = Ty::Slice(&PS_MODEL);
static PS_MODEL: Ty = Ty::Struct("", "", &[Field("Name", &Ty::Str), Field("Model", &Ty::Str), Field("context_length", &Ty::Int)]);
static SHOW: Ty = Ty::Struct("", "", &[Field("parameters", &Ty::Str), Field("model_info", &SHOW_INFO)]);
static SHOW_INFO: Ty = Ty::Map(&Ty::Any);
static PROPS: Ty = Ty::Map(&Ty::Any);
static LIST: Ty = Ty::Struct("", "", &[Field("data", &LIST_DATA)]);
static LIST_DATA: Ty = Ty::Slice(&PROPS);

/// Go's `json.Unmarshal(b, &v) == nil`: the value when nothing went wrong.
fn read(t: &Ty, b: &[u8]) -> Option<Val> {
    match gostruct::unmarshal(t, b) {
        (v, None) => Some(v),
        _ => None,
    }
}

fn val_map(v: &Val) -> BTreeMap<String, Any> {
    match v {
        Val::Map(Some(m)) => m
            .iter()
            .map(|(k, x)| {
                (
                    k.clone(),
                    match x {
                        Val::Any(a) => a.clone(),
                        _ => Any::Null,
                    },
                )
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// Go's `regexp.MustCompile(`(?m)^\s*num_ctx\s+(\d+)`)` with Go's ASCII `\s` and `\d`.
fn num_ctx(params: &str) -> Option<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?m)^[\t\n\x0C\r ]*num_ctx[\t\n\x0C\r ]+([0-9]+)").expect("pattern"));
    re.captures(params).map(|c| c[1].to_string())
}

/// What one probe gave: an answer (2xx, at most 4 MiB), none (try the next probe), or a transport failure (stop).
enum Got {
    Body(Vec<u8>),
    None,
    Failed,
}

impl Discoverer {
    /// Go's `ContextWindow`: the window the backend reports for the model and where the number came from ("ollama",
    /// "llama.cpp", "models"); 0 when it would not say.
    pub fn context_window(&self, ctx: &Ctx, cfg: &ModelConfig) -> (i64, String) {
        let now = || self.now.as_ref().map_or_else(Instant::now, |f| f());
        let ttl = if self.ttl.is_zero() { DEFAULT_DISCOVERY_TTL } else { self.ttl };
        let key = format!("{}\x00{}", cfg.base_url, cfg.model);
        if let Some((n, s, at)) = self.cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            if now().saturating_duration_since(*at) < ttl {
                return (*n, s.clone());
            }
        }
        let timeout = if self.timeout.is_zero() { DEFAULT_DISCOVERY_TIMEOUT } else { self.timeout };
        let by = Instant::now() + timeout;
        let (n, source) = self.probe(ctx, by, cfg);
        if ctx.err().is_some() {
            // cancelled by the caller: a half answer is not cached
            return (n, source);
        }
        self.cache.lock().unwrap_or_else(|p| p.into_inner()).insert(key, (n, source.clone(), now()));
        (n, source)
    }

    fn get(&self, ctx: &Ctx, by: Instant, cfg: &ModelConfig, method: &str, url: &str, body: Option<&[u8]>) -> Got {
        let auth = format!("Bearer {}", cfg.api_key);
        let mut headers = vec![("Authorization", auth.as_str())];
        if body.is_some() {
            headers.push(("Content-Type", "application/json"));
        }
        if crate::llm::client::bad_header(&headers).is_some() {
            return Got::Failed;
        }
        let deadline: gohttp::Deadline = Rc::new(Cell::new(by));
        let o = Opts { dial: Dial::Tcp, method, url, headers: &headers, body, tls: self.tls.clone() };
        let Ok(mut resp) = gohttp::open(&o, ctx, &deadline) else { return Got::Failed };
        let mut b = Vec::new();
        if (&mut resp.body).take(MAX_ANSWER as u64 + 1).read_to_end(&mut b).is_err() || resp.encoded {
            return Got::Failed;
        }
        if b.len() > MAX_ANSWER || !(200..=299).contains(&resp.status) {
            return Got::None;
        }
        Got::Body(b)
    }

    fn probe(&self, ctx: &Ctx, by: Instant, cfg: &ModelConfig) -> (i64, String) {
        let root = server_root(&cfg.base_url);
        let none = (0, String::new());
        let found = |n: i64, s: &str| (n, s.to_string());
        // Ollama: the context the loaded model runs with, then the configured num_ctx or the model's trained maximum
        match self.get(ctx, by, cfg, "GET", &format!("{root}/api/ps"), None) {
            Got::Failed => return none,
            Got::None => {}
            Got::Body(b) => {
                if let Some(Val::Struct(f)) = read(&PS, &b) {
                    if let Val::Slice(Some(models)) = &f[0] {
                        for m in models {
                            let Val::Struct(mf) = m else { continue };
                            let s = |i: usize| match &mf[i] {
                                Val::Str(x) => x.as_str(),
                                _ => "",
                            };
                            let n = match &mf[2] {
                                Val::Int(n) => *n,
                                _ => 0,
                            };
                            if (same_model(s(0), &cfg.model) || same_model(s(1), &cfg.model)) && plausible(n) {
                                return found(n, "ollama");
                            }
                        }
                    }
                }
            }
        }
        let mut show_body = String::from("{\"model\":");
        crate::gojson::write_str(&mut show_body, &cfg.model, true);
        show_body.push_str(",\"name\":");
        crate::gojson::write_str(&mut show_body, &cfg.model, true);
        show_body.push('}');
        match self.get(ctx, by, cfg, "POST", &format!("{root}/api/show"), Some(show_body.as_bytes())) {
            Got::Failed => return none,
            Got::None => {}
            Got::Body(b) => {
                if let Some(Val::Struct(f)) = read(&SHOW, &b) {
                    let params = match &f[0] {
                        Val::Str(s) => s.as_str(),
                        _ => "",
                    };
                    if let Some(n) = num_ctx(params).and_then(|d| crate::gotext::atoi(d.as_bytes())) {
                        if plausible(n) {
                            return found(n, "ollama");
                        }
                    }
                    // Go walks the map in random order; the first key in sorted order here (docs/RUST_PORT.md)
                    for (k, v) in val_map(&f[1]) {
                        if k.ends_with(".context_length") {
                            if let Some(n) = as_int(Some(&v)).filter(|&n| plausible(n)) {
                                return found(n, "ollama");
                            }
                        }
                    }
                }
            }
        }
        // llama.cpp's server: the context it was started with
        match self.get(ctx, by, cfg, "GET", &format!("{root}/props"), None) {
            Got::Failed => return none,
            Got::None => {}
            Got::Body(b) => {
                if let Some(v) = read(&PROPS, &b) {
                    let props = val_map(&v);
                    let nested = as_map(props.get("default_generation_settings")).get("n_ctx").cloned();
                    for v in [nested, props.get("n_ctx").cloned()] {
                        if let Some(n) = as_int(v.as_ref()).filter(|&n| plausible(n)) {
                            return found(n, "llama.cpp");
                        }
                    }
                }
            }
        }
        // an OpenAI-compatible model list (vLLM, LM Studio, OpenRouter, ...)
        let url = format!("{}/models", cfg.base_url.trim_end_matches('/'));
        let Got::Body(b) = self.get(ctx, by, cfg, "GET", &url, None) else { return none };
        let Some(Val::Struct(f)) = read(&LIST, &b) else { return none };
        let data: Vec<BTreeMap<String, Any>> = match &f[0] {
            Val::Slice(Some(items)) => items.iter().map(val_map).collect(),
            _ => Vec::new(),
        };
        let entry = data
            .iter()
            .find(|m| match m.get("id") {
                Some(Any::String(id)) => same_model(id, &cfg.model),
                _ => false,
            })
            .or(if data.len() == 1 { data.first() } else { None });
        if let Some(m) = entry {
            let sub = |k: &str, f: &str| as_map(m.get(k)).get(f).cloned();
            let fields = [
                m.get("max_model_len").cloned(),
                m.get("context_length").cloned(),
                m.get("context_window").cloned(),
                m.get("max_context_length").cloned(),
                m.get("loaded_context_length").cloned(),
                m.get("n_ctx").cloned(),
                sub("meta", "n_ctx"),
                sub("meta", "n_ctx_train"),
                sub("top_provider", "context_length"),
            ];
            for v in fields {
                if let Some(Any::Bool(_)) = v {
                    continue;
                }
                if let Some(n) = as_int(v.as_ref()).filter(|&n| plausible(n)) {
                    return found(n, "models");
                }
            }
        }
        none
    }
}
