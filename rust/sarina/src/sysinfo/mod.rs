//! Go: `sysinfo`: the optional system tool set (SARINA_SYSTEM=on): `system_status` (how the computer is doing, from
//! fixed, harmless commands: no shell, nothing from the model on a command line, 8 s each), `crash_detail` (the latest
//! crash of one program) and `system_fix` (a named, reversible repair, shell-gated), and the skills' "status" and
//! "security" prefetches (which run with the set off too). Proved by the CLI differential's system cases (rust/parity),
//! against stand-in programs on the case's own PATH and a stand-in Heimdall.
//!
//! A command's output is read as bytes, as Go holds it in a string: a byte that is not UTF-8 stays one byte (one U+FFFD
//! when the text reaches JSON) and the cuts by characters write it as U+FFFD, as Go's `[]rune` round trip does.

pub mod crash;
pub mod gaming;
pub mod security;

use crate::goctx::Ctx;
use crate::gotext;
use crate::tools::registry::{Output, Tool};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// One fixed command of a topic.
#[derive(Clone, Debug)]
pub struct Step {
    pub title: String,
    pub argv: Vec<String>,
    /// keep this many lines (0: all)
    pub lines: usize,
    /// cut each line at this many characters (0: whole lines)
    pub cols: usize,
}

pub(crate) fn step(title: &str, argv: &[&str], lines: usize, cols: usize) -> Step {
    Step { title: title.into(), argv: argv.iter().map(|s| s.to_string()).collect(), lines, cols }
}

/// A command with no title (the summaries' own lookups).
pub(crate) fn cmd(argv: &[&str], lines: usize) -> Step {
    step("", argv, lines, 0)
}

/// Go's `Topics`: what the model may ask about, each a few fixed commands ("gaming" with Steam's folder as it is now).
pub fn topics() -> BTreeMap<&'static str, Vec<Step>> {
    let mut t = BTreeMap::new();
    t.insert(
        "performance",
        vec![
            step("Uptime and load", &["uptime"], 0, 0),
            step("Memory", &["free", "-h"], 0, 0),
            step("Busiest programs by processor", &["ps", "-eo", "pid,comm,pcpu,pmem", "--sort=-pcpu"], 9, 0),
            step("Biggest programs by memory", &["ps", "-eo", "pid,comm,pcpu,pmem", "--sort=-pmem"], 9, 0),
            step(
                "Pressure (share of time programs waited)",
                &["cat", "/proc/pressure/cpu", "/proc/pressure/memory", "/proc/pressure/io"],
                0,
                0,
            ),
        ],
    );
    t.insert(
        "network",
        vec![
            step("Connection state", &["nmcli", "-t", "-f", "STATE,CONNECTIVITY", "general", "status"], 0, 0),
            step("Devices", &["nmcli", "-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"], 0, 0),
            step("Addresses", &["ip", "-brief", "address"], 0, 0),
            step("Routes", &["ip", "route"], 6, 0),
            step("Name server answers", &["getent", "hosts", "archlinux.org"], 2, 0),
        ],
    );
    t.insert(
        "disk",
        vec![
            step("Space", &["df", "-h", "-x", "tmpfs", "-x", "devtmpfs", "-x", "squashfs"], 0, 0),
            step("Drives", &["lsblk", "-o", "NAME,SIZE,TYPE,MOUNTPOINTS"], 0, 0),
        ],
    );
    t.insert(
        "logs",
        vec![step(
            "Warnings and errors since boot (newest last)",
            &["journalctl", "-b", "-p", "warning", "-n", "40", "--no-pager", "-o", "short"],
            0,
            0,
        )],
    );
    t.insert(
        "services",
        vec![
            step("Services that failed", &["systemctl", "--failed", "--no-pager", "--plain"], 0, 0),
            step("User services that failed", &["systemctl", "--user", "--failed", "--no-pager", "--plain"], 0, 0),
        ],
    );
    t.insert(
        "crashes",
        vec![
            step(
                "Crashes in the last 7 days, newest first (core dumps)",
                &["coredumpctl", "list", "--no-pager", "--reverse", "--since=-7d", "-n", "12"],
                0,
                0,
            ),
            step(
                "Crash and out-of-memory lines this boot",
                &[
                    "journalctl",
                    "-b",
                    "--no-pager",
                    "-n",
                    "25",
                    "-g",
                    "segfault|core dumped|dumped core|out of memory|oom-kill|Killed process|general protection",
                    "-o",
                    "short",
                ],
                0,
                0,
            ),
        ],
    );
    t.insert(
        "boot",
        vec![
            step("Time to start", &["systemd-analyze", "--no-pager"], 0, 0),
            step(
                "When the login screen appeared (what it waited for)",
                &["systemd-analyze", "critical-chain", "display-manager.service", "--no-pager"],
                12,
                0,
            ),
            step(
                "When everything was started (the graphical target; background services can make this later than the login screen)",
                &["systemd-analyze", "critical-chain", "--no-pager"],
                14,
                0,
            ),
            step(
                "Slowest units overall (includes background jobs that do not delay the start)",
                &["systemd-analyze", "blame", "--no-pager"],
                10,
                0,
            ),
        ],
    );
    t.insert("audio", vec![step("Sound devices and apps", &["wpctl", "status"], 28, 0)]);
    t.insert(
        "kernel",
        vec![step(
            "Kernel warnings and errors this boot",
            &["journalctl", "-k", "-b", "-p", "warning", "-n", "30", "--no-pager", "-o", "short"],
            0,
            0,
        )],
    );
    t.insert("security", vec![]);
    t.insert(
        "hardware",
        vec![
            step("Temperatures and fans", &["sensors"], 30, 0),
            step(
                "Graphics card",
                &["nvidia-smi", "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu", "--format=csv,noheader"],
                0,
                0,
            ),
            step("Battery and power", &["cat", "/sys/class/power_supply/BAT0/capacity"], 0, 0),
        ],
    );
    t.insert("gaming", gaming::steps());
    t
}

