//! labyrinth-ask-sarina [--cwd DIR] [--write] [--shell] [--conversation ID] [--voice] QUESTION...
//!
//! Ask Sarina one thing and print her answer: the conversation side of a spoken question (Super+S, the crash watch's "Ask Sarina",
//! her evals). A port of the Python script of the same name in Labyrinth_OS (profile/airootfs/usr/local/bin), line for line in
//! behaviour: the same files, environment, programs started with the same arguments, the same output and exit status. The
//! function names follow the Python ones so the two can be read side by side; tests/ runs both on the same inputs.

use crate::json::{self, Value};
use crate::proc;
use crate::py::{self, Crash, Env, LocalTime, PyResult};
use crate::{rx, S};
use std::collections::HashSet;
use std::io::Read;
use std::sync::OnceLock;

pub const VOICE_RULES: &str = "/usr/share/labyrinth/sarina-voice-rules.txt";
pub const DRAFT_WITHDRAWN: &str = "\x1edraft withdrawn\x1e";

// What a spoken question may use: reading, and her vault (the only place it may write, through the vault's own safety gate).
pub const VOICE_TOOLS: &str = "read_file,list_dir,glob_search,grep_search,vault_search,vault_read,vault_write,system_status,crash_detail,stage_binary,reverse_engineer,crypto_identify,crypto_decode,crypto_classic,crypto_hash,crypto_modern,stego_inspect";

pub const SPOKEN_BRIEF: &str = "Your answer is spoken aloud: at most three short sentences, about 60 words. Lead with the one thing to do or the answer itself; leave out alternatives and background unless asked, and offer more in a few words if there is more.";

/// What the program reads from the process around it: os.environ as it was at start, and ~/.config/labyrinth/sarina.env.
pub struct Ctx {
    pub environ: Env,
    env_file: OnceLock<PyResult<Env>>,
}

impl Ctx {
    pub fn new(environ: Env) -> Ctx {
        Ctx { environ, env_file: OnceLock::new() }
    }

    /// env_file(): KEY=value lines of sarina.env (the Python script read it again at each use; it does not change during a turn).
    pub fn env_file(&self) -> PyResult<&Env> {
        match self.env_file.get_or_init(|| read_env_file(&self.environ)) {
            Ok(e) => Ok(e),
            Err(c) => Err(Crash(c.0.clone())),
        }
    }

    fn runtime_dir(&self) -> String {
        match self.environ.truthy("XDG_RUNTIME_DIR") {
            Some(d) => d.to_string(),
            None => format!("/run/user/{}", py::getuid()),
        }
    }

    pub fn sessions(&self) -> String {
        self.environ.get_or("SARINA_SESSION_DIR", "/var/lib/sarina/sessions").to_string()
    }
}

pub fn read_env_file(environ: &Env) -> PyResult<Env> {
    let base = match environ.truthy("XDG_CONFIG_HOME") {
        Some(d) => d.to_string(),
        None => py::expanduser("~/.config", environ),
    };
    let path = py::join_all(&[&base, "labyrinth", "sarina.env"]);
    let mut out = Env::default();
    if let Some(text) = py::read_text_opt(&path)? {
        for line in py::lines_keepends(&text) {
            let line = py::strip(line);
            if !line.is_empty() && !line.starts_with('#') && line.contains('=') {
                let (k, v) = line.split_once('=').unwrap();
                out.set(py::strip(k), py::strip(v));
            }
        }
    }
    Ok(out)
}

// ---- timing

/// LABYRINTH_VOICE_TIMING=1: when each stage of a turn ended, in seconds from the start, as one line appended to
/// $XDG_RUNTIME_DIR/labyrinth-sarina/timing.log (whisper= first when labyrinth-sarina-ask passed LABYRINTH_VOICE_WHISPER_S).
pub struct Timing {
    on: bool,
    t0: f64,
    marks: Vec<(String, f64)>,
    whisper: Option<f64>,
}

impl Timing {
    pub fn new(environ: &Env) -> Timing {
        Timing {
            on: environ.get("LABYRINTH_VOICE_TIMING") == Some("1"),
            t0: py::monotonic(),
            marks: Vec::new(),
            whisper: py::parse_float(environ.get_or("LABYRINTH_VOICE_WHISPER_S", "")),
        }
    }

    pub fn mark(&mut self, stage: &str) {
        if self.on && !self.marks.iter().any(|(s, _)| s == stage) {
            self.marks.push((stage.to_string(), py::monotonic() - self.t0));
        }
    }

    pub fn write(&mut self, ctx: &Ctx) {
        if !self.on {
            return;
        }
        self.mark("done");
        let run = py::join(&ctx.runtime_dir(), "labyrinth-sarina");
        let mut marks: Vec<(String, f64)> = Vec::new();
        if let Some(w) = self.whisper {
            marks.push(("whisper".into(), w));
        }
        marks.extend(self.marks.iter().cloned());
        let line = format!(
            "{} {}",
            LocalTime::now().iso_t(),
            marks.iter().map(|(k, v)| format!("{}={}", k, py::fmt2(*v))).collect::<Vec<_>>().join(" ")
        );
        if py::makedirs(&run, 0o700).is_ok() {
            let _ = py::append_text(&py::join(&run, "timing.log"), &(line + "\n"));
        }
    }
}

// ---- small helpers

