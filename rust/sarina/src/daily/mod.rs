//! Go: `go/daily`: the day's note in the vault (Daily/YYYY-MM-DD.md: the check-up's lines, a status section, a numbers
//! line per probe) and `sarina trends`. Ported part by part, each proved against Go's own answers
//! (testdata/parity/daily.json): the note's sections ([`parse`], [`Note::render`]), the numbers line ([`parse_numbers`],
//! [`Numbers::line`]), [`read_days`] and [`trends`]; probing the computer ([`probe`], [`Status::render`]) and
//! `sarina daily-report` (the note loaded, brought up to date and saved; proved by the CLI differential).

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

// ---- probing the computer and the day's note ------------------------------------------------------------------------

/// Go's `Disk`.
#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub mount: String,
    pub pct: i64,
    pub free: i64,
}

/// Go's `Status`: one probe of the computer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// local time: (date, HH:MM)
    pub at: (String, String),
    pub cores: i64,
    pub load: f64,
    pub mem_used: i64,
    pub mem_total: i64,
    pub swap_used: i64,
    pub disks: Vec<Disk>,
    pub failed: Vec<String>,
    pub crashes: BTreeMap<String, i64>,
    pub security: String,
    pub kernel: i64,
}

/// Go's `runtime.NumCPU`: the processors this process may run on.
fn num_cpu() -> i64 {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) == 0 {
            return i64::from(libc::CPU_COUNT(&set));
        }
    }
    1
}

/// Go's `run`: a program's output (the first MiB; "" when it failed and printed nothing), 10 s at most and a child left
/// holding its output not waited for longer than 2 s.
fn run(ctx: &crate::goctx::Ctx, args: &[&str]) -> String {
    let mut out = Vec::new();
    if ctx.err().is_some() {
        return String::new();
    }
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    if let Some(d) = ctx.deadline() {
        deadline = deadline.min(d);
    }
    let mut sink = |stream: &'static str, b: &[u8]| {
        if stream == "stdout" {
            let room = (1usize << 20).saturating_sub(out.len());
            out.extend_from_slice(&b[..b.len().min(room)]);
        }
    };
    let rest: Vec<String> = args[1..].iter().map(|a| a.to_string()).collect();
    let env = crate::tools::exec::parent_env();
    let failed =
        match crate::tools::exec::run_group(args[0], &rest, &env, None, deadline, ctx, crate::tools::exec::GroupKill::Inherit, &mut sink) {
            Err(_) => true,
            Ok(end) => !end.status.is_some_and(|s| s.success()) || end.io_incomplete,
        };
    if failed && out.is_empty() {
        return String::new();
    }
    lossy(&out)
}

/// Go's `mounts`: the real file systems worth watching (/, /home, /boot, /boot/efi, /mnt/..., /run/media/...), a btrfs
/// once.
fn mounts() -> Vec<String> {
    let Ok(b) = std::fs::read("/proc/self/mounts") else { return vec!["/".into()] };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for l in lossy(&b).split('\n') {
        let f = gotext::fields(l);
        if f.len() < 3 {
            continue;
        }
        let (dev, mnt, typ) = (f[0], f[1], f[2]);
        if !dev.starts_with("/dev/") || typ == "squashfs" || typ == "iso9660" {
            continue;
        }
        if !(matches!(mnt, "/" | "/home" | "/boot" | "/boot/efi") || mnt.starts_with("/mnt/") || mnt.starts_with("/run/media/")) {
            continue;
        }
        let key = if typ == "btrfs" { dev.to_string() } else { format!("{dev} {mnt}") };
        if seen.insert(key) {
            out.push(mnt.to_string());
        }
    }
    if out.is_empty() {
        out.push("/".into());
    }
    out
}