/// Go's `names`: the topics, sorted.
pub fn names() -> Vec<&'static str> {
    topics().into_keys().collect()
}

/// The most of one command's output that is kept (Go's `maxStepOutput`).
pub const MAX_STEP_OUTPUT: usize = 1 << 20;

/// Go's `exec.CommandContext(ctx with 8 s, ...)` with only standard output kept (its first MiB), standard error to the
/// null device and `WaitDelay` 2 s: what it printed, and whether `Run` returned an error.
fn exec(ctx: &Ctx, argv: &[String]) -> (Vec<u8>, bool) {
    let mut deadline = Instant::now() + Duration::from_secs(8);
    if let Some(d) = ctx.deadline() {
        deadline = deadline.min(d);
    }
    // a context already done: Go's Start refuses before starting anything
    if ctx.err().is_some() || Instant::now() >= deadline {
        return (Vec::new(), true);
    }
    let Ok(path) = crate::tools::exec::look_path_go(&argv[0]) else { return (Vec::new(), true) };
    let mut out = Vec::new();
    let mut sink = |stream: &'static str, b: &[u8]| {
        if stream == "stdout" {
            let room = MAX_STEP_OUTPUT.saturating_sub(out.len());
            out.extend_from_slice(&b[..b.len().min(room)]);
        }
    };
    let env = crate::tools::exec::parent_env();
    match crate::tools::exec::run_group(&path, &argv[1..], &env, None, deadline, ctx, crate::tools::exec::GroupKill::Inherit, &mut sink) {
        Err(_) => (out, true),
        Ok(end) => {
            let ok = end.status.is_some_and(|s| s.success()) && !end.io_incomplete;
            (out, !ok)
        }
    }
}