fn s(v: &str) -> String {
    v.to_string()
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

/// subprocess.run(..., capture_output=True, text=True, timeout=t): the decoded stdout and the returncode, None for OSError or
/// TimeoutExpired; a reply that is not UTF-8 ends the program (as UnicodeDecodeError did).
fn run_text(cmd: &[String], env: Option<&Env>, timeout: f64) -> PyResult<Option<(i32, String)>> {
    match proc::run_capture(cmd, env, None, Some(timeout)) {
        Ok(o) => {
            let out = py::universal_newlines(py::decode_strict(o.stdout)?);
            py::decode_strict(o.stderr)?; // text=True decodes stderr too
            Ok(Some((o.code, out)))
        }
        Err(_) => Ok(None),
    }
}

// ---- persona, routing, prefetch

/// How she talks, from Persona.md in her vault. Front matter dropped, 2500 characters at most.
pub fn persona_text(ctx: &Ctx) -> PyResult<String> {
    let vault = match ctx.environ.truthy("SARINA_VAULT") {
        Some(v) => v.to_string(),
        None => ctx.env_file()?.get_or("SARINA_VAULT", "").to_string(),
    };
    let text = match py::read_text_opt(&py::join(&vault, "Persona.md"))? {
        Some(t) => t,
        None => return Ok(String::new()),
    };
    let text = drop_front_matter(&text);
    let text = py::take(py::strip(text), 2500);
    Ok(if text.is_empty() { String::new() } else { format!("How you talk (your Persona note):\n{}", text) })
}

fn drop_front_matter(text: &str) -> &str {
    if text.starts_with("---\n") {
        if let Some(i) = text[4..].find("\n---") {
            return &text[4 + i + 4..];
        }
    }
    text
}

rx!(R_CRASHES, r"(?i)\b(?:crash|crashed|crashes|core dump|segfault|segmentation|aborted|froze|freez|hung|hang)");
rx!(R_PERF, r"(?i)\b(?:slow|lag|laggy|sluggish|performance|cpu|processor|memory|ram|swap|busy|fan|hot|temperature|heat|load)\b");
rx!(R_NET, r"(?i)\b(?:internet|wi-?fi|network|connection|connect|online|offline|dns|ethernet)\b");
rx!(R_DISK, r"(?i)\b(?:disk|storage|space|full)\b");
rx!(R_DRIVE, r"(?i)\bdrive\b");
rx!(R_BOOT, r"(?i)\b(?:boot|startup|start up|takes? long to (?:start|boot))\b");
rx!(R_AUDIO, r"(?i)\b(?:sound|audio|speaker|microphone|mic|volume)\b");
rx!(R_SERVICES, r"(?i)\b(?:error|errors|log|logs|failed|failing|broke|broken|problem|wrong|issue)\b");
rx!(R_GAMING, r"(?i)\b(?:game|games|gaming|steam|proton|wine|mods?|modding|fps|frame ?rates?|stutter[\p{L}\p{N}_]*|controller|gamepad)\b");

fn route_matches(i: usize, q: &str) -> bool {
    let q = &*rx::pyb(q);
    match i {
        0 => R_CRASHES.is_match(q),
        1 => R_PERF.is_match(q),
        2 => R_NET.is_match(q),
        3 => {
            // r"\b(disk|storage|space|full)\b|\b(?<!proton )drive\b"
            R_DISK.is_match(q)
                || R_DRIVE.find_iter(q).any(|m| {
                    let before: Vec<char> = q[..m.start()].chars().rev().take(7).collect();
                    let before: String = before.into_iter().rev().collect();
                    !before.eq_ignore_ascii_case("proton ")
                })
        }
        4 => R_BOOT.is_match(q),
        5 => R_AUDIO.is_match(q),
        6 => R_SERVICES.is_match(q),
        _ => R_GAMING.is_match(q),
    }
}

const ROUTE_TOPICS: [&str; 8] = ["crashes", "performance", "network", "disk", "boot", "audio", "services", "gaming"];

pub fn keyword_topics(question: &str) -> Vec<String> {
    let mut topics: Vec<String> = Vec::new();
    for (i, t) in ROUTE_TOPICS.iter().enumerate() {
        if route_matches(i, question) && !topics.iter().any(|x| x == t) {
            topics.push(s(t));
        }
    }
    topics
}

pub fn route_cache_path(ctx: &Ctx) -> String {
    py::join_all(&[&ctx.runtime_dir(), "labyrinth-sarina", "route-cache.json"])
}

rx!(R_NOT_KEY, r"[^a-z0-9 ]");

/// What the question needs looked up, decided by meaning (`sarina route`), or None when that is not available. A word or two is
/// decided by the keyword rules (none: talk); the answer for a phrase is kept for the session.
pub fn route(ctx: &Ctx, question: &str, env: &Env) -> PyResult<Option<Vec<String>>> {
    let low = py::lower(question);
    let key = R_NOT_KEY.replace_all(&low, "");
    let key = py::strip(&key).to_string();
    if py::split_ws(&key).len() <= 2 {
        let t = keyword_topics(question);
        return Ok(Some(if t.is_empty() { vec![s("chat")] } else { t }));
    }
    let cache_path = route_cache_path(ctx);
    // the cache as json.load gave it; a damaged one is {}
    let mut cache = match py::read_text(&cache_path) {
        Ok(t) => json::loads(&t).unwrap_or(Value::Obj(vec![])),
        Err(_) => Value::Obj(vec![]),
    };
    match &cache {
        Value::Obj(_) => {
            if let Some(v) = cache.get(&key) {
                return Ok(Some(list_of(v)?));
            }
        }
        Value::Arr(a) => {
            if a.iter().any(|x| x.as_str() == Some(key.as_str())) {
                return Err(Crash::new("TypeError: list indices must be integers or slices, not str"));
            }
        }
        Value::Str(t) => {
            if t.contains(key.as_str()) {
                return Err(Crash::new("TypeError: string indices must be integers, not 'str'"));
            }
        }
        _ => return Err(Crash::new("TypeError: argument of type is not iterable")),
    }
    let r = match run_text(&argv(&["sarina", "route", question]), Some(env), 8.0)? {
        Some(r) => r,
        None => return Ok(None),
    };
    if r.0 != 0 {
        return Ok(None);
    }
    let topics: Vec<String> = py::strip(&r.1).split(',').filter(|t| !t.is_empty()).map(s).collect();
    if !matches!(cache, Value::Obj(_)) {
        // a list or a str: more than 300 items is replaced by {}, else cache[key] = ... is a TypeError
        let dir = py::dirname(&cache_path);
        if py::makedirs(&dir, 0o700).is_err() {
            return Ok(Some(topics));
        }
        let n = match &cache {
            Value::Arr(a) => a.len(),
            Value::Str(t) => py::clen(t),
            _ => 0,
        };
        if n <= 300 {
            return Err(Crash::new("TypeError: object does not support item assignment"));
        }
        cache = Value::Obj(vec![]);
    }
    let dir = py::dirname(&cache_path);
    if py::makedirs(&dir, 0o700).is_ok() {
        if let Value::Obj(m) = &cache {
            if m.len() > 300 {
                cache = Value::Obj(vec![]);
            }
        }
        cache.set(&key, Value::Arr(topics.iter().map(|t| json::s(t)).collect()));
        let _ = py::write_text(&cache_path, &json::dumps(&cache));
    }
    Ok(Some(topics))
}

/// list(cache[key]): a list stays a list, a str becomes its characters, an object its keys.
fn list_of(v: &Value) -> PyResult<Vec<String>> {
    match v {
        Value::Arr(a) => a
            .iter()
            .map(|x| match x {
                Value::Str(t) => Ok(t.clone()),
                other => Ok(other.py_str()),
            })
            .collect(),
        Value::Str(t) => Ok(t.chars().map(|c| c.to_string()).collect()),
        Value::Obj(m) => Ok(m.iter().map(|(k, _)| k.clone()).collect()),
        _ => Err(Crash::new("TypeError: object is not iterable")),
    }
}

/// The program of the user's own crash to look at: the one the question names, else the newest of the last day.
pub fn recent_crash(question: &str) -> PyResult<String> {
    let out = match run_text(&argv(&["coredumpctl", "list", "--no-pager", "--no-legend", "--reverse", "--since=-1d"]), None, 10.0)? {
        Some(r) => r.1,
        None => return Ok(String::new()),
    };
    let uid = py::getuid().to_string();
    let mut mine: Vec<String> = Vec::new();
    for line in py::splitlines(&out) {
        let f = py::split_ws(line);
        if f.len() >= 10 && f[5] == uid {
            mine.push(s(f[9]));
        }
    }
    let q = py::lower(question);
    for exe in &mine {
        if q.contains(py::lower(py::basename(exe)).as_str()) {
            return Ok(exe.clone());
        }
    }
    Ok(mine.first().cloned().unwrap_or_default())
}

rx!(
    R_GENERAL,
    r"(?i)\b(?:how(?:'s| is| are|s)?|is|are|was) (?:the |my |this )?(?:system|computer|pc|machine|everything|things|it|all)\b|\b(?:status|check[- ]?up|health check)\b"
);
rx!(R_GAME_EXE, r"(?i)steam|wine|proton|\.exe");

/// What the computer reports about the given topics (at most two), as text to place before the question; empty when none.
pub fn prefetch(ctx: &Ctx, question: &str, topics: &[String]) -> PyResult<String> {
    let mut out: Vec<String> = Vec::new();
    let mut topics: Vec<String> = topics.iter().filter(|t| *t != "guides" && *t != "chat").cloned().collect();
    let general = hit(&R_GENERAL, question);
    if general && !topics.is_empty() && topics.iter().all(|t| t == "services" || t == "performance" || t == "disk") {
        let vault = match ctx.env_file()?.get("SARINA_VAULT") {
            Some(v) => s(v),
            None => s(ctx.environ.get_or("SARINA_VAULT", "")),
        };
        if let Some((rc, o)) = run_text(&argv(&["sarina", "daily-report", "--path", &vault, "--max-age", "15"]), None, 20.0)? {
            if rc == 0 && !py::strip(&o).is_empty() {
                out.push(s(py::take(py::strip(&o), 2500)));
                topics.clear();
            }
        }
    }
    for t in topics.iter().take(2) {
        let Some((rc, o)) = run_text(&argv(&["sarina", "system-status", t]), None, 20.0)? else { continue };
        if rc == 0 && !py::strip(&o).is_empty() {
            out.push(format!("[{}]\n{}", t, py::take(py::strip(&o), 2500)));
        }
    }
    if topics.iter().any(|t| t == "crashes") {
        let c = recent_crash(question)?;
        if !c.is_empty() {
            if let Some((rc, o)) = run_text(&argv(&["sarina", "crash-detail", &c]), None, 30.0)? {
                if rc == 0 && !py::strip(&o).is_empty() {
                    let base = py::basename(&c);
                    let label = format!("latest crash of {}", base);
                    let mut lead = String::new();
                    if !py::lower(question).contains(py::lower(base).as_str()) {
                        let game = hit(&R_GAME_EXE, &c);
                        lead = format!(
                            "Note first: the newest crash recorded is of {}, {}. Say which program this record is about{}\n",
                            base,
                            if game { "a Steam, Proton or Wine program" } else { "an ordinary program, not a game" },
                            if game {
                                "."
                            } else {
                                "; if they asked about a game, say that no game crash is recorded and that this one is a different program. Do not call it the game."
                            }
                        );
                    }
                    out.push(format!("[{}]\n{}{}", label, lead, py::take(py::strip(&o), 3500)));
                }
            }
        }
    }
    if out.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "What the computer reports right now (read-only, already looked up for you; call system_status for anything more):\n\n{}\n\nThe question: ",
        out.join("\n\n")
    ))
}

// ---- spoken text

rx!(R_EMPH, r"\*\*|__|`");
rx!(R_HEADING, concat!(r"^#{1,6}", S!(), "*"));
rx!(R_ITEM, concat!(r"^(?:[-*•]|\d{1,2}[.)])", S!(), r"+(.*)$"));
rx!(R_BLANKS, r"[ \t]+");
rx!(R_NL_SPACES, r" *\n *");
rx!(R_MANY_NL, r"\n{2,}");

/// A spoken answer is plain sentences: markdown marks, headings and list markers removed, list items made sentences on one line.
pub fn flatten(text: &str) -> String {
    let text = R_EMPH.replace_all(text, "");
    let mut out = String::new();
    for line in py::splitlines(&text) {
        let t = py::strip(line);
        if t.is_empty() {
            out.push('\n');
            continue;
        }
        let t = R_HEADING.replace(t, "").into_owned();
        let t = match R_ITEM.captures(&t) {
            Some(c) => {
                let mut x = py::strip(c.get(1).unwrap().as_str()).to_string();
                if let Some(last) = x.chars().last() {
                    if !".!?:".contains(last) {
                        x.push('.');
                    }
                }
                x
            }
            None => t,
        };
        out.push_str(&t);
        out.push(' ');
    }
    let text = R_BLANKS.replace_all(&out, " ");
    let text = R_NL_SPACES.replace_all(&text, "\n");
    let text = R_MANY_NL.replace_all(&text, "\n\n");
    py::strip(&text).to_string()
}

