//! Go: `go/daily`: the day's note in the vault (Daily/YYYY-MM-DD.md: the check-up's lines, a status section, a numbers
//! line per probe) and `sarina trends`. Ported part by part, each proved against Go's own answers
//! (testdata/parity/daily.json): the note's sections ([`parse`], [`Note::render`]), the numbers line ([`parse_numbers`],
//! [`Numbers::line`]), [`read_days`] and [`trends`]; probing the computer and `sarina daily-report` are next.

use crate::cryptotools::fmt_f;
use crate::gopath;
use crate::gotext::{self, lossy};
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// Go's `Numbers`: one probe's numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Numbers {
    /// HH:MM
    pub at: String,
    /// mount point -> percent used
    pub disks: BTreeMap<String, i64>,
    pub crashes: i64,
    /// the program that crashed most today ("" when none)
    pub top: String,
    pub top_n: i64,
    pub failed: i64,
    pub mem_pct: i64,
    pub load: f64,
}

static NUMBERS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    crate::goregexp::compile(
        r"^- (\d{2}:\d{2}) \| disk (.*?) \| crashes (\d+)(?: \((.+?) (\d+)\))? \| failed (\d+) \| memory (\d+)% \| load ([\d.]+)$",
    )
    .expect("numbersRx")
});

static STATUS_HEAD_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| crate::goregexp::compile(r"^## Status \((\d{2}:\d{2})\)\s*$").expect("statusHeadRx"));

static DAY_FILE_RE: LazyLock<regex::Regex> = LazyLock::new(|| crate::goregexp::compile(r"^(\d{4}-\d{2}-\d{2})\.md$").expect("dayFileRx"));

/// Go's `strconv.Atoi` with its error ignored: the number, 0 for anything that is not one, the nearest int64 for one out
/// of range.
fn atoi(s: &str) -> i64 {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return 0;
    }
    s.parse::<i64>().unwrap_or(if s.starts_with('-') { i64::MIN } else { i64::MAX })
}

/// Go's `ParseNumbers`: a numbers line read back.
pub fn parse_numbers(line: &str) -> Option<Numbers> {
    let m = NUMBERS_RE.captures(gotext::trim_space(line))?;
    let g = |i: usize| m.get(i).map_or("", |x| x.as_str());
    let mut n = Numbers { at: g(1).into(), top: g(4).into(), ..Numbers::default() };
    for d in g(2).split(", ") {
        if let Some((mnt, pct)) = d.split_once(' ') {
            n.disks.insert(mnt.to_string(), atoi(pct.strip_suffix('%').unwrap_or(pct)));
        }
    }
    n.crashes = atoi(g(3));
    n.top_n = atoi(g(5));
    n.failed = atoi(g(6));
    n.mem_pct = atoi(g(7));
    n.load = gotext::parse_float(g(8)).unwrap_or(0.0);
    Some(n)
}

impl Numbers {
    /// Go's `Line`.
    pub fn line(&self) -> String {
        let d: Vec<String> = self.disks.iter().map(|(m, p)| format!("{m} {p}%")).collect();
        let mut crash = self.crashes.to_string();
        if !self.top.is_empty() {
            crash += &format!(" ({} {})", self.top, self.top_n);
        }
        format!(
            "- {} | disk {} | crashes {crash} | failed {} | memory {}% | load {}",
            self.at,
            d.join(", "),
            self.failed,
            self.mem_pct,
            fmt_f(self.load, 1)
        )
    }
}

/// Go's `Note`: a day's note in its parts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    /// YYYY-MM-DD
    pub day: String,
    pub head: Vec<String>,
    /// HH:MM of the status section, "" when there is none
    pub status_at: String,
    pub status: Vec<String>,
    pub numbers: Vec<String>,
}

/// Go's `header`: a new day note's front matter and title.
fn header(day: &str) -> Vec<String> {
    format!("---\ncreated: {day}\nsource: labyrinth-sarina-checkup\ntags: [daily]\n---\n\n# {day}\n")
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// Go's `Parse`.
pub fn parse(day: &str, text: &str) -> Note {
    let mut n = Note { day: day.into(), ..Note::default() };
    let mut section = "head";
    for l in text.split('\n') {
        let t = gotext::trim_space(l);
        if let Some(m) = STATUS_HEAD_RE.captures(t) {
            section = "status";
            n.status_at = m[1].to_string();
            continue;
        }
        if t == "## Numbers" {
            section = "numbers";
            continue;
        }
        if t.starts_with("## ") {
            section = "head";
        }
        match section {
            "status" => {
                if !t.is_empty() {
                    n.status.push(l.to_string());
                }
            }
            "numbers" => {
                if parse_numbers(t).is_some() {
                    n.numbers.push(t.to_string());
                } else if !t.is_empty() {
                    n.head.push(l.to_string());
                }
            }
            _ => n.head.push(l.to_string()),
        }
    }
    while n.head.last().is_some_and(|l| gotext::trim_space(l).is_empty()) {
        n.head.pop();
    }
    if n.head.is_empty() {
        n.head = header(day);
    }
    n
}

impl Note {
    /// Go's `Render`.
    pub fn render(&self) -> String {
        let mut b = self.head.join("\n") + "\n";
        if !self.status.is_empty() {
            b += &format!("\n## Status ({})\n\n{}\n", self.status_at, self.status.join("\n"));
        }
        if !self.numbers.is_empty() {
            b += &format!("\n## Numbers\n\n{}\n", self.numbers.join("\n"));
        }
        b
    }
}

/// Go's `Day`.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: String,
    pub numbers: Numbers,
}