/// The first `n` characters of `b` as Go's `string([]rune(b)[:n])` writes them (a byte that is not UTF-8 is one
/// character and comes out as U+FFFD), when `b` has more than `n`.
pub(crate) fn rune_prefix(b: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut count = 0;
    let mut i = 0;
    let mut end = None;
    while i < b.len() {
        if count == n {
            end = Some(i);
            break;
        }
        let (_, w) = gotext::decode_rune(&b[i..]);
        i += w.max(1);
        count += 1;
    }
    let end = end?;
    let mut out = Vec::with_capacity(end + 8);
    let mut i = 0;
    while i < end {
        match gotext::decode_rune(&b[i..]) {
            (Some(c), w) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                i += w;
            }
            (None, _) => {
                out.extend_from_slice("\u{FFFD}".as_bytes());
                i += 1;
            }
        }
    }
    Some(out)
}

/// Go's `run`: one step's output, trimmed, cut to its lines and columns and to 3500 characters; "(not available here)"
/// when it printed nothing and failed, "(nothing to report)" when it printed nothing and succeeded.
pub fn run(ctx: &Ctx, s: &Step) -> Vec<u8> {
    let (out, failed) = exec(ctx, &s.argv);
    let text = gotext::trim_space_bytes(&out);
    if text.is_empty() {
        return if failed { b"(not available here)".to_vec() } else { b"(nothing to report)".to_vec() };
    }
    let mut lines: Vec<Vec<u8>> = text.split(|&c| c == b'\n').map(<[u8]>::to_vec).collect();
    if s.lines > 0 && lines.len() > s.lines {
        lines.truncate(s.lines);
    }
    if s.cols > 0 {
        for l in lines.iter_mut() {
            if let Some(mut cut) = rune_prefix(l, s.cols) {
                cut.extend_from_slice(b"...");
                *l = cut;
            }
        }
    }
    let text = lines.join(&b'\n');
    match rune_prefix(&text, 3500) {
        Some(mut cut) => {
            cut.extend_from_slice(b"\n[cut]");
            cut
        }
        None => text,
    }
}

/// [`run`] as text (each byte that is not UTF-8 a U+FFFD), for the summaries that read it.
pub(crate) fn run_text(ctx: &Ctx, s: &Step) -> String {
    gotext::lossy(&run(ctx, s))
}

/// Go's `Report`: one topic's commands and what they printed, with the topic's summary first.
pub fn report(ctx: &Ctx, topic: &str) -> Result<Vec<u8>, String> {
    let key = gotext::to_lower(gotext::trim_space(topic));
    let all = topics();
    let Some(steps) = all.get(key.as_str()) else {
        return Err(format!("no such topic {}: use one of {}", gotext::quote(topic), names().join(", ")));
    };
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(format!("Now: {}\n\n", crate::gotime::Time::now_local().format_weekday_date_minute()).as_bytes());
    match key.as_str() {
        "boot" => {
            let sum = boot_summary(ctx);
            if !sum.is_empty() {
                b.extend_from_slice(format!("## Summary\n{sum}\n\n").as_bytes());
            }
        }
        "gaming" => b.extend_from_slice(format!("## Summary\n{}\n\n", gaming::summary(ctx)).as_bytes()),
        "crashes" => {
            let list = run_text(ctx, &steps[0]);
            b.extend_from_slice(format!("## Summary\n{} A game started through Steam with Proton also writes its own log (PROTON_LOG=1 in the game's launch options gives ~/steam-<id>.log).\n\n", crash::newest_crash(&list)).as_bytes());
        }
        "security" => {
            b.extend_from_slice(b"## What Heimdall finds open\n");
            b.extend_from_slice(&security::summary(ctx));
            b.extend_from_slice(b"\n\n");
        }
        "audio" => b.extend_from_slice(b"## Note\nThe sound system (PipeWire, WirePlumber) runs as the user: it is restarted with `systemctl --user restart pipewire pipewire-pulse wireplumber`, never with sudo; with shell, system_fix restart_audio does the same.\n\n"),
        _ => {}
    }
    for s in steps {
        b.extend_from_slice(format!("## {}\n", s.title).as_bytes());
        b.extend_from_slice(&run(ctx, s));
        b.extend_from_slice(b"\n\n");
    }
    b.extend_from_slice(b"[Evidence only: these are the numbers the computer gave just now. Say what they show, in plain words; if something looks fine, say it looks fine; if a part is not available, say you could not see it.]");
    Ok(b)
}

