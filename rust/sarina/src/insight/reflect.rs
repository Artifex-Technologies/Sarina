//! Go: `go/insight/insight.go` and `command.go`'s `reflect`: once a day the talk log since the last time is read, the model
//! is asked what those lines show about the owner, and each answer whose quote really is in what they said is folded into
//! one note, Entities/About <owner>.md (seen once more when already there; never again once moved under "Forgotten").

use super::{capital, cut_bytes, local_time, norm, owner_from_env, replace_once, similar, Line};
use crate::goctx::Ctx;
use crate::gostruct::{self, Field, Ty};
use crate::gotext::{self, quote, trim_space};
use crate::gotime::Time;
use crate::llm::openai::Request;
use crate::llm::types::Message;
use std::sync::LazyLock;

fn re(go: &str) -> regex::Regex {
    crate::goregexp::compile(go).expect("a fixed pattern")
}

/// Go's `Observation`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Observation {
    pub text: String,
    pub seen: i64,
    /// YYYY-MM-DD
    pub last: String,
    /// their own words, newest first, at most three
    pub evidence: Vec<String>,
    /// one of [`KINDS`]' keys; "" = other
    pub kind: String,
}

/// Go's `Kinds`: the headings observations are listed under.
pub const KINDS: [(&str, &str); 7] = [
    ("fact", "Facts"),
    ("like", "Likes"),
    ("dislike", "Dislikes"),
    ("habit", "Habits and routines"),
    ("mood", "Moods and what affects them"),
    ("goal", "Goals and projects"),
    ("", "Other"),
];

/// Go's `kindOf`.
fn kind_of(s: &str) -> &'static str {
    let s = gotext::to_lower(trim_space(s));
    for (key, heading) in KINDS {
        if !key.is_empty() && (s == key || s == format!("{key}s") || gotext::equal_fold(&s, heading)) {
            return key;
        }
    }
    ""
}

/// Go's `Note`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub observations: Vec<Observation>,
    pub forgotten: Vec<String>,
}

static OBS_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"^- (.+?) — seen (\d+)×, last (\d{4}-\d{2}-\d{2})\.(?: Evidence: (.*))?$"));
static QUOTE_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r#""([^"]*)""#));
static THINK_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"(?s)<think>.*?</think>"));
static OBJ_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"\{[^{}]*\}"));

/// Go's `strconv.Atoi` of a string of ASCII digits, as `seen, _ := strconv.Atoi(...)` keeps it: past int64, the largest.
fn atoi_digits(d: &str) -> i64 {
    let t = d.trim_start_matches('0');
    if t.len() > 19 {
        return i64::MAX;
    }
    t.parse::<u64>().map_or(0, |n| i64::try_from(n).unwrap_or(i64::MAX))
}

/// Go's `ParseNote`.
pub fn parse_note(text: &str) -> Note {
    let mut n = Note::default();
    let (mut section, mut kind) = (String::new(), "");
    for l in text.split('\n') {
        let l = trim_space(l);
        if let Some(h) = l.strip_prefix("## ") {
            section = gotext::to_lower(trim_space(h));
            kind = "";
            continue;
        }
        if let Some(h) = l.strip_prefix("### ") {
            kind = kind_of(h);
            continue;
        }
        let Some(item) = l.strip_prefix("- ") else { continue };
        match section.as_str() {
            "forgotten" => {
                let t = match OBS_RE.captures(l) {
                    Some(m) => m[1].to_string(),
                    None => trim_space(item).to_string(),
                };
                n.forgotten.push(t);
            }
            "observations" => match OBS_RE.captures(l) {
                None => n.observations.push(Observation {
                    text: trim_space(item).to_string(),
                    seen: 1,
                    kind: kind.into(),
                    ..Default::default()
                }),
                Some(m) => {
                    let ev = m.get(4).map_or("", |x| x.as_str());
                    n.observations.push(Observation {
                        text: m[1].to_string(),
                        seen: atoi_digits(&m[2]),
                        last: m[3].to_string(),
                        evidence: QUOTE_RE.captures_iter(ev).take(3).map(|q| q[1].to_string()).collect(),
                        kind: kind.into(),
                    });
                }
            },
            _ => {}
        }
    }
    n
}

