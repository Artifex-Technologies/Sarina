//! Go: `go/insight`: what Sarina learns about the person she talks with, from the talk log (Talks/YYYY-MM.md in the
//! vault). Ported part by part, each proved against Go (the CLI differential): the talk log's lines, telling her own voice
//! heard back ([`is_echo`]) and small talk ([`small_talk`]) apart, and `sarina vault-tidy` ([`tidy`], [`tidy_command`]).
//! `vault-ingest` and `reflect` are next.

use crate::gopath;
use crate::gotext::{self, lossy};
use std::sync::LazyLock;

/// Go's `Line`: one exchange of the talk log.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// local time in Unix seconds; None when the log's time is not one
    pub when: Option<i64>,
    /// what the owner said
    pub said: String,
    /// what she answered
    pub me: String,
}

fn re(go: &str) -> regex::Regex {
    crate::goregexp::compile(go).expect("a fixed pattern")
}

static TALK_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"^- (\d{4}-\d{2}-\d{2} \d{2}:\d{2}) \| they said: (.*?) \| I said: (.*)$"));

static OWN_VOICE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(i('m| am) (right )?here whenever|standing by whenever|whenever you('re| are| would like| want)|dive back into|your custom ai|",
        r"ready to (tackle|help you|assist)|feel free to ask|let me know (if|when|how)|how can i (help|assist)|is there anything (else )?i can)\b"
    ))
});

static KEEP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(haha|hahaha|lol|lmao|rofl|funny|hilarious|cracked me up|i (really )?(like|love|hate|prefer|want|wish|enjoy|miss)|",
        r"my (favou?rite|cat|dog|pet|wife|husband|partner|friend|brother|sister|mom|mum|dad|son|daughter|kid|boss|job|work|project|book|game)|",
        r"(decided|let'?s go with|i'?ll go with|we'?ll|plan(ning)? to|going to|remember that|don'?t forget|call me|my name)\b|😂|🤣|😆)"
    ))
});

static SMALL_TALK_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)^(hey|hi|hello|yo|good (morning|afternoon|evening|night)|thanks?( you)?( so much| a lot)?|thank you|ok(ay)?|sure|yes|no|yep|nope|alright|cool|nice|sounds good|got it|",
        r"bye|goodbye|see you( later)?|take care|what time is it|how are (you|things)( doing| looking| going)?|how'?s it going|how is it going|how'?s (the )?(system|computer|everything)( doing| looking)?|",
        r"(just )?(wanted to )?(check(ing)? (in|and see)|see) how things (were|are) (looking|going|doing)|(is )?everything (ok|okay|fine|good)|you there|are you there|testing)\W*(sarina\W*)?$"
    ))
});

static FILLER_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| re(r"^((yeah|yes|yep|ok|okay|oh|well|alright|so|um|uh|hey|hi|hello|sarina|please)[,!.]?\s+)+"));
static TRAILER_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| re(r"(\s+(too|again|today|there|though|for you|with you|sarina|please|now))*[.!?, ]*$"));
static STATUS_ANSWER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(your (network|audio|system|computer)|everything('s| is) (looking good|fine|stable)|all (good|fine)|no (hiccups|issues|problems))",
    )
});

/// Go's `OwnerFromEnv().Name`: SARINA_OWNER_NAME, or "the owner" when it is unset or looks like a path.
fn owner_name() -> String {
    let n = std::env::var("SARINA_OWNER_NAME").unwrap_or_default();
    let n = gotext::trim_space(&n);
    if n.is_empty() || n.contains(['/', '\\', '.']) {
        return "the owner".into();
    }
    n.to_string()
}

/// Go's `norm`: lower case, curly quotes made plain or dropped, only a-z, 0-9 and apostrophes, one space between words.
pub fn norm(s: &str) -> String {
    let s = gotext::to_lower(s).replace(['’', '‘'], "'").replace(['“', '”', '"'], "");
    s.split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '\'')).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

const STOP: [&str; 26] = [
    "the", "a", "an", "and", "or", "of", "to", "in", "is", "are", "they", "their", "them", "has", "have", "with", "for", "on", "at",
    "likes", "like", "it", "that", "was", "be", "about",
];