static AT_RE: LazyLock<regex::Regex> = LazyLock::new(|| crate::goregexp::compile(r"^(\S+)\s+@([0-9.]+(?:min [0-9.]+)?m?s)").expect("atRE"));

/// Go's `bootSummary`: when the login screen appeared, when everything was started, and the slowest units that are not
/// in the login screen's chain.
fn boot_summary(ctx: &Ctx) -> String {
    let dm = run_text(ctx, &cmd(&["systemd-analyze", "critical-chain", "display-manager.service", "--no-pager"], 0));
    let all = run_text(ctx, &cmd(&["systemd-analyze", "critical-chain", "--no-pager"], 0));
    let first = |chain: &str| -> (String, String) {
        for l in chain.split('\n') {
            if let Some(m) = AT_RE.captures(gotext::trim_space(l)) {
                return (m[1].to_string(), m[2].to_string());
            }
        }
        (String::new(), String::new())
    };
    let (dm_unit, dm_at) = first(&dm);
    let (_, all_at) = first(&all);
    if dm_at.is_empty() && all_at.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    if !dm_at.is_empty() {
        parts.push(format!("The login screen ({dm_unit}) appeared {dm_at} after the start."));
    }
    if !all_at.is_empty() {
        parts.push(format!("Everything was started after {all_at}."));
    }
    let mut background = Vec::new();
    for l in run_text(ctx, &cmd(&["systemd-analyze", "blame", "--no-pager"], 6)).split('\n') {
        let f = gotext::fields(l);
        if f.len() < 2 {
            continue;
        }
        let unit = f[f.len() - 1];
        if !dm.contains(unit) {
            background.push(unit.to_string());
        }
    }
    if !background.is_empty() {
        parts.push(format!("These slow units are not in the login screen's chain, so they did not delay it (they run in the background or later), and changing them would not bring the login screen sooner: {}. archlinux-keyring-wkd-sync keeps the package signing keys current and should stay on.", background.join(", ")));
    }
    parts.join(" ")
}

fn str_arg(a: &BTreeMap<String, crate::goany::Any>, k: &str) -> String {
    match a.get(k) {
        Some(crate::goany::Any::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn meta(pairs: &[(&str, &str)]) -> BTreeMap<String, crate::goany::Any> {
    pairs.iter().map(|(k, v)| (k.to_string(), crate::goany::Any::String(v.to_string()))).collect()
}

/// Go's `Tools`: crash_detail, system_fix and system_status, when SARINA_SYSTEM=on (read when the registry is built).
pub fn tools() -> Vec<Tool> {
    if std::env::var("SARINA_SYSTEM").unwrap_or_default() != "on" {
        return Vec::new();
    }
    use crate::tools::defs::tool;
    use crate::tools::files::fail;
    let crash = tool(
        "crash_detail",
        Arc::new(|a, tc| {
            let p = str_arg(a, "program");
            match crash::crash_detail(&tc.ctx, &p) {
                Ok(text) => {
                    Output { content: gotext::lossy(&text), metadata: meta(&[("action", "crash_detail"), ("program", &p)]), error: None }
                }
                Err(e) => fail(e),
            }
        }),
    );
    let fix = tool(
        "system_fix",
        Arc::new(|a, tc| {
            let name = str_arg(a, "action");
            match gaming::apply_fix(&tc.ctx, &name) {
                Ok(text) => {
                    Output { content: gotext::lossy(&text), metadata: meta(&[("action", "system_fix"), ("fix", &name)]), error: None }
                }
                Err(e) => fail(e),
            }
        }),
    );
    let status = tool(
        "system_status",
        Arc::new(|a, tc| {
            let topic = str_arg(a, "topic");
            match report(&tc.ctx, &topic) {
                Ok(text) => {
                    Output { content: gotext::lossy(&text), metadata: meta(&[("action", "system_status"), ("topic", &topic)]), error: None }
                }
                Err(e) => fail(e),
            }
        }),
    );
    vec![crash, fix, status]
}

// ---- the skills' prefetches -------------------------------------------------------------------------------------------

static KEYWORD_TOPICS: LazyLock<Vec<(regex::Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)\b(crash|crashed|crashes|core dump|segfault|segmentation|aborted|froze|freez|hung|hang)\w*", "crashes"),
        (r"(?i)\b(slow|lag|laggy|sluggish|performance|cpu|processor|memory|ram|swap|busy|fan|hot|temperature|heat|load)\b", "performance"),
        (r"(?i)\b(internet|wi-?fi|network|connection|connect|online|offline|dns|ethernet)\b", "network"),
        (r"(?i)\b(disk|storage|space|full)\b", "disk"),
        (r"(?i)\b(boot|startup|start up|takes? long to (start|boot))\b", "boot"),
        (r"(?i)\b(sound|audio|speaker|speakers|headphones?|headset|microphone|mic|volume)\b", "audio"),
        (r"(?i)\b(error|errors|log|logs|failed|failing|broke|broken|problem|wrong|issue)\b", "services"),
        (r"(?i)\b(game|games|gaming|steam|proton|wine|mods?|modding|fps|frame ?rates?|stutter\w*|controller|gamepad)\b", "gaming"),
    ]
    .into_iter()
    .map(|(p, t)| (crate::goregexp::compile(p).expect("keywordTopics"), t))
    .collect()
});