/// Go's `ReadDays`: each day's last numbers (with the day's highest crash count), oldest first, the last `days` of them
/// (all for 0 or less); the notes read as bounded regular files.
pub fn read_days(vault: &str, days: i64) -> Vec<Day> {
    let dir = gopath::join(&[vault.as_bytes(), b"Daily"]);
    let mut names: Vec<Vec<u8>> = gopath::read_dir(&dir)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.name)
        .filter(|n| gopath::path_match(b"20*.md", n).unwrap_or(false))
        .collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let name = lossy(&name);
        let Some(m) = DAY_FILE_RE.captures(&name) else { continue };
        let Ok(b) = crate::session::readfile::read_regular(&gopath::join(&[&dir, name.as_bytes()]), 1 << 20) else { continue };
        let n = parse(&m[1], &lossy(&b));
        let Some(mut last) = n.numbers.last().and_then(|l| parse_numbers(l)) else { continue };
        for l in &n.numbers {
            if let Some(x) = parse_numbers(l) {
                if x.crashes > last.crashes {
                    (last.crashes, last.top, last.top_n) = (x.crashes, x.top, x.top_n);
                }
            }
        }
        out.push(Day { date: m[1].to_string(), numbers: last });
    }
    if days > 0 && out.len() as i64 > days {
        out.drain(..out.len() - days as usize);
    }
    out
}

/// Go's `fmt.Sprintf("%-w s", s)` and `"%w s"`: padded by characters.
fn pad(s: &str, w: usize, left: bool) -> String {
    let n = s.chars().count();
    if n >= w {
        return s.to_string();
    }
    let sp = " ".repeat(w - n);
    if left {
        format!("{s}{sp}")
    } else {
        format!("{sp}{s}")
    }
}

/// Go's `Trends`: a table of the days and what stands out (disks nearly full or filling up, programs that crash on
/// three days or more, failed services).
pub fn trends(days: &[Day]) -> String {
    if days.is_empty() {
        return "No daily numbers yet: they are kept from the first `sarina daily-report` or check-up on.".into();
    }
    let mut ms: Vec<&str> = days.iter().flat_map(|d| d.numbers.disks.keys().map(String::as_str)).collect();
    ms.sort();
    ms.dedup();
    let mut b = pad("day", 10, true);
    for m in &ms {
        b += &format!(" {}", pad(m, 6, false));
    }
    b += "  crashes  failed  memory  load\n";
    for d in days {
        let n = &d.numbers;
        b += &pad(&d.date, 10, true);
        for m in &ms {
            match n.disks.get(*m) {
                Some(p) => b += &format!(" {}%", pad(&p.to_string(), 5, false)),
                None => b += &format!(" {}", pad("-", 6, false)),
            }
        }
        let mut crash = n.crashes.to_string();
        if !n.top.is_empty() {
            crash += &format!(" ({})", n.top);
        }
        b += &format!(
            "  {} {} {}%  {}\n",
            pad(&crash, 8, true),
            pad(&n.failed.to_string(), 7, true),
            pad(&n.mem_pct.to_string(), 5, false),
            fmt_f(n.load, 1)
        );
    }
    let mut notes = Vec::new();
    let (first, last) = (&days[0].numbers, &days[days.len() - 1].numbers);
    for m in &ms {
        let (a, z) = (first.disks.get(*m), last.disks.get(*m));
        match (a, z) {
            (_, Some(&z)) if z >= 90 => notes.push(format!("{m} is {z}% full: nearly full")),
            (Some(&a), Some(&z)) if days.len() > 1 && z.wrapping_sub(a) >= 5 => {
                notes.push(format!("{m} went from {a}% to {z}% over {} day(s): filling up", days.len()))
            }
            _ => {}
        }
    }
    let mut by_prog: BTreeMap<&str, usize> = BTreeMap::new();
    for d in days {
        if !d.numbers.top.is_empty() {
            *by_prog.entry(&d.numbers.top).or_insert(0) += 1;
        }
    }
    let mut progs: Vec<(&str, usize)> = by_prog.into_iter().collect();
    progs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    for (p, n) in progs {
        if n >= 3 {
            notes.push(format!("{p} crashed on {n} of {} day(s): a recurring problem, worth a look (sarina crash-detail {p})", days.len()));
        }
    }
    if last.failed > 0 {
        notes.push(format!("{} service(s) failed on the last day", last.failed));
    }
    if notes.is_empty() {
        notes.push("nothing stands out".into());
    }
    b + &format!("\nWhat stands out: {}.\n", notes.join("; "))
}

/// `sarina trends [--path VAULT] [--days N]`.
pub fn trends_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("trends");
    fs.string("path", &env.getenv("SARINA_VAULT"), "the vault folder (default $SARINA_VAULT)");
    fs.int("days", 30, "how many days back");
    if fs.parse(args, &mut env.stderr).is_err() {
        env.flush();
        return 2;
    }
    let path = fs.str("path");
    if path.is_empty() {
        env.eprintln("sarina trends: no vault: give --path or set SARINA_VAULT");
        return 2;
    }
    env.stdout.extend_from_slice(trends(&read_days(&path, fs.num("days"))).as_bytes());
    env.flush();
    0
}