impl Note {
    /// Go's `Render`: the note's body (the vault adds the front matter).
    pub fn render(&self) -> String {
        let o = owner_from_env();
        let mut b = format!(
            "# About {0}\n\nWhat I have learned from talking with {0}, each with how often I have seen it and {1} own words that show it. ",
            o.name, o.possessive
        );
        b += &format!(
            "I update this myself once a day. {} can edit it: anything moved under Forgotten I will not add again.\n\n## Observations\n\n",
            capital(&o.name)
        );
        let mut obs = self.observations.clone();
        obs.sort_by(|a, b| b.seen.cmp(&a.seen).then_with(|| b.last.as_bytes().cmp(a.last.as_bytes())));
        for (key, heading) in KINDS {
            let these: Vec<&Observation> = obs.iter().filter(|o| kind_of(&o.kind) == key).collect();
            if these.is_empty() {
                continue;
            }
            b += &format!("### {heading}\n\n");
            for o in these {
                b += &format!("- {} — seen {}×, last {}.", o.text, o.seen, o.last);
                if !o.evidence.is_empty() {
                    let q: Vec<String> = o.evidence.iter().map(|e| format!("\"{e}\"")).collect();
                    b += &format!(" Evidence: {}", q.join("; "));
                }
                b.push('\n');
            }
            b.push('\n');
        }
        b += "\n## Forgotten\n\n";
        for f in &self.forgotten {
            b += &format!("- {f}\n");
        }
        b
    }

    /// Go's `Merge`: checked proposals folded in; how many were added and refined, and why the others were dropped.
    pub fn merge(&mut self, props: &[Proposal], lines: &[Line], gate: Option<&dyn Fn(&str) -> bool>) -> (usize, usize, Vec<String>) {
        let (mut added, mut refined, mut dropped) = (0, 0, Vec::new());
        for p in props {
            let t = trim_space(&p.observation);
            let t = trim_space(t.strip_suffix('.').unwrap_or(t));
            let text = t.replace('—', "-").replace('\n', " ");
            let text = cut_bytes(&text, 200).to_string();
            let Some(line) = grounded(&p.quote, lines) else {
                dropped.push(format!("{}: its quote is not in what they said", quote(&text)));
                continue;
            };
            let q = trim_space(&p.quote).replace('"', "'");
            if let Some(g) = gate {
                if g(&text) || g(&q) {
                    dropped.push(format!("{}: the vault's gate refused it", quote(&text)));
                    continue;
                }
            }
            if self.forgotten.iter().any(|f| similar(f, &text)) {
                dropped.push(format!("{}: the owner asked me to forget this", quote(&text)));
                continue;
            }
            let (y, m, d, ..) = local_time(line.when.unwrap_or_default()).civil();
            let day = format!("{y:04}-{m:02}-{d:02}");
            let mut found = false;
            for o in &mut self.observations {
                if !similar(&o.text, &text) {
                    continue;
                }
                found = true;
                if o.evidence.iter().any(|e| norm(e) == norm(&q)) {
                    break; // the same words seen again are not new evidence
                }
                o.seen = o.seen.wrapping_add(1);
                if o.kind.is_empty() {
                    o.kind = kind_of(&p.kind).into();
                }
                if day > o.last {
                    o.last = day.clone();
                }
                o.evidence.insert(0, q.clone());
                o.evidence.truncate(3);
                refined += 1;
                break;
            }
            if !found {
                self.observations.push(Observation { text, seen: 1, last: day, evidence: vec![q], kind: kind_of(&p.kind).into() });
                added += 1;
            }
        }
        (added, refined, dropped)
    }
}

/// Go's `grounded`: the newest talk line whose owner's words hold the quote (three words at least).
fn grounded<'a>(q: &str, lines: &'a [Line]) -> Option<&'a Line> {
    let q = norm(q);
    if gotext::fields(&q).len() < 3 {
        return None;
    }
    lines.iter().rev().find(|l| norm(&l.said).contains(&q))
}

/// Go's `Proposal`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proposal {
    pub observation: String,
    pub quote: String,
    pub kind: String,
}

static PROPOSAL_TY: Ty =
    Ty::Struct("insight.Proposal", "Proposal", &[Field("observation", &Ty::Str), Field("quote", &Ty::Str), Field("kind", &Ty::Str)]);

/// Go's `ParseProposals`: the JSON objects in a reply (at most 40 looked at).
pub fn parse_proposals(reply: &str) -> Vec<Proposal> {
    let reply = THINK_RE.replace_all(reply, "");
    let mut out = Vec::new();
    for m in OBJ_RE.find_iter(&reply).take(40) {
        let (v, err) = gostruct::unmarshal(&PROPOSAL_TY, m.as_str().as_bytes());
        let p = Proposal {
            observation: gostruct::str_field(&PROPOSAL_TY, &v, "observation").into(),
            quote: gostruct::str_field(&PROPOSAL_TY, &v, "quote").into(),
            kind: gostruct::str_field(&PROPOSAL_TY, &v, "kind").into(),
        };
        if err.is_none() && !trim_space(&p.observation).is_empty() {
            out.push(p);
        }
    }
    out
}