/// Go's `words`: the words that say something (no stop words, longer than two letters, not the owner's name), without
/// a plural s.
fn words(s: &str) -> std::collections::HashSet<String> {
    let owner = norm(&owner_name());
    norm(s)
        .split(' ')
        .filter(|w| !STOP.contains(w) && w.len() > 2 && *w != owner)
        .map(|w| w.strip_suffix('s').unwrap_or(w).to_string())
        .collect()
}

/// Go's `similar`: most of the shorter text's words are in the other.
fn similar(a: &str, b: &str) -> bool {
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return false;
    }
    let both = wa.iter().filter(|w| wb.contains(*w)).count();
    both as f64 >= 0.6 * wa.len().min(wb.len()) as f64
}

/// Go's `IsEcho`: what "they said" is her own voice heard back (her phrases, or most of one of her last six answers).
pub fn is_echo(said: &str, previous: &[Line]) -> bool {
    let s = gotext::trim_space(said);
    if s.is_empty() {
        return false;
    }
    if OWN_VOICE_RE.is_match(s) {
        return true;
    }
    previous[previous.len().saturating_sub(6)..].iter().any(|p| gotext::fields(&norm(&p.me)).len() >= 4 && similar(&p.me, s))
}

/// Go's `mentionsOwner`.
fn mentions_owner(said: &str) -> bool {
    let name = gotext::to_lower(gotext::trim_space(&std::env::var("SARINA_OWNER_NAME").unwrap_or_default()));
    if name.is_empty() {
        return false;
    }
    gotext::to_lower(said)
        .split(|c: char| !gotext::is_letter(c) && c != '\'')
        .filter(|w| !w.is_empty())
        .any(|w| w.strip_suffix("'s").unwrap_or(w) == name)
}

/// Go's `SmallTalk`: a greeting, thanks or status check with nothing to learn from (twelve words at most), or a short
/// line her status answer followed.
pub fn small_talk(l: &Line) -> bool {
    let said = gotext::trim_space(&l.said);
    if KEEP_RE.is_match(said) || mentions_owner(said) {
        return false;
    }
    let s = FILLER_RE.replace_all(&gotext::to_lower(said), "").into_owned();
    let s = gotext::trim_space(&TRAILER_RE.replace_all(&s, "")).to_string();
    let n = gotext::fields(&s).len();
    if n > 12 {
        return false;
    }
    if SMALL_TALK_RE.is_match(&s) || SMALL_TALK_RE.is_match(said) {
        return true;
    }
    n <= 6 && STATUS_ANSWER_RE.is_match(&l.me)
}

/// Go's `time.ParseInLocation("2006-01-02 15:04", s, time.Local)` as Unix seconds (None for a date or time that is not
/// one).
fn parse_local(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mo) || d < 1 || d > days_in[(mo - 1) as usize] || h > 23 || mi > 59 {
        return None;
    }
    // days from the civil date (Howard Hinnant's algorithm)
    let (yy, mm) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * mm + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let civil = days * 86400 + h * 3600 + mi * 60;
    let off = i64::from(crate::tz::local_offset(civil));
    let utc = civil - off;
    let off2 = i64::from(crate::tz::local_offset(utc));
    Some(if off2 == off { utc } else { civil - off2 })
}

/// Go's `Tidy`: the log's lines kept, and the ones dropped with why: her own voice heard back (at any age), small talk
/// older than `older_than_ns`.
pub fn tidy(text: &str, now_ns: i128, older_than_ns: i64) -> (Vec<String>, Vec<String>, Vec<&'static str>) {
    let (mut kept, mut dropped, mut why) = (Vec::new(), Vec::new(), Vec::new());
    let mut seen: Vec<Line> = Vec::new();
    for raw in text.split('\n') {
        let Some(m) = TALK_RE.captures(gotext::trim_space(raw)) else {
            kept.push(raw.to_string());
            continue;
        };
        let l = Line { when: parse_local(&m[1]), said: m[2].to_string(), me: m[3].to_string() };
        // Go's now.Sub(t): saturated at the largest duration (a time that is not one is the year 1)
        let age = match l.when {
            Some(t) => (now_ns - i128::from(t) * 1_000_000_000).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
            None => i64::MAX,
        };
        if is_echo(&l.said, &seen) {
            dropped.push(raw.to_string());
            why.push("my own voice heard back");
        } else if age > older_than_ns && small_talk(&l) {
            dropped.push(raw.to_string());
            why.push("small talk");
        } else {
            kept.push(raw.to_string());
        }
        seen.push(l);
    }
    (kept, dropped, why)
}