/// Go's `Probe`: load, memory, disks, failed services (system and session), today's crashes of this user's programs,
/// kernel errors today, Heimdall's headline.
pub fn probe(ctx: &crate::goctx::Ctx) -> Status {
    let (date, hm) = crate::vault::write::local_now();
    let mut s = Status { at: (date, hm), cores: num_cpu(), ..Status::default() };
    if let Ok(b) = std::fs::read("/proc/loadavg") {
        if let Some(f) = gotext::fields(&lossy(&b)).first() {
            s.load = gotext::parse_float(f).unwrap_or(0.0);
        }
    }
    if let Ok(b) = std::fs::read("/proc/meminfo") {
        let text = lossy(&b);
        let kb = |key: &str| -> i64 {
            for l in text.split('\n') {
                if l.starts_with(&format!("{key}:")) {
                    let f = gotext::fields(l);
                    if f.len() >= 2 {
                        return f[1].parse::<i64>().unwrap_or(0).wrapping_mul(1024);
                    }
                }
            }
            0
        };
        s.mem_total = kb("MemTotal");
        s.mem_used = s.mem_total - kb("MemAvailable");
        s.swap_used = kb("SwapTotal") - kb("SwapFree");
    }
    for m in mounts() {
        let Ok(c) = std::ffi::CString::new(m.clone()) else { continue };
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 || st.f_blocks == 0 {
            continue;
        }
        let used = st.f_blocks - st.f_bfree;
        let total = used + st.f_bavail;
        if total == 0 {
            continue;
        }
        s.disks.push(Disk { mount: m, pct: (used * 100 / total) as i64, free: (st.f_bavail as i64).wrapping_mul(st.f_bsize) });
    }
    for scope in [None, Some("--user")] {
        let mut args = vec!["systemctl"];
        args.extend(scope);
        args.extend(["--failed", "--no-legend", "--plain", "--no-pager"]);
        for l in run(ctx, &args).split('\n') {
            if let Some(unit) = gotext::fields(l).first().filter(|u| !u.starts_with("run-")) {
                s.failed.push(if scope.is_some() { format!("{unit} (your session)") } else { unit.to_string() });
            }
        }
    }
    let uid = unsafe { libc::getuid() }.to_string();
    for l in run(ctx, &["coredumpctl", "list", "--no-pager", "--no-legend", "--since=today"]).split('\n') {
        let f = gotext::fields(l);
        if f.len() >= 10 && f[5] == uid {
            *s.crashes.entry(lossy(&gopath::base(f[9].as_bytes()))).or_insert(0) += 1;
        }
    }
    s.kernel = run(ctx, &["journalctl", "-k", "-p", "err", "--since=today", "--no-pager", "-q", "-o", "cat"])
        .split('\n')
        .filter(|l| !gotext::trim_space(l).is_empty())
        .count() as i64;
    s.security = crate::sysinfo::security::security_headline(ctx);
    s
}

/// Go's `gb`.
fn gb(b: i64) -> String {
    format!("{} GB", fmt_f(b as f64 / 1e9, 0))
}

impl Status {
    /// Go's `Render`: the status section and the probe's numbers.
    pub fn render(&self) -> (String, Numbers) {
        let mut n = Numbers { at: self.at.1.clone(), failed: self.failed.len() as i64, load: self.load, ..Numbers::default() };
        if self.mem_total > 0 {
            n.mem_pct = self.mem_used.wrapping_mul(100) / self.mem_total;
        }
        let mut b = format!("## Status ({})\n\n", self.at.1);
        b += &format!("- Load: {} on {} cores", fmt_f(self.load, 1), self.cores);
        let cores = self.cores as f64;
        b += if self.cores > 0 && self.load > cores {
            " (busy: more work than cores)"
        } else if self.cores > 0 && self.load > cores / 2.0 {
            " (working)"
        } else {
            " (idle)"
        };
        b += "\n";
        if self.mem_total > 0 {
            b += &format!("- Memory: {} of {} used ({}%)", gb(self.mem_used), gb(self.mem_total), n.mem_pct);
            if self.swap_used > 0 {
                b += &format!(", swap {} in use", gb(self.swap_used));
            }
            b += "\n";
        }
        if !self.disks.is_empty() {
            let mut parts = Vec::new();
            for d in &self.disks {
                n.disks.insert(d.mount.clone(), d.pct);
                let note = if d.pct >= 90 { " NEARLY FULL" } else { "" };
                parts.push(format!("{} {}% used ({} free){note}", d.mount, d.pct, gb(d.free)));
            }
            b += &format!("- Disks: {}\n", parts.join("; "));
        }
        if self.failed.is_empty() {
            b += "- Services: none failed\n";
        } else {
            b += &format!("- Services failed: {}\n", self.failed.join(", "));
        }
        if self.crashes.is_empty() {
            b += "- Crashes today: none\n";
        } else {
            let mut names: Vec<(&String, &i64)> = self.crashes.iter().collect();
            names.sort_by(|a, c| c.1.cmp(a.1).then(a.0.cmp(c.0)));
            let parts: Vec<String> = names.iter().map(|(k, v)| format!("{k} {v}")).collect();
            n.crashes = names.iter().map(|(_, v)| **v).sum();
            (n.top, n.top_n) = (names[0].0.clone(), *names[0].1);
            b += &format!("- Crashes today: {} ({})\n", n.crashes, parts.join(", "));
        }
        if self.kernel > 0 {
            b += &format!("- Kernel errors today: {}\n", self.kernel);
        }
        if !self.security.is_empty() {
            b += &format!("- Security: {}\n", self.security);
        }
        (b, n)
    }
}