const PROMPT_TEMPLATE: &str = r#"Below are things {name} said to you in conversation, with the date, and what you answered.
Write what these show about {name} as a person: what {he} likes or dislikes, people and pets in {his} life, what {he} is working on or planning, what worries {him}, what makes {him} laugh, how {he} likes to be talked to.
Rules:
- Only what {name} said about {himself}, not what you said.
- Each observation is one short sentence in the third person ("{name} has a cat named Pixel").
- Each needs a quote: {name}'s exact words copied from the lines below, at least three words, that show it.
- Only what the words plainly show. A question {he} asks you shows what {he} wants to know, not a trait: "Do you know my name?" is not self-awareness. Do not guess at feelings {he} did not state.
- Each has a kind: fact (name, work, people, pets, where {he} lives), like, dislike, habit (routines, when and how {he} does things), mood (how {he} feels and what changes it), goal (what {he} is working on or wants).
- Never anything about passwords, keys, account numbers or other secrets.
- At most 8. If nothing is worth keeping, write nothing.
Answer only with one JSON object per line: {"observation": "...", "quote": "...", "kind": "..."}

"#;

/// Go's `prompt`: the template with the owner's name and pronouns.
pub fn prompt() -> String {
    let o = owner_from_env();
    let name = capital(&o.name);
    replace_once(
        PROMPT_TEMPLATE,
        &[("{name}", &name), ("{he}", o.subject), ("{him}", o.object), ("{his}", o.possessive), ("{himself}", o.reflexive)],
    )
}

/// Go's `Ask`.
pub fn ask(ctx: &Ctx, client: &crate::llm::client::Client, lines: &[Line]) -> Result<Vec<Proposal>, String> {
    let mut b = prompt();
    let name = capital(&owner_from_env().name);
    for l in lines {
        b += &format!("{} {name}: {}\n   You: {}\n", local_time(l.when.unwrap_or_default()).format_date_minute(), l.said, l.me);
    }
    let req = Request {
        messages: vec![Message { role: "user".into(), content: b, ..Message::default() }],
        max_tokens: 3000,
        ..Request::default()
    };
    let turn = client.complete(ctx, &req, None).map_err(|e| e.text().to_string())?;
    Ok(parse_proposals(&turn.content))
}

static STATE_TY: Ty = Ty::Struct("insight.state", "state", &[Field("last", &Ty::Time)]);

/// Go's `time.Time` as `json.Marshal` writes it (RFC 3339 with nanoseconds, in the time's own offset).
fn time_json(t: &Time) -> String {
    let (y, m, d, hh, mm, ss, _) = t.civil();
    let mut s = format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}");
    if t.nanos() != 0 {
        s += format!(".{:09}", t.nanos()).trim_end_matches('0');
    }
    let zone = t.offset() / 60;
    if zone == 0 {
        s.push('Z');
    } else {
        let (sign, z) = if zone < 0 { ('-', -zone) } else { ('+', zone) };
        s += &format!("{sign}{:02}:{:02}", z / 60, z % 60);
    }
    s
}

fn save_state(path: &[u8], last: &Time) {
    super::write_state(path, format!("{{\"last\":\"{}\"}}", time_json(last)).as_bytes());
}

/// Go's `maxNoteBytes`.
const MAX_NOTE_BYTES: u64 = 4 << 20;