/// `sarina vault-tidy [--path VAULT] [--older-than DAYS] [--yes] [--dry-run]`.
pub fn tidy_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("vault-tidy");
    fs.string("path", &env.getenv("SARINA_VAULT"), "the vault folder (default $SARINA_VAULT)");
    fs.int("older-than", 30, "small talk older than this many days may go (her own voice heard back goes at any age)");
    fs.bool("yes", false, "do not ask");
    fs.bool("dry-run", false, "show the lines; change nothing");
    if fs.parse(args, &mut env.stderr).is_err() {
        env.flush();
        return 2;
    }
    let path = fs.str("path");
    if path.is_empty() {
        env.eprintln("sarina vault-tidy: no vault: give --path or set SARINA_VAULT");
        return 2;
    }
    let v = match crate::vault::Vault::open(&path) {
        Ok(v) => v,
        Err(e) => {
            env.eprintln(&format!("sarina vault-tidy: {e}"));
            return 1;
        }
    };
    let dir = gopath::join(&[v.root.as_bytes(), b"Talks"]);
    let mut names: Vec<Vec<u8>> = gopath::read_dir(&dir)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.name)
        .filter(|n| gopath::path_match(b"20*.md", n).unwrap_or(false))
        .collect();
    names.sort();
    // Go's time.Duration(days) * 24 * time.Hour, wrapping as Go's does
    let older = fs.num("older-than").wrapping_mul(24).wrapping_mul(3_600_000_000_000);
    let now_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i128);
    let mut changes: Vec<(Vec<u8>, String)> = Vec::new();
    let mut total = 0;
    for name in names {
        let f = gopath::join(&[&dir, &name]);
        // only a regular file in Talks/ is tidied: a link there is not followed, a named pipe not waited on
        if !std::fs::symlink_metadata(gopath::os_path(&f)).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Ok(b) = crate::session::readfile::read_regular(&f, 4 << 20) else { continue };
        let (kept, dropped, why) = tidy(&lossy(&b), now_ns, older);
        if dropped.is_empty() {
            continue;
        }
        let rel = lossy(&gopath::rel(v.root.as_bytes(), &f).unwrap_or_default());
        let mut o = format!("{rel}: {} line(s) would go\n", dropped.len());
        for (d, w) in dropped.iter().zip(&why) {
            let line = gotext::trim_space(d);
            let line = if line.chars().count() > 140 { line.chars().take(140).collect::<String>() + "..." } else { line.to_string() };
            o += &format!("  - ({w}) {line}\n");
        }
        env.stdout.extend_from_slice(o.as_bytes());
        env.flush();
        total += dropped.len();
        changes.push((f, kept.join("\n")));
    }
    if total == 0 {
        env.println("nothing to tidy");
        return 0;
    }
    if fs.is_true("dry-run") {
        return 0;
    }
    if !fs.is_true("yes") {
        env.stdout
            .extend_from_slice(format!("Remove these {total} line(s)? The vault is in git, so this can be undone. [y/N] ").as_bytes());
        env.flush();
        let mut answer = Vec::new();
        {
            use std::io::Read;
            let mut byte = [0u8; 1];
            let mut stdin = std::io::stdin().lock();
            while let Ok(1) = stdin.read(&mut byte) {
                answer.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
        }
        let a = gotext::to_lower(gotext::trim_space(&lossy(&answer)));
        if a != "y" && a != "yes" {
            env.println("nothing changed");
            return 0;
        }
    }
    for (f, text) in &changes {
        if let Err(e) = crate::testguard::allow(f).and_then(|()| crate::goos::write_file(f, text.as_bytes(), 0o644)) {
            env.eprintln(&format!("sarina vault-tidy: {e}"));
            return 1;
        }
    }
    v.commit(&format!("talk log tidied: {total} line(s) of small talk or my own voice removed"));
    env.println(&format!("removed {total} line(s)"));
    0
}