/// Go's `topicsFor`: the route by meaning when the embedding model answers, else the voice path's keywords; at most two
/// topics that system_status has.
fn topics_for(task: &str) -> Vec<String> {
    let getenv = |k: &str| std::env::var(k).unwrap_or_default();
    let out: Vec<String> = match crate::route::route(task, &crate::embed::default_embedder(&getenv)) {
        Ok(found) => found,
        Err(_) => KEYWORD_TOPICS.iter().filter(|(re, _)| re.is_match(task)).map(|(_, t)| t.to_string()).collect(),
    };
    let all = topics();
    let mut keep = Vec::new();
    for t in out {
        if all.contains_key(t.as_str()) && keep.len() < 2 {
            keep.push(t);
        }
    }
    keep
}

/// Go's `perTopic`: the most of one topic's report (in bytes) that goes in front of the model.
const PER_TOPIC: usize = 3500;

/// Go's `statusContext`: what the computer reports about the task's topics, before the model answers.
pub fn status_context(task: &str) -> Vec<u8> {
    let topics = topics_for(task);
    if topics.is_empty() {
        return Vec::new();
    }
    let ctx = Ctx::with_deadline(Instant::now() + Duration::from_secs(30));
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for t in &topics {
        let Ok(mut out) = report(&ctx, t) else { continue };
        if gotext::trim_space_bytes(&out).is_empty() {
            continue;
        }
        if out.len() > PER_TOPIC {
            out.truncate(PER_TOPIC);
            out.extend_from_slice("\n[…]".as_bytes());
        }
        let mut p = format!("## {t}\n").into_bytes();
        p.extend_from_slice(&out);
        parts.push(p);
    }
    if parts.is_empty() {
        return Vec::new();
    }
    let mut b =
        b"What the computer reports right now (read-only, already looked up for you; call system_status for any other topic):\n\n".to_vec();
    b.extend_from_slice(&parts.join(&b"\n\n"[..]));
    b
}

/// Go's "security" context: Heimdall's report, looked up for a skill that asks for it.
pub fn security_context(_task: &str) -> Vec<u8> {
    let ctx = Ctx::with_deadline(Instant::now() + Duration::from_secs(15));
    let mut b = b"What Heimdall reports right now (read-only, already looked up for you):\n\n".to_vec();
    b.extend_from_slice(&report(&ctx, "security").unwrap_or_default());
    b
}