/// A spoken answer longer than about 75 words is cut after the last whole sentence that fits; the first sentence always stays.
pub fn shorten(text: &str, limit: usize) -> String {
    if py::split_ws(text).len() <= limit {
        return s(text);
    }
    let mut out: Vec<&str> = Vec::new();
    let mut n = 0;
    for sen in rx::split_sentences(text) {
        let w = py::split_ws(sen).len();
        if !out.is_empty() && n + w > limit {
            break;
        }
        out.push(sen);
        n += w;
    }
    s(py::strip(&out.join(" ")))
}

rx!(R_GUIDE_LINE, concat!(r"^(Guides/[^\s\x1c-\x1f].*?\.md)", S!(), "{2}"));

/// Her guide notes (Guides/ in the vault) that match the question: at most two, 2600 characters each.
pub fn guides(question: &str, vault: &str) -> PyResult<String> {
    if vault.is_empty() || !py::isdir(&py::join(vault, "Guides")) {
        return Ok(String::new());
    }
    let out = match run_text(&argv(&["sarina", "vault-search", "--path", vault, question]), None, 10.0)? {
        Some(r) => r.1,
        None => return Ok(String::new()),
    };
    let mut picked: Vec<String> = Vec::new();
    for line in py::splitlines(&out) {
        if let Some(c) = R_GUIDE_LINE.captures(line) {
            let g = s(c.get(1).unwrap().as_str());
            if !picked.contains(&g) {
                picked.push(g);
            }
        }
        if picked.len() == 2 {
            break;
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for rel in &picked {
        let Some(text) = py::read_text_opt(&py::join(vault, rel))? else { continue };
        let text = drop_front_matter(&text);
        parts.push(format!("[{}]\n{}", &rel[..rel.len() - 3], py::take(py::strip(text), 2600)));
    }
    if parts.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "From your guides about this computer (use them when they answer the question; prefer them to general Linux advice):\n\n{}\n\n",
        parts.join("\n\n")
    ))
}

rx!(R_NOTE, concat!(r"^", S!(), r"*\((?:Style hint|They laughed)[^)]*\)", S!(), "*"));

/// What was said, without the desktop's notes before the words ("(Style hint ...)", "(They laughed.)").
pub fn spoken(question: &str) -> String {
    let mut q = s(question);
    while let Some(m) = R_NOTE.find(&q) {
        q = q[m.end()..].to_string();
    }
    s(py::strip(&q))
}

rx!(
    R_ACTION,
    r"(?i)\b(?:sudo|pacman|yay|systemctl|delete|remove|uninstall|install|update|upgrade|rename|kill|restart|reboot|shut ?down|format|wipe|empty the)\b"
);

pub fn about_the_computer(topics: &[String], said: &str) -> bool {
    topics.iter().any(|t| t != "chat") || hit(&R_ACTION, said)
}

pub fn daypart(now: &LocalTime) -> &'static str {
    let h = now.hour;
    if h < 5 {
        "late at night"
    } else if h < 12 {
        "morning"
    } else if h < 17 {
        "afternoon"
    } else if h < 22 {
        "evening"
    } else {
        "late evening"
    }
}

rx!(
    R_SECRET,
    r"(?i)(?:password|passphrase|pass code|passcode|pin code|api key|token|secret|private key|credit card|\b[A-Za-z0-9+/_-]{32,}\b)"
);
rx!(R_CONTROL, r"[\x00-\x1f\x7f\x{202a}-\x{202e}\x{2066}-\x{2069}]");

/// Control and direction characters made spaces, stripped, at most n characters.
pub fn plain(t: &str, n: usize) -> String {
    let t = R_CONTROL.replace_all(t, " ");
    s(py::take(py::strip(&t), n))
}

rx!(R_NOTE_LINE, concat!(r"^([^\s\x1c-\x1f].*?\.md)", S!(), "{2}"));
rx!(R_THEY_SAID, r"\| they said: (.*?) \| I said:");

/// Earlier moments that fit what was just said, from her talk log, jokes and inside jokes (vault-search): at most three lines.
pub fn recall(question: &str, vault: &str, env: &Env) -> PyResult<Vec<String>> {
    let out = match run_text(&argv(&["sarina", "vault-search", "--path", vault, question]), Some(env), 10.0)? {
        Some(r) => r.1,
        None => return Ok(vec![]),
    };
    let mut res: Vec<String> = Vec::new();
    let mut note = String::new();
    for line in py::splitlines(&out) {
        if let Some(c) = R_NOTE_LINE.captures(line) {
            note = s(c.get(1).unwrap().as_str());
            continue;
        }
        if (note.starts_with("Talks/") || note.starts_with("Jokes/") || note.starts_with("Entities/Inside jokes"))
            && py::strip(line).starts_with("- 20")
        {
            let t = py::skip(py::strip(line), 2).to_string();
            let q = words_of(question);
            let said = match R_THEY_SAID.captures(&t) {
                Some(c) => words_of(c.get(1).unwrap().as_str()),
                None => HashSet::new(),
            };
            if !q.is_empty() && (q.intersection(&said).count() as f64) >= 0.8 * q.len() as f64 {
                continue;
            }
            if !res.contains(&t) {
                res.push(t);
            }
        }
        if res.len() == 3 {
            break;
        }
    }
    Ok(res)
}

pub fn tail_lines(path: &str, n: usize, prefix: &str) -> PyResult<Vec<String>> {
    let Some(text) = py::read_text_opt(path)? else { return Ok(vec![]) };
    let all: Vec<String> =
        py::lines_keepends(&text).into_iter().filter(|l| l.starts_with(prefix)).map(|l| s(py::skip(py::strip(l), 2))).collect();
    let k = all.len().saturating_sub(n);
    Ok(all[k..].to_vec())
}

rx!(R_MEMORY_ASK, r"(?i)\b(?:remember|recall|told you|said about|mentioned|last time|that time)\b");
rx!(R_CALLBACK, r"(?i)\b(?:remember|again|my name|funny|joke|laugh|that time|last time|the one about)\b");
rx!(R_JOKE_ASK, r"(?i)\b(?:tell|another|got|know|have|one more)\b.*\b(?:joke|pun|riddle)s?\b|make me laugh|something funny|cheer me up");
rx!(
    R_ANNOUNCE,
    concat!(
        r"(?i)^(?:ok(?:ay)?[,.]?",
        S!(),
        r"*)?(?:let me|i'?ll|i am going to|i'?m going to|i will|allow me to|give me a (?:moment|second)|one (?:moment|second)|hold on|checking|looking(?: into| at)?|let'?s (?:check|look|see))\b"
    )
);
rx!(
    R_SECURITY_OFF,
    r"(?i)\b(?:disabl(?:e|ing)|turn(?:ing)? off|switch(?:ing)? off|stop(?:ping)?|mask(?:ing)?|remov(?:e|ing)|uninstall(?:ing)?)\b[^.\n]{0,60}\b(?:ufw|firewall|firewalld|heimdall|apparmor|selinux|keyring|freshclam|clamav|antivirus|secure boot|limbo)\b"
);
rx!(R_WARNED, concat!(r"(?i)\b(?:do not|don'?t|never|avoid|no need to|without|not)", S!(), r"+(?:[\p{L}\p{N}_]+", S!(), r"+){0,2}$"));
rx!(
    R_OFFER,
    r"(?i)^(?:would you like|do you want|want me to|want help|need help|is there anything|let me know|if you (?:need|want)|i'?m (?:always )?here (?:if|for)|feel free|or is there anything|anything (?:else|i can))"
);
rx!(R_OFFER2, r"(?i)\b(?:anything (?:else|i can do|specific)|help (?:ease|you with|with (?:something|anything)))\b");
rx!(R_HELP, r"(?i)\b(?:help|assist)\b");

/// A case-insensitive search with Python's idea of a word boundary (see rx::pyb).
fn hit(r: &regex::Regex, s: &str) -> bool {
    r.is_match(&rx::pyb(s))
}

fn offer(x: &str) -> bool {
    hit(&R_OFFER, x) || hit(&R_OFFER2, x)
}

/// A closing offer of help is a help desk, not a friend; it is dropped from a chat answer that has something else in it.
pub fn drop_offers(text: &str) -> String {
    let stripped = py::strip(text);
    let cut = drop_closing_aside(stripped);
    let text: &str = if cut.is_empty() { text } else { &cut };
    let mut sentences: Vec<&str> = rx::split_sentences(py::strip(text));
    while sentences.len() > 1 && offer(sentences[sentences.len() - 1]) {
        sentences.pop();
    }
    let kept: Vec<&str> = sentences.iter().copied().filter(|x| !hit(&R_HELP, x) || !offer(x)).collect();
    if kept.is_empty() {
        sentences.join(" ")
    } else {
        kept.join(" ")
    }
}