impl Note {
    /// Go's `Fresh`: the note's status is today's and at most `max_age_min` minutes old (`now`: the local date, and the
    /// time of day in seconds).
    pub fn fresh(&self, now: (&str, i64), max_age_min: i64) -> bool {
        if self.status_at.is_empty() || max_age_min <= 0 || self.day != now.0 {
            return false;
        }
        let Some((h, m)) = self.status_at.split_once(':') else { return false };
        let (Ok(h), Ok(m)) = (h.parse::<i64>(), m.parse::<i64>()) else { return false };
        if !(0..24).contains(&h) || !(0..60).contains(&m) {
            return false;
        }
        let at = h * 3600 + m * 60;
        at <= now.1 && now.1 - at <= max_age_min.saturating_mul(60)
    }

    /// Go's `SetStatus`: the status section replaced by the probe's, its numbers line added (the last 48 kept).
    pub fn set_status(&mut self, s: &Status) {
        let (section, numbers) = s.render();
        let lines: Vec<&str> = gotext::trim_space(&section).split('\n').collect();
        self.status_at = s.at.1.clone();
        self.status = lines[1..].iter().filter(|l| !gotext::trim_space(l).is_empty()).map(|l| l.to_string()).collect();
        self.numbers.push(numbers.line());
        if self.numbers.len() > 48 {
            self.numbers.drain(..self.numbers.len() - 48);
        }
    }

    /// Go's `Print`: the status as the report shows it, with the day's check-up lines.
    pub fn print(&self, from_note: bool) -> String {
        let src = if from_note { "from the day's note, no new probe" } else { "just now" };
        let mut b =
            format!("Status of the computer as of {} today ({src}; read-only facts, say what they show in plain words):\n", self.status_at);
        for l in &self.status {
            b += &format!("{l}\n");
        }
        let checkups: Vec<&str> =
            self.head.iter().map(|l| gotext::trim_space(l)).filter(|t| t.starts_with("- ") && t.contains("check-up:")).collect();
        if !checkups.is_empty() {
            b += &format!("Today's check-up notes:\n{}\n", checkups.join("\n"));
        }
        b
    }
}

/// Go's `Path`: Daily/YYYY-MM-DD.md in the vault.
pub fn path(vault: &str, date: &str) -> Vec<u8> {
    gopath::join(&[vault.as_bytes(), b"Daily", format!("{date}.md").as_bytes()])
}

/// Go's `Load`: the day's note (a bounded regular file; an empty one when there is none).
pub fn load(vault: &str, date: &str) -> Note {
    let b = crate::session::readfile::read_regular(&path(vault, date), 1 << 20).unwrap_or_default();
    parse(date, &lossy(&b))
}

/// Go's `Save`: the note written anew (never through a link planted under its copy's name).
pub fn save(vault: &str, n: &Note) -> Result<(), String> {
    let p = gopath::join(&[vault.as_bytes(), b"Daily", format!("{}.md", n.day).as_bytes()]);
    crate::testguard::allow(&p)?;
    crate::goos::mkdir_all(&gopath::dir(&p))?;
    let ps = lossy(&p);
    let tmp = format!("{ps}.tmp");
    let _ = std::fs::remove_file(&tmp);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&tmp)
            .map_err(|e| crate::goerr::path("open", tmp.as_bytes(), &e))?;
        if let Err(e) = f.write_all(n.render().as_bytes()) {
            let _ = std::fs::remove_file(&tmp);
            return Err(crate::goerr::path("write", tmp.as_bytes(), &e));
        }
    }
    std::fs::rename(&tmp, &ps).map_err(|e| format!("rename {tmp} {ps}: {}", crate::goerr::text(&e)))
}

/// `sarina daily-report [--path VAULT] [--max-age MINUTES] [--no-write]`.
pub fn report_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("daily-report");
    fs.string("path", &env.getenv("SARINA_VAULT"), "the vault folder (default $SARINA_VAULT)");
    fs.int("max-age", 0, "print the note's status when it is at most this many minutes old; 0 probes every time");
    fs.bool("no-write", false, "probe and print; change nothing in the vault");
    if fs.parse(args, &mut env.stderr).is_err() {
        env.flush();
        return 2;
    }
    let vault = fs.str("path");
    let (y, mo, d, h, mi, sec, _) = crate::gotime::Time::now_local().civil();
    let today = format!("{y:04}-{mo:02}-{d:02}");
    let mut n = Note::default();
    if !vault.is_empty() {
        n = load(&vault, &today);
        if n.fresh((&today, h * 3600 + mi * 60 + sec), fs.num("max-age")) {
            env.stdout.extend_from_slice(n.print(true).as_bytes());
            env.flush();
            return 0;
        }
    }
    let s = probe(&crate::goctx::Ctx::background());
    n.set_status(&s);
    if !vault.is_empty() && !fs.is_true("no-write") {
        if let Err(e) = save(&vault, &n) {
            env.eprintln(&format!("sarina daily-report: {e}"));
            return 1;
        }
    }
    env.stdout.extend_from_slice(n.print(false).as_bytes());
    env.flush();
    0
}