/// `sarina reflect [--path VAULT] [--dry-run] [--state FILE] [--batch N] [--force]`.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let home = env.home_dir();
    let mut fs = crate::goflag::FlagSet::new("reflect");
    fs.string("path", &env.getenv("SARINA_VAULT"), "the vault folder (default $SARINA_VAULT)");
    fs.bool("dry-run", false, "show what would change; write nothing");
    let state_default = crate::gotext::lossy(&crate::gopath::join(&[home.as_bytes(), b".local", b"state", b"sarina", b"reflect.json"]));
    fs.string("state", &state_default, "where the last reflected talk line is remembered");
    fs.int("batch", 60, "talk lines per model call");
    fs.bool("force", false, "run even while a game is running");
    if fs.parse(args, &mut env.stderr).is_err() {
        env.flush();
        return 2;
    }
    let batch = fs.num("batch");
    if batch < 1 {
        env.eprintln("sarina reflect: --batch must be at least 1");
        return 2;
    }
    let rc = super::not_while_gaming(env, "reflect", fs.is_true("force"));
    if rc != 0 {
        return rc;
    }
    let path = fs.str("path");
    if path.is_empty() {
        env.eprintln("sarina reflect: no vault: give --path or set SARINA_VAULT");
        return 2;
    }
    let v = match crate::vault::Vault::open(&path) {
        Ok(v) => v,
        Err(e) => {
            env.eprintln(&format!("sarina reflect: {e}"));
            return 1;
        }
    };
    let state_path = fs.str("state");
    let mut last = Time::default();
    if let Ok(b) = super::read_state(state_path.as_bytes()) {
        let (v, _) = gostruct::unmarshal(&STATE_TY, &b);
        if let Some(gostruct::Val::Time(t)) = gostruct::field(&STATE_TY, &v, "last") {
            last = *t;
        }
    }
    let (lines, echoes) = super::without_echoes(super::read_talks(v.root.as_bytes(), &last));
    for e in &echoes {
        env.println(&format!("skipped (my own voice heard back): {}", quote(&e.said)));
    }
    let when = |l: &Line| local_time(l.when.unwrap_or_default());
    if lines.is_empty() {
        env.println("nothing new to reflect on");
        if let Some(e) = echoes.last() {
            save_state(state_path.as_bytes(), &when(e));
        }
        return 0;
    }
    let getenv = env.getenv_owned();
    let client = crate::llm::client::Client::new(crate::config::model_from_env(&*getenv));
    // the note as it is: only a regular file is read, before the model is asked
    let name = super::note_name();
    let note_path = crate::gopath::join(&[v.root.as_bytes(), name.as_bytes()]);
    let mut old = Vec::new();
    let mut exists = false;
    if let Ok(st) = std::fs::symlink_metadata(crate::gopath::os_path(&note_path)) {
        if !st.file_type().is_file() {
            env.eprintln(&format!("sarina reflect: {name} is not a regular file; not read"));
            return 1;
        }
        match crate::session::readfile::read_regular(&note_path, MAX_NOTE_BYTES) {
            Ok(b) => old = b,
            Err(e) => {
                env.eprintln(&format!("sarina reflect: {e}"));
                return 1;
            }
        }
        exists = true;
    }
    let mut n = parse_note(&crate::gotext::lossy(&old));
    let scan = |t: &str| {
        let getenv = |k: &str| std::env::var(k).unwrap_or_default();
        crate::guard::Gate::memory().scan(t.as_bytes(), &getenv).blocked
    };
    let ctx = Ctx::with_deadline(std::time::Instant::now() + std::time::Duration::from_secs(20 * 60));
    let (mut added, mut refined) = (0, 0);
    let size = usize::try_from(batch).unwrap_or(usize::MAX);
    for part in lines.chunks(size) {
        let props = match ask(&ctx, &client, part) {
            Ok(p) => p,
            Err(e) => {
                env.eprintln(&format!("sarina reflect: the model did not answer: {e}"));
                return 1; // the state is not moved on, so these lines are read again next time
            }
        };
        let (a, r, dropped) = n.merge(&props, part, Some(&scan));
        added += a;
        refined += r;
        for d in dropped {
            env.println(&format!("dropped {d}"));
        }
    }
    env.println(&format!("{} talk lines read: {added} new observations, {refined} seen again", lines.len()));
    if fs.is_true("dry-run") {
        env.stdout.extend_from_slice(format!("\n{}", n.render()).as_bytes());
        env.flush();
        return 0;
    }
    if added + refined > 0 {
        let mode = if exists { "replace" } else { "create" };
        let gate = |t: &str| {
            let getenv = |k: &str| std::env::var(k).unwrap_or_default();
            let v = crate::guard::Gate::memory().scan(t.as_bytes(), &getenv);
            v.blocked.then_some(v.reason)
        };
        if let Err(e) = v.write(Some(&gate), &name, &n.render(), mode, &crate::vault::write::local_now().0) {
            env.eprintln(&format!("sarina reflect: {e}"));
            return 1;
        }
        v.commit("what I learned from our talks");
    }
    let mut last = when(&lines[lines.len() - 1]);
    if let Some(e) = echoes.last() {
        if when(e) > last {
            last = when(e);
        }
    }
    save_state(state_path.as_bytes(), &last);
    0
}