/// re.sub(r"(?<=[.!?])\s*\([^()]{8,}\)\s*$", "", text): a closing aside in brackets, after a sentence's end, is removed.
fn drop_closing_aside(text: &str) -> String {
    if !text.ends_with(')') {
        return s(text);
    }
    let body_end = text.len() - 1;
    let Some(open) = text[..body_end].rfind(['(', ')']) else { return s(text) };
    if &text[open..open + 1] != "(" {
        return s(text);
    }
    if py::clen(&text[open + 1..body_end]) < 8 {
        return s(text);
    }
    let before = py::rstrip(&text[..open]);
    if before.ends_with(['.', '!', '?']) {
        s(before)
    } else {
        s(text)
    }
}

rx!(R_OBS, r"^- (.+?) — seen (\d+)×, last (\d{4}-\d{2}-\d{2})\.");

/// The owner's name from the local config (SARINA_OWNER_NAME), never from the code.
pub fn owner_name(ctx: &Ctx) -> PyResult<String> {
    if let Some(n) = ctx.environ.truthy("SARINA_OWNER_NAME") {
        return Ok(s(py::strip(n)));
    }
    if let Some(n) = ctx.env_file()?.truthy("SARINA_OWNER_NAME") {
        return Ok(s(py::strip(n)));
    }
    Ok(s("the owner"))
}

/// The value of a run of decimal digits (any script, as int() reads them).
fn digits_value(d: &str) -> u128 {
    let mut v: u128 = 0;
    for c in d.chars() {
        let x = match c.to_digit(10) {
            Some(x) if c.is_ascii() => x as u128,
            _ => rx::unicode_digit_value(c) as u128,
        };
        v = v.saturating_mul(10).saturating_add(x);
    }
    v
}

/// What she has learned about the owner (Entities/About <name>.md): observations seen at least twice, or in the last week, most
/// seen first; only the Observations section.
pub fn learned(ctx: &Ctx, vault: &str, today_days: i64, n: usize) -> PyResult<Vec<String>> {
    let name = owner_name(ctx)?;
    let Some(text) = py::read_text_opt(&py::join_all(&[vault, "Entities", &format!("About {}.md", name)]))? else {
        return Ok(vec![]);
    };
    let mut out: Vec<(u128, String)> = Vec::new();
    let mut section = String::new();
    for line in py::splitlines(&text) {
        if let Some(rest) = line.strip_prefix("## ") {
            section = py::lower(py::strip(rest));
            continue;
        }
        if section != "observations" {
            continue;
        }
        let Some(c) = R_OBS.captures(py::strip(line)) else { continue };
        let seen = digits_value(c.get(2).unwrap().as_str());
        let last = py::date_fromiso(c.get(3).unwrap().as_str()).ok_or_else(|| Crash::new("ValueError: Invalid isoformat string"))?;
        if seen >= 2 || today_days - last <= 7 {
            out.push((seen, s(c.get(1).unwrap().as_str())));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(out.into_iter().take(n).map(|(_, t)| t).collect())
}

pub const NEUTRAL: [&str; 10] = [
    "What time is it?",
    "Tell me something interesting.",
    "How are you doing today?",
    "What do you think about that?",
    "Okay, sounds good.",
    "Good morning!",
    "Can you help me with something?",
    "I was just thinking about stuff.",
    "Tell me a short joke.",
    "Yes or no?",
];

const LEARN_STOP: &[&str] = &[
    "their", "about", "with", "they", "them", "from", "that", "this", "when", "have", "likes", "prefers", "wants", "being", "despite",
    "the", "and", "you", "are", "was", "for", "how", "its", "not", "but", "can", "did", "has", "had", "his", "her", "she", "him", "our",
    "who", "why", "way", "all", "any", "one", "out", "get", "got", "too", "now", "new", "day", "see", "say", "what", "think", "doing",
    "your", "just", "like", "some", "will", "would", "could", "should", "really", "today", "tell", "know", "want", "yes",
];

rx!(R_AZ3, r"[a-z]{3,}");

fn sha1_hex(data: &[u8]) -> String {
    crate::sha1::hex(data)
}

/// Embeddings from the small model Sarina already uses for her notes (Ollama /api/embed), cached by text; None when not there.
fn embed(texts: &[String], env: &Env, cache: &mut Value) -> PyResult<Option<Vec<Value>>> {
    let key = |t: &str| sha1_hex(t.as_bytes());
    let todo: Vec<&String> = texts.iter().filter(|t| cache.get(&key(t)).is_none()).collect();
    if !todo.is_empty() {
        let body = json::dumps(&json::obj(vec![
            ("model", json::s(env.get_or("SARINA_EMBED_MODEL", "nomic-embed-text"))),
            ("input", Value::Arr(todo.iter().map(|t| json::s(t)).collect())),
        ]));
        let url = env.get_or("SARINA_EMBED_URL", "http://127.0.0.1:11434/api/embed");
        let resp = match crate::http::request(url, Some((body.as_bytes(), "application/json")), 8.0, usize::MAX) {
            Ok(b) => b,
            Err(_) => return Ok(None),
        };
        let Ok(v) = json::loads_bytes(&resp) else { return Ok(None) };
        let embs = match &v {
            Value::Obj(_) => match v.get("embeddings") {
                Some(e) => e.clone(),
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        let items: Vec<Value> = match embs {
            Value::Arr(a) => a,
            Value::Obj(m) => m.into_iter().map(|(k, _)| Value::Str(k)).collect(),
            Value::Str(t) => t.chars().map(|c| Value::Str(c.to_string())).collect(),
            _ => return Ok(None),
        };
        for (t, e) in todo.iter().zip(items) {
            if !matches!(cache, Value::Obj(_)) {
                return Err(Crash::new("TypeError: the embedding cache is not a dict"));
            }
            cache.set(&key(t), e);
        }
    }
    let mut out = Vec::new();
    for t in texts {
        match cache.get(&key(t)) {
            Some(v) => out.push(v.clone()),
            None => return Err(Crash::new("KeyError")),
        }
    }
    Ok(Some(out))
}

/// A number as Python holds it in this arithmetic: an int stays exact, a float is a float.
#[derive(Clone, Copy, Debug)]
enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    fn of(v: &Value) -> PyResult<Num> {
        match v {
            Value::Int(s) => s.parse::<i128>().map(Num::Int).map_err(|_| Crash::new("OverflowError")),
            Value::Bool(b) => Ok(Num::Int(*b as i128)),
            Value::Float(f) => Ok(Num::Float(*f)),
            _ => Err(Crash::new("TypeError: unsupported operand type(s)")),
        }
    }

    fn f(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    fn mul(self, o: Num) -> PyResult<Num> {
        Ok(match (self, o) {
            (Num::Int(a), Num::Int(b)) => Num::Int(a.checked_mul(b).ok_or_else(|| Crash::new("OverflowError"))?),
            (a, b) => Num::Float(a.f() * b.f()),
        })
    }
}

/// sum() as CPython 3.14 does it: ints exactly, then floats with Neumaier's compensated summation.
fn py_sum(items: impl Iterator<Item = PyResult<Num>>) -> PyResult<Num> {
    let mut it = items;
    let mut i_result: i128 = 0;
    let first_float;
    loop {
        match it.next() {
            None => return Ok(Num::Int(i_result)),
            Some(x) => match x? {
                Num::Int(v) => i_result = i_result.checked_add(v).ok_or_else(|| Crash::new("OverflowError"))?,
                Num::Float(f) => {
                    first_float = i_result as f64 + f;
                    break;
                }
            },
        }
    }
    let mut f_result = first_float;
    let mut c = 0.0f64;
    for x in it {
        match x? {
            Num::Float(x) => {
                let t = f_result + x;
                if f_result.abs() >= x.abs() {
                    c += (f_result - t) + x;
                } else {
                    c += (x - t) + f_result;
                }
                f_result = t;
            }
            Num::Int(v) => f_result += v as f64,
        }
    }
    if c != 0.0 && c.is_finite() {
        f_result += c;
    }
    Ok(Num::Float(f_result))
}

fn as_list(v: &Value) -> PyResult<Vec<Num>> {
    match v {
        Value::Arr(a) => a.iter().map(Num::of).collect(),
        _ => Err(Crash::new("TypeError: object is not iterable")),
    }
}

/// a / b (true division)
fn div(a: Num, b: Num) -> PyResult<f64> {
    let d = b.f();
    if d == 0.0 {
        return Err(Crash::new("ZeroDivisionError"));
    }
    Ok(a.f() / d)
}

fn cos(a: &[Num], b: &[Num]) -> PyResult<f64> {
    let dot = py_sum(a.iter().zip(b.iter()).map(|(x, y)| x.mul(*y)))?;
    let na = py_sum(a.iter().map(|x| x.mul(*x)))?;
    let nb = py_sum(b.iter().map(|y| y.mul(*y)))?;
    let prod = na.mul(nb)?;
    let root = prod.f().sqrt();
    let denom = if root == 0.0 { Num::Int(1) } else { Num::Float(root) };
    div(dot, denom)
}

extern "C" {
    fn pow(x: f64, y: f64) -> f64;
}

/// x ** 2.0 for a float: C's pow, as CPython calls it.
fn square(x: f64) -> f64 {
    unsafe { pow(x, std::hint::black_box(2.0)) }
}

/// The observations that are about what was just said: clearly closer to it in meaning than to everyday small talk, or sharing a word.
pub fn relevant(ctx: &Ctx, question: &str, observations: &[String], env: &Env) -> PyResult<Vec<String>> {
    if observations.is_empty() {
        return Ok(vec![]);
    }
    let mut stop: HashSet<String> = LEARN_STOP.iter().map(|w| s(w)).collect();
    for m in R_AZ3.find_iter(&py::lower(&owner_name(ctx)?)) {
        stop.insert(s(m.as_str()));
    }
    let words =
        |t: &str| -> HashSet<String> { R_AZ3.find_iter(&py::lower(t)).map(|m| s(m.as_str())).filter(|w| !stop.contains(w)).collect() };
    let said = words(question);
    let mut out: Vec<String> = observations.iter().filter(|o| !said.is_disjoint(&words(o))).cloned().collect();
    let cache_path = py::join_all(&[&ctx.runtime_dir(), "labyrinth-sarina", "embed-cache.json"]);
    let mut cache = match py::read_text(&cache_path) {
        Ok(t) => json::loads(&t).unwrap_or(Value::Obj(vec![])),
        Err(_) => Value::Obj(vec![]),
    };
    let docs = embed(&observations.iter().map(|o| format!("search_document: {}", o)).collect::<Vec<_>>(), env, &mut cache)?;
    let mut qtexts: Vec<String> = NEUTRAL.iter().map(|q| format!("search_query: {}", q)).collect();
    qtexts.push(format!("search_query: {}", question));
    let qs = embed(&qtexts, env, &mut cache)?;
    if let (Some(docs), Some(qs)) = (docs, qs) {
        if !docs.is_empty() && !qs.is_empty() {
            let qn: Vec<Vec<Num>> = qs.iter().map(as_list).collect::<PyResult<_>>()?;
            for (o, d) in observations.iter().zip(docs.iter()) {
                let d = as_list(d)?;
                let base: Vec<f64> = qn[..qn.len() - 1].iter().map(|n| cos(n, &d)).collect::<PyResult<_>>()?;
                let mean = div(py_sum(base.iter().map(|b| Ok(Num::Float(*b))))?, Num::Int(base.len() as i128))?;
                let var = div(py_sum(base.iter().map(|b| Ok(Num::Float(square(b - mean)))))?, Num::Int(base.len() as i128))?;
                let sdv = var.sqrt();
                let sd = if 0.02 > sdv { 0.02 } else { sdv };
                if (cos(&qn[qn.len() - 1], &d)? - mean) / sd >= 1.5 && !out.contains(o) {
                    out.push(o.clone());
                }
            }
            let dir = py::dirname(&cache_path);
            if py::makedirs(&dir, 0o700).is_ok() {
                if let Value::Obj(m) = &cache {
                    if m.len() > 500 {
                        cache = Value::Obj(vec![]);
                    }
                }
                let _ = py::write_text(&cache_path, &json::dumps(&cache));
            }
        }
    }
    Ok(observations.iter().filter(|o| out.contains(o)).take(4).cloned().collect())
}

/// What a friend would already know when the talk is not about the computer: the time of day, how the owner has sounded lately,
/// what she learned that bears on it, inside jokes when asked, earlier moments that fit. Placed before the question.
pub fn companion(ctx: &Ctx, question: &str, env: &Env, now: Option<LocalTime>) -> PyResult<String> {
    let now = now.unwrap_or_else(LocalTime::now);
    let vault = s(env.get_or("SARINA_VAULT", ""));
    let mut parts: Vec<String> = vec![format!("It is {} {}, {}.", now.day_name(), daypart(&now), now.hm())];
    // sarina-tone reads its own numbers only (no model, no graphics card): it runs while the vault is searched
    let tone =
        if env.get_or("LABYRINTH_TONE", "1") != "0" { Some(Pending::start(&argv(&["sarina-tone", "--pattern"]), 5.0)) } else { None };
    let mut vault_parts: Vec<String> = Vec::new();
    if !vault.is_empty() && py::isdir(&vault) {
        let sp = spoken(question);
        let obs = learned(ctx, &vault, now.days(), 20)?;
        let known = relevant(ctx, &sp, &obs, env)?;
        if !known.is_empty() {
            vault_parts.push(format!(
                "What you have learned about them that bears on this (your own note; you know it the way a friend does: use it only if it fits, never list it or say where it comes from):\n{}",
                known.iter().map(|x| format!("- {}", plain(x, 200))).collect::<Vec<_>>().join("\n")
            ));
        }
        let mut inside: Vec<String> = Vec::new();
        if hit(&R_CALLBACK, &sp) {
            inside = tail_lines(&py::join_all(&[&vault, "Entities", "Inside jokes.md"]), 8, "- 20")?;
            if !inside.is_empty() {
                vault_parts.push(format!(
                    "Your inside jokes with them (they may be asking about one of these):\n{}",
                    inside.iter().map(|x| format!("- {}", plain(x, 200))).collect::<Vec<_>>().join("\n")
                ));
            }
        }
        let moments: Vec<String> = recall(&sp, &vault, env)?.into_iter().filter(|x| !inside.contains(x)).collect();
        if !moments.is_empty() {
            vault_parts.push(format!(
                "Earlier moments and inside jokes that may be about this (from your notes; use one only if it clearly is):\n{}",
                moments.iter().map(|x| format!("- {}", plain(x, 240))).collect::<Vec<_>>().join("\n")
            ));
        } else if inside.is_empty() && hit(&R_MEMORY_ASK, &sp) {
            vault_parts.push(s("Your notes (Talks, Jokes, inside jokes) hold nothing about what they are asking you to remember. Say plainly that you do not remember it; do not invent one, and do not guess at details."));
        }
    }
    if let Some(t) = tone {
        if let Some((rc, out)) = t.finish()? {
            if rc == 0 && !py::strip(&out).is_empty() {
                parts.push(format!(
                    "How they have sounded lately (from their voice only; do not bring it up unless the moment really fits, and if asked how you know, say so honestly): {}",
                    plain(&out, 400)
                ));
            }
        }
    }
    parts.extend(vault_parts);
    Ok(format!("About this moment (for you, not to recite):\n{}\n\nWhat they said: ", parts.join("\n\n")))
}

/// A program started now and read later (sarina-tone, alongside the vault lookups).
struct Pending {
    child: Option<std::process::Child>,
    started: std::time::Instant,
    timeout: f64,
}

impl Pending {
    fn start(cmd: &[String], timeout: f64) -> Pending {
        let mut c = proc::command(cmd, None, None);
        c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        Pending { child: c.spawn().ok(), started: std::time::Instant::now(), timeout }
    }

    /// (returncode, stdout) as subprocess.run(..., timeout) gave them, or None (OSError, TimeoutExpired).
    fn finish(self) -> PyResult<Option<(i32, String)>> {
        let Some(child) = self.child else { return Ok(None) };
        let left = self.timeout - self.started.elapsed().as_secs_f64();
        match proc::finish(child, Some(left)) {
            Ok(o) => {
                let out = py::universal_newlines(py::decode_strict(o.stdout)?);
                py::decode_strict(o.stderr)?;
                Ok(Some((o.code, out)))
            }
            Err(_) => Ok(None),
        }
    }
}

/// What she said in her joke log of this month and the one before.
pub fn told_jokes(vault: &str) -> PyResult<Vec<String>> {
    let dir = py::join(vault, "Jokes");
    let mut files: Vec<String> = py::listdir(&dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.starts_with("20") && n.ends_with(".md") && n.len() >= 5)
        .map(|n| py::join(&dir, &n))
        .collect();
    files.sort();
    let k = files.len().saturating_sub(2);
    let mut out = Vec::new();
    for f in &files[k..] {
        for x in tail_lines(f, 400, "- 20")? {
            if let Some((_, said)) = x.split_once("| I said:") {
                out.push(s(py::strip(said)));
            }
        }
    }
    Ok(out)
}

rx!(R_WORDS, r"[a-z']{3,}");

pub fn words_of(t: &str) -> HashSet<String> {
    R_WORDS.find_iter(&py::lower(t)).map(|m| s(m.as_str())).collect()
}

rx!(R_TAGS, concat!(r"\[(?:joking|unsure|urgent|gentle|serious)\]", S!(), "*"));

/// Her own voice coming back through the microphone: four words or more, most of them in her last answer.
pub fn is_echo(said: &str, prev_answer: &str) -> bool {
    if prev_answer.is_empty() {
        return false;
    }
    let a = words_of(&R_TAGS.replace_all(prev_answer, ""));
    let h = words_of(said);
    if h.len() < 4 || a.is_empty() {
        return false;
    }
    (h.intersection(&a).count() as f64) >= 0.7 * h.len() as f64
}

/// The joke she already told that this answer repeats (most of its words again), or "".
pub fn a_repeat(answer: &str, told: &[String]) -> String {
    let a = words_of(&R_TAGS.replace_all(answer, ""));
    if a.len() < 4 {
        return String::new();
    }
    for t in told {
        let w = words_of(t);
        if !w.is_empty() && (a.intersection(&w).count() as f64) >= 0.7 * a.len().min(w.len()) as f64 {
            return t.clone();
        }
    }
    String::new()
}

/// SARINA_TEST=1 (her eval runner): notes are written only inside SARINA_TEST_ROOT or the temporary folder.
pub fn test_refuses(vault: &str, env: &Env) -> PyResult<bool> {
    if env.get("SARINA_TEST") != Some("1") {
        return Ok(false);
    }
    let p = py::realpath(vault)?;
    let mut roots: Vec<String> = env.get_or("SARINA_TEST_ROOT", "").split(':').filter(|r| !r.is_empty()).map(s).collect();
    roots.push(s(env.truthy("TMPDIR").unwrap_or("/tmp")));
    for r in &roots {
        let rr = py::realpath(r)?;
        if p == rr || p.starts_with(&format!("{}/", rr)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Talk that is not about the computer is kept in her vault (Talks/YYYY-MM.md), one line each; short replies and anything that
/// looks like a secret are not. LABYRINTH_TALK_LOG=0 turns this off (and the joke and laugh notes).
pub fn log_talk(question: &str, answer: &str, env: &Env) -> PyResult<()> {
    if env.get_or("LABYRINTH_TALK_LOG", "1") == "0" {
        return Ok(());
    }
    let vault = env.get_or("SARINA_VAULT", "");
    let q = spoken(question);
    if vault.is_empty()
        || !py::isdir(vault)
        || test_refuses(vault, env)?
        || py::split_ws(&q).len() < 3
        || hit(&R_SECRET, &q)
        || hit(&R_SECRET, answer)
    {
        return Ok(());
    }
    let now = LocalTime::now();
    let d = py::join(vault, "Talks");
    if py::makedirs(&d, 0o777).is_err() {
        return Ok(());
    }
    let path = py::join(&d, &format!("{}.md", now.year_month()));
    let mut text = String::new();
    if !py::exists(&path) {
        text.push_str(&format!(
            "# Talks {}\n\nWhat we talked about when it was not about the computer. Each line: when, what the owner said, what I said.\n\n",
            now.year_month()
        ));
    }
    text.push_str(&format!(
        "- {} | they said: {} | I said: {}\n",
        now.ymd_hm(),
        plain(&q, 240),
        plain(&R_TAGS.replace_all(answer, ""), 240)
    ));
    let _ = py::append_text(&path, &text);
    Ok(())
}

/// They laughed at what she said just before: that line goes into Entities/Inside jokes, once.
pub fn note_laugh(prev: Option<(String, String)>, env: &Env) -> PyResult<()> {
    let vault = env.get_or("SARINA_VAULT", "");
    let Some((q, ans)) = prev else { return Ok(()) };
    if env.get_or("LABYRINTH_TALK_LOG", "1") == "0" || vault.is_empty() || !py::isdir(vault) || test_refuses(vault, env)? {
        return Ok(());
    }
    let ans = R_TAGS.replace_all(&ans, "").into_owned();
    if py::strip(&ans).is_empty() || hit(&R_SECRET, &ans) || hit(&R_SECRET, &q) {
        return Ok(());
    }
    let path = py::join_all(&[vault, "Entities", "Inside jokes.md"]);
    let today = LocalTime::now().date_iso();
    let line = format!("- {}: they laughed when I said \"{}\" (to: {})\n", today, plain(&ans, 200), plain(&q, 120));
    if py::makedirs(&py::dirname(&path), 0o777).is_err() {
        return Ok(());
    }
    if !py::exists(&path) {
        let head = format!(
            "---\ncreated: {}\nsource: sarina\n---\n\n# Inside jokes\n\nThings that made the owner laugh, so she can call back to them. One line each, with the date.\n\n",
            today
        );
        if py::write_text(&path, &head).is_err() {
            return Ok(());
        }
    }
    match py::read_text(&path) {
        Ok(t) => {
            if t.contains(plain(&ans, 200).as_str()) {
                return Ok(());
            }
        }
        Err(py::ReadErr::Os) => return Ok(()),
        Err(py::ReadErr::Crash(c)) => return Err(c),
    }
    let _ = py::append_text(&path, &line);
    Ok(())
}

rx!(R_HOWTO, r"(?i)\b(?:how (?:do|can) i|where (?:is|do|can)|setting|shortcut|install|sign in|open)\b");
rx!(R_LAUGH, r"(?i)\(They laughed\.\)|\b(?:ha(?:ha)+|lol|lmao|that'?s (?:so )?funny|hilarious)\b");
rx!(R_JOKE_WORD, r"(?i)\b(?:joke|funny|pun|humou?r)\b");

pub fn laugh(q: &str) -> bool {
    hit(&R_LAUGH, q)
}

/// When she told a joke, keep what it answered (Jokes/YYYY-MM.md), so she can say later why she said it.
pub fn log_joke(question: &str, answer: &str, env: &Env) -> PyResult<()> {
    let question = spoken(question);
    if env.get_or("LABYRINTH_TALK_LOG", "1") == "0" {
        return Ok(());
    }
    if !answer.contains("[joking]") && !hit(&R_JOKE_WORD, &question) {
        return Ok(());
    }
    let vault = env.get_or("SARINA_VAULT", "");
    if vault.is_empty() || !py::isdir(vault) || test_refuses(vault, env)? {
        return Ok(());
    }
    let now = LocalTime::now();
    let joke = plain(&R_TAGS.replace_all(answer, ""), 300);
    let d = py::join(vault, "Jokes");
    if py::makedirs(&d, 0o777).is_err() {
        return Ok(());
    }
    let path = py::join(&d, &format!("{}.md", now.year_month()));
    let mut text = String::new();
    if !py::exists(&path) {
        text.push_str(&format!(
            "# Jokes {}\n\nEach line: when, what the owner said, what I said. I chose a joke because of what was said just before it.\n\n",
            now.year_month()
        ));
    }
    text.push_str(&format!("- {} | they said: {} | I said: {}\n", now.ymd_hm(), plain(&question, 200), joke));
    let _ = py::append_text(&path, &text);
    Ok(())
}

/// The id of the session a first turn of a conversation made (the one new file in the sessions folder), or None.
pub fn new_session(ctx: &Ctx, before: &HashSet<String>, state: Option<&str>) -> Option<String> {
    state?;
    let names = py::listdir(&ctx.sessions()).ok()?;
    let new: Vec<&String> = names.iter().filter(|n| n.ends_with(".json") && !before.contains(*n)).collect();
    if new.len() == 1 {
        Some(new[0][..new[0].len() - 5].to_string())
    } else {
        None
    }
}

/// The question and answer of this conversation's turn before.
pub fn last_turn(state: &str) -> PyResult<Option<(String, String)>> {
    let Some(t) = py::read_text_opt(&format!("{}.last", state))? else { return Ok(None) };
    let (q, ans) = t.split_once('\n').unwrap_or((&t, ""));
    Ok(if ans.is_empty() { None } else { Some((s(q), s(ans))) })
}

pub fn save_turn(state: &str, question: &str, answer: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(format!("{}.last", state)) {
        let _ = f.write_all(format!("{}\n{}", question.replace('\n', " "), answer).as_bytes());
    }
}

rx!(R_EMOJI, r"[\x{1F000}-\x{1FAFF}\x{2600}-\x{27BF}\x{FE0F}\x{200D}]");
rx!(R_NOTES_ANY, concat!(r"\((?:They laughed|Style hint)[^)]*\)", S!(), "*"));
rx!(R_ROUND_TAG, r"(?i)\((joking|unsure|urgent|gentle|serious)\)");
rx!(R_SPACES, concat!(S!(), "+"));

/// One spoken sentence as it is shown and said: no notes, no markdown, tags in brackets, no emoji.
pub fn clean_sentence(t: &str) -> String {
    let t = R_NOTES_ANY.replace_all(t, "");
    let t = R_ROUND_TAG.replace_all(&t, "[$1]");
    let t = flatten(&t);
    let t = R_EMOJI.replace_all(&t, "");
    s(py::strip(&R_SPACES.replace_all(&t, " ")))
}

rx!(R_NONWORD, r"[^\p{L}\p{N}_]+");
rx!(R_SENTENCE_END, concat!(r"[.!?]", S!(), "+$"));

/// Turns the model's reply, as it arrives, into finished spoken sentences, so the first can be said while the rest is still
/// being written (see the Python Streamer: think blocks dropped, the question said back dropped, offers and announcements not
/// said, a withdrawn draft cut off and corrected, about 75 words at most).
pub struct Streamer {
    said: String,
    chat: bool,
    limit: usize,
    raw: String,
    pos: usize,
    pub out: Vec<String>,
    words: usize,
    corrected: bool,
    said_before: Vec<String>,
    pending_apology: bool,
}

impl Streamer {
    pub fn new(said: &str, chat: bool) -> Streamer {
        Streamer {
            said: s(py::strip(said)),
            chat,
            limit: 75,
            raw: String::new(),
            pos: 0,
            out: Vec::new(),
            words: 0,
            corrected: false,
            said_before: Vec::new(),
            pending_apology: false,
        }
    }

    fn ready(&self, fin: bool) -> Vec<String> {
        let text = rx::strip_tag_blocks(&self.raw[self.pos..]);
        let text = match rx::open_tag(&text) {
            Some(i) => &text[..i],
            None => &text[..],
        };
        let mut parts: Vec<&str> = rx::split_sentences_not_numbers(text);
        if !fin && !R_SENTENCE_END.is_match(text) && !parts.is_empty() {
            parts.pop();
        }
        parts.into_iter().filter(|p| !py::strip(p).is_empty()).map(s).collect()
    }

    fn take(&mut self, sentence: &str, fin: bool) -> Option<String> {
        let mut sentence = s(sentence);
        if self.out.is_empty()
            && self.words == 0
            && py::clen(&self.said) > 12
            && py::lower(&sentence).starts_with(py::lower(&self.said).as_str())
        {
            sentence = s(py::lstrip_chars(py::skip(&sentence, py::clen(&self.said)), " .?!,"));
        }
        let sentence = clean_sentence(&sentence);
        if sentence.is_empty() {
            return None;
        }
        if self.out.is_empty() && py::clen(&sentence) > 12 {
            let a = R_NONWORD.replace_all(&py::lower(&sentence), " ").into_owned();
            let b = R_NONWORD.replace_all(&py::lower(&self.said), " ").into_owned();
            if b.contains(py::strip(&a)) {
                return None;
            }
        }
        if self.chat && offer(&sentence) && !(fin && self.out.is_empty()) {
            return None;
        }
        if hit(&R_ANNOUNCE, &sentence) && !(fin && self.out.is_empty()) {
            return None;
        }
        let seen = rx::pyb(&sentence);
        if let Some(m) = R_SECURITY_OFF.find(&seen) {
            if !R_WARNED.is_match(&seen[..m.start()]) {
                return None;
            }
        }
        if self.said_before.contains(&sentence) {
            return None;
        }
        if !self.said_before.is_empty() && !self.corrected {
            self.corrected = true;
            self.out.push(s("Sorry, let me correct that."));
            self.pending_apology = true;
        }
        let n = py::split_ws(&sentence).len();
        if !self.out.is_empty() && self.words + n > self.limit {
            return None;
        }
        self.words += n;
        self.out.push(sentence.clone());
        Some(sentence)
    }

    pub fn feed(&mut self, chunk: &str, fin: bool) -> Vec<String> {
        self.raw.push_str(chunk);
        let marker = format!("{}]", DRAFT_WITHDRAWN);
        if let Some(i) = self.raw[self.pos..].find(&marker).map(|i| i + self.pos) {
            let rest = s(py::lstrip_chars(&self.raw[i + marker.len()..], "\n "));
            self.raw = rest;
            self.pos = 0;
            self.said_before = self.out.clone();
            self.words = 0;
            return self.feed("", fin);
        }
        let ready = self.ready(fin);
        if !fin && !ready.is_empty() {
            let last = ready.last().unwrap();
            self.pos = match self.raw[self.pos..].find(last.as_str()) {
                Some(c) => self.pos + c + last.len(),
                None => self.raw.len(),
            };
        } else if fin {
            self.pos = self.raw.len();
        }
        let mut emitted = Vec::new();
        for p in &ready {
            if let Some(t) = self.take(p, fin) {
                if self.pending_apology {
                    self.pending_apology = false;
                    emitted.push(s("Sorry, let me correct that."));
                }
                emitted.push(t);
            }
        }
        emitted
    }

    pub fn finish(&mut self) -> Vec<String> {
        self.feed("", true)
    }

    pub fn text(&self) -> String {
        self.out.join("\n")
    }
}

/// UTF-8 decoded as it arrives, with replacement characters for bad bytes and universal newlines, like the TextIOWrapper the
/// Python script read her reply through.
pub struct StreamDecoder {
    buf: Vec<u8>,
    pending_cr: bool,
}

impl StreamDecoder {
    pub fn new() -> StreamDecoder {
        StreamDecoder { buf: Vec::new(), pending_cr: false }
    }

    pub fn push(&mut self, bytes: &[u8], fin: bool) -> Vec<char> {
        self.buf.extend_from_slice(bytes);
        let mut chars = Vec::new();
        let mut i = 0;
        loop {
            match std::str::from_utf8(&self.buf[i..]) {
                Ok(v) => {
                    chars.extend(v.chars());
                    i = self.buf.len();
                    break;
                }
                Err(e) => {
                    let good = e.valid_up_to();
                    chars.extend(std::str::from_utf8(&self.buf[i..i + good]).unwrap().chars());
                    i += good;
                    match e.error_len() {
                        Some(n) => {
                            chars.push('\u{fffd}');
                            i += n;
                        }
                        None => {
                            if fin {
                                chars.push('\u{fffd}');
                                i = self.buf.len();
                            }
                            break;
                        }
                    }
                }
            }
        }
        self.buf.drain(..i);
        let mut out = Vec::new();
        for c in chars {
            if self.pending_cr {
                self.pending_cr = false;
                out.push('\n');
                if c == '\n' {
                    continue;
                }
            }
            if c == '\r' {
                self.pending_cr = true;
            } else {
                out.push(c);
            }
        }
        if fin && self.pending_cr {
            self.pending_cr = false;
            out.push('\n');
        }
        out
    }
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs her with the reply streamed, prints each spoken sentence the moment it is complete; (exit code, the whole answer).
fn run_streaming(cmd: &[String], env: &Env, streamer: &mut Streamer, timing: &mut Timing) -> (i32, String) {
    let mut full: Vec<String> = cmd.to_vec();
    full.push(s("-stream"));
    let mut c = proc::command(&full, Some(env), None);
    c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(_) => return (1, String::new()),
    };
    let mut so = child.stdout.take().unwrap();
    let mut dec = StreamDecoder::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = so.read(&mut buf).unwrap_or(0);
        let fin = n == 0;
        for ch in dec.push(&buf[..n], fin) {
            timing.mark("first_token");
            let mut b = [0u8; 4];
            for sen in streamer.feed(ch.encode_utf8(&mut b), false) {
                timing.mark("first_sentence");
                py::print_flush(&sen);
            }
        }
        if fin {
            break;
        }
    }
    for sen in streamer.finish() {
        timing.mark("first_sentence");
        py::print_flush(&sen);
    }
    let rc = child.wait().map(proc::returncode).unwrap_or(1);
    (rc, streamer.text())
}

/// The voice rules name the owner as {owner} and may add {owner_note}: the shipped file names nobody.
pub fn owner_filled(ctx: &Ctx, rules: &str) -> PyResult<String> {
    let ef = ctx.env_file()?;
    let name = s(py::strip(ctx.environ.truthy("SARINA_OWNER_NAME").or(ef.truthy("SARINA_OWNER_NAME")).unwrap_or("the owner")));
    let note = s(py::strip(ctx.environ.truthy("SARINA_OWNER_NOTE").or(ef.truthy("SARINA_OWNER_NOTE")).unwrap_or("")));
    Ok(rules.replace("{owner}", &name).replace("{owner_note}", &if note.is_empty() { String::new() } else { format!(" {}", note) }))
}

#[allow(clippy::too_many_arguments)]
pub fn command(
    ctx: &Ctx,
    question: &str,
    cwd: &str,
    write: bool,
    shell: bool,
    voice: bool,
    resume: Option<&str>,
    rules: &str,
) -> PyResult<Vec<String>> {
    let mut cmd = match resume {
        Some(r) => argv(&["sarina", "resume", r, question]),
        None => argv(&["sarina", "chat", question]),
    };
    cmd.extend([s("--cwd"), py::abspath(cwd)?, s("--session-dir"), ctx.sessions()]);
    if voice {
        let spec = match ctx.environ.truthy("LABYRINTH_VOICE_EXTRA_TOOLS") {
            Some(v) => s(v),
            None => s(ctx.env_file()?.get_or("LABYRINTH_VOICE_EXTRA_TOOLS", "")),
        };
        let extra: Vec<String> = spec
            .split(',')
            .map(|t| s(py::strip(t)))
            .filter(|t| !t.is_empty() && t.chars().all(|c| py::is_alnum(c) || c == '_' || c == '-'))
            .collect();
        let mut tools = vec![s(VOICE_TOOLS)];
        tools.extend(extra);
        cmd.extend([s("--tools"), tools.join(","), s("--allow-write")]);
        let persona: Vec<String> =
            [persona_text(ctx)?, s(SPOKEN_BRIEF), owner_filled(ctx, rules)?].into_iter().filter(|x| !x.is_empty()).collect();
        if !persona.is_empty() {
            cmd.extend([s("--append-system-prompt"), persona.join("\n\n")]);
        }
    } else if write {
        cmd.push(s("--allow-write"));
    }
    if shell {
        cmd.push(s("--allow-shell"));
    }
    Ok(cmd)
}

/// LABYRINTH_VOICE_PREFETCH: an owner's add-on commands (';'-separated, absolute programs), each run with what was said as the
/// last argument; what they print replaces the usual notes.
pub fn addon_prefetch(ctx: &Ctx, said: &str, env: &Env) -> PyResult<String> {
    let spec = match ctx.environ.truthy("LABYRINTH_VOICE_PREFETCH") {
        Some(v) => s(v),
        None => s(ctx.env_file()?.get_or("LABYRINTH_VOICE_PREFETCH", "")),
    };
    let mut out: Vec<String> = Vec::new();
    for x in spec.split(';') {
        let c: Vec<String> = py::split_ws(x).into_iter().map(s).collect();
        if c.is_empty() || !py::isabs(&c[0]) || !py::access_x(&c[0]) {
            continue;
        }
        let mut cmd = c.clone();
        cmd.push(s(said));
        let Some((rc, o)) = run_text(&cmd, Some(env), 5.0)? else { continue };
        if rc == 0 && !py::strip(&o).is_empty() {
            out.push(s(py::take(py::strip(&o), 6000)));
        }
    }
    Ok(out.join("\n"))
}

// ---- main

pub fn main(prog: &str, args: &[String]) -> i32 {
    py::coerce_c_locale();
    let ctx = Ctx::new(Env::from_process());
    let mut timing = Timing::new(&ctx.environ);
    match run(&ctx, prog, args, &mut timing) {
        Ok(Some(code)) => {
            timing.write(&ctx);
            py::exit_status(code)
        }
        Ok(None) => 0,
        Err(Crash(what)) => {
            eprintln!("Traceback: {}", what);
            1
        }
    }
}

/// Ok(None): argparse ended it (help printed or an error, already exited).
fn run(ctx: &Ctx, prog: &str, args: &[String], timing: &mut Timing) -> PyResult<Option<i32>> {
    let default_cwd = py::getcwd()?;
    let a = match crate::argparse::parse(prog, args, &default_cwd) {
        crate::argparse::Outcome::Args(a) => a,
        crate::argparse::Outcome::Help(h) => {
            print!("{}", h);
            std::process::exit(0);
        }
        crate::argparse::Outcome::Error(e) => {
            eprint!("{}", e);
            std::process::exit(2);
        }
    };
    let mut env = ctx.environ.clone();
    for (k, v) in &ctx.env_file()?.vars {
        env.setdefault(k, v);
    }
    env.setdefault("SARINA_TEMPERATURE", "0.6");
    env.setdefault("SARINA_REQUEST_MAX_TOKENS", "6000");
    env.setdefault("SARINA_SEMANTIC", "on");
    env.setdefault(
        "SARINA_PRIVATE_DIRS",
        "~/.mozilla:~/.config/vivaldi:~/.config/chromium:~/.config/google-chrome:~/.local/share/keyrings:~/.password-store:~/.gnupg:~/.ssh",
    );
    if a.voice && !env.contains("LABYRINTH_VOICE_RULES") {
        if let Some(r) = py::read_text_opt(VOICE_RULES)? {
            env.set("LABYRINTH_VOICE_RULES", py::strip(&r));
        }
    }
    let mut state: Option<String> = None;
    let mut resume: Option<String> = None;
    let mut before: HashSet<String> = HashSet::new();
    if !a.conversation.is_empty() && a.conversation.chars().all(|c| py::is_alnum(c) || c == '-' || c == '_') {
        let run = py::join(&ctx.runtime_dir(), "labyrinth-sarina");
        py::makedirs(&run, 0o700).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
        let st = py::join(&run, &format!("conv-{}.session", a.conversation));
        if let Some(t) = py::read_text_opt(&st)? {
            let sid = py::strip(&t);
            if !sid.is_empty() && sid.chars().all(py::is_alnum) {
                resume = Some(s(sid));
            }
        }
        if resume.is_none() {
            before = py::listdir(&ctx.sessions()).map(|v| v.into_iter().collect()).unwrap_or_default();
        }
        state = Some(st);
    }
    let joined = a.question.join(" ");
    let mut question = joined.clone();
    let mut chat = false;
    if a.voice {
        let said = spoken(&question);
        let topics = route(ctx, &said, &env)?;
        timing.mark("route");
        let topics = match topics {
            Some(t) => t,
            None => {
                let mut t = keyword_topics(&said);
                if !t.is_empty() || hit(&R_HOWTO, &said) {
                    t.push(s("guides"));
                }
                t
            }
        };
        chat = !about_the_computer(&topics, &said);
        if chat {
            question = companion(ctx, &question, &env, None)? + &question;
        } else {
            let g = guides(&said, env.get_or("SARINA_VAULT", ""))?;
            if env.get("SARINA_SYSTEM") == Some("on") {
                question = prefetch(ctx, &said, &topics)? + &question;
            }
            question = g + &question;
        }
        if let Some(st) = &state {
            if hit(&R_LAUGH, &joined) {
                note_laugh(last_turn(st)?, &env)?;
            }
        }
    }
    if a.voice {
        let prev = match &state {
            Some(st) => last_turn(st)?,
            None => None,
        };
        if let Some((_, ans)) = &prev {
            if is_echo(&spoken(&joined), ans) {
                py::print_flush("I did not catch that: it sounded like my own voice.");
                return Ok(Some(3));
            }
        }
        let pre = addon_prefetch(ctx, &spoken(&joined), &env)?;
        if !pre.is_empty() {
            question = format!("{}\n\nWhat they said: {}", pre, joined);
        }
        timing.mark("prefetch");
        let think = py::lower(ctx.environ.truthy("LABYRINTH_VOICE_THINK").or(env.truthy("LABYRINTH_VOICE_THINK")).unwrap_or("computer"));
        if think == "off" || (think == "computer" && chat && pre.is_empty()) {
            question.push_str(" /no_think");
        }
    }
    let rules = s(env.get_or("LABYRINTH_VOICE_RULES", ""));
    let cmd = command(ctx, &question, &a.cwd, a.write, a.shell, a.voice, resume.as_deref(), &rules)?;
    let rc;
    if a.voice {
        let said_q = spoken(&joined);
        let joke = chat && hit(&R_JOKE_ASK, &said_q);
        if !joke && env.get_or("LABYRINTH_VOICE_STREAM", "1") != "0" {
            let mut streamer = Streamer::new(&said_q, chat);
            let (code, answer) = run_streaming(&cmd, &env, &mut streamer, timing);
            log_joke(&joined, &answer, &env)?;
            if chat && !answer.contains("[joking]") {
                log_talk(&joined, &answer, &env)?;
            }
            if let Some(st) = &state {
                save_turn(st, &said_q, &answer);
            }
            if state.is_some() && resume.is_none() {
                if let Some(sid) = new_session(ctx, &before, state.as_deref()) {
                    let _ = py::write_text(state.as_ref().unwrap(), &sid);
                }
            }
            return Ok(Some(if !answer.is_empty() || code != 0 { code } else { 4 }));
        }
        let p = proc::run_stdout(&cmd, Some(&env), None).map_err(|_| Crash::new("OSError: sarina could not be started"))?;
        rc = p.code;
        let mut out = py::universal_newlines(py::decode_replace(&p.stdout));
        if chat && rc == 0 && hit(&R_JOKE_ASK, &said_q) {
            let old = a_repeat(&rx::strip_tag_blocks(&out), &told_jokes(env.get_or("SARINA_VAULT", ""))?);
            if !old.is_empty() {
                let again = format!(
                    "You told that one before ({}). Tell a different joke, a new one, about the same thing. Only the joke.",
                    py::take(&old, 160)
                );
                let sid = match &resume {
                    Some(r) => Some(r.clone()),
                    None => new_session(ctx, &before, state.as_deref()),
                };
                let c2 = match &sid {
                    Some(sid) => command(ctx, &again, &a.cwd, false, false, true, Some(sid), &rules)?,
                    None => command(ctx, &format!("{} The request was: {}", again, said_q), &a.cwd, false, false, true, None, &rules)?,
                };
                let p2 = proc::run_stdout(&c2, Some(&env), None).map_err(|_| Crash::new("OSError: sarina could not be started"))?;
                let o2 = py::universal_newlines(py::decode_replace(&p2.stdout));
                if p2.code == 0 && !py::strip(&o2).is_empty() {
                    out = o2;
                }
            }
        }
        let answer = rx::strip_tag_blocks(&out);
        let answer = R_NOTES_ANY.replace_all(&answer, "").into_owned();
        let mut answer = R_ROUND_TAG.replace_all(&answer, "[$1]").into_owned();
        let said = s(py::strip(&said_q));
        if py::clen(&said) > 12 && py::lower(py::lstrip(&answer)).starts_with(py::lower(&said).as_str()) {
            answer = s(py::lstrip_chars(py::skip(py::lstrip(&answer), py::clen(&said)), " .?!,"));
        }
        let mut answer = shorten(&flatten(&answer), 75);
        if chat {
            answer = drop_offers(&answer);
        }
        let answer = R_EMOJI.replace_all(&answer, "");
        let answer = R_TRAILING_BLANKS.replace_all(&answer, "\n");
        let answer = R_THREE_NL.replace_all(&answer, "\n\n");
        let answer = s(py::strip(&answer));
        py::print_flush(&answer);
        log_joke(&joined, &answer, &env)?;
        if chat && !answer.contains("[joking]") {
            log_talk(&joined, &answer, &env)?;
        }
        if let Some(st) = &state {
            save_turn(st, &said_q, &answer);
        }
    } else {
        rc = proc::run_inherit(&cmd, Some(&env)).map_err(|_| Crash::new("OSError: sarina could not be started"))?;
    }
    if state.is_some() && resume.is_none() {
        if let Some(sid) = new_session(ctx, &before, state.as_deref()) {
            let _ = py::write_text(state.as_ref().unwrap(), &sid);
        }
    }
    Ok(Some(rc))
}

rx!(R_TRAILING_BLANKS, r"[ \t]+\n");
rx!(R_THREE_NL, r"\n{3,}");
