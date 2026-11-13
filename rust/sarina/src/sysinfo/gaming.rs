//! Go: `sysinfo/gaming.go`: the "gaming" topic (Steam, Proton, the card and who holds its memory), `GameRunning`, and
//! `system_fix`'s table of named repairs at user level.

use super::{cmd, run, run_text, step, Step};
use crate::goctx::Ctx;
use crate::gotext::{self, quote, trim_space};
use std::sync::LazyLock;

/// Go's `os.UserHomeDir` as the topic uses it ("" when HOME is not set).
fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

fn join(parts: &[&str]) -> String {
    let b: Vec<&[u8]> = parts.iter().map(|p| p.as_bytes()).collect();
    gotext::lossy(&crate::gopath::join(&b))
}

/// Go's `steamRoot`: Steam's data folder for this user ("" when Steam has never run).
fn steam_root() -> String {
    let h = home();
    for d in [join(&[&h, ".local", "share", "Steam"]), join(&[&h, ".steam", "steam"])] {
        if std::fs::metadata(join(&[&d, "steamapps"])).is_ok_and(|m| m.is_dir()) {
            return d;
        }
    }
    String::new()
}

/// The gaming topic's steps (Go builds them at start; HOME does not change in a run).
pub fn steps() -> Vec<Step> {
    let mut root = steam_root();
    if root.is_empty() {
        root = join(&[&home(), ".local", "share", "Steam"]);
    }
    let compat = join(&[&root, "compatibilitytools.d"]);
    let common = join(&[&root, "steamapps", "common"]);
    vec![
        step("Gaming setup check (labyrinth-gaming check: PASS, WARN or FAIL per item)", &["labyrinth-gaming", "check"], 0, 0),
        step(
            "Graphics card now (use %, memory used MiB, memory total MiB, temperature C, power W)",
            &["nvidia-smi", "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw", "--format=csv,noheader"],
            0,
            0,
        ),
        step(
            "Programs holding graphics memory (pid, program, MiB)",
            &["nvidia-smi", "--query-compute-apps=pid,process_name,used_memory", "--format=csv,noheader"],
            14,
            110,
        ),
        step("Game mode (gamemoded; it is active only while a game started with gamemoderun is running, so inactive is normal when no game runs)", &["gamemoded", "-s"], 0, 0),
        step("Steam, Proton and Wine processes", &["pgrep", "-a", "-f", r"steam|wine|proton|gamescope|umu-run|\.exe"], 12, 140),
        step(
            "Proton versions available (system package, then your own compatibilitytools.d)",
            &["ls", "-1", "/usr/share/steam/compatibilitytools.d", &compat],
            20,
            0,
        ),
        step("Installed Steam games (steamapps/common)", &["ls", "-1", &common], 40, 0),
    ]
}

static GPU_APP_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| crate::goregexp::compile(r"^\s*(\d+),\s*(.*?),\s*(\d+)\s*MiB").expect("gpuAppRE"));
static APP_ID_RE: LazyLock<regex::Regex> = LazyLock::new(|| crate::goregexp::compile(r"SteamLaunch AppId=(\d+)").expect("appIDRE"));

/// Go's `strconv.Atoi` with its result kept on overflow (the value clamped, as Go returns it with ErrRange).
fn atoi_clamped(s: &str) -> i64 {
    match s.parse::<i64>() {
        Ok(v) => v,
        Err(_) if !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) => i64::MAX,
        Err(_) => 0,
    }
}

/// Go's `gamingSummary`: whether a game runs, and who holds the card's memory, Sarina's own model included.
pub fn summary(ctx: &Ctx) -> String {
    let mut parts = Vec::new();
    match game_running(ctx) {
        Some(why) => parts.push(format!("A game seems to be running ({why}).")),
        None => parts.push("No game is running right now.".to_string()),
    }
    let card = run_text(ctx, &cmd(&["nvidia-smi", "--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits"], 0));
    let f: Vec<&str> = card.split(',').collect();
    if f.len() == 2 {
        parts.push(format!("The graphics card has {} of {} MiB in use.", trim_space(f[0]), trim_space(f[1])));
    }
    let apps = run_text(ctx, &cmd(&["nvidia-smi", "--query-compute-apps=pid,process_name,used_memory", "--format=csv,noheader"], 0));
    let mut ollama: i64 = 0;
    for l in apps.split('\n') {
        if let Some(m) = GPU_APP_RE.captures(l) {
            if gotext::to_lower(&m[2]).contains("ollama") {
                ollama = ollama.wrapping_add(atoi_clamped(&m[3]));
            }
        }
    }
    if ollama > 0 {
        parts.push(format!("Ollama, the model server Sarina answers from, holds {ollama} MiB of it (her model loads to answer and is unloaded by itself after a few idle minutes; `ollama stop MODEL` frees it at once, and it loads again on the next question). Part of the card's use right now is that model answering this very question, so a high use figure here is not a game's."));
    } else {
        parts.push("Sarina's model is not loaded, so it takes no graphics memory at the moment.".to_string());
    }
    parts.join(" ")
}

/// Go's `GameRunning`: game mode active, a game Steam launched, or gamescope up (the reason, or None).
pub fn game_running(ctx: &Ctx) -> Option<String> {
    if gotext::to_lower(&run_text(ctx, &cmd(&["gamemoded", "-s"], 0))).contains("is active") {
        return Some("game mode is active".into());
    }
    let out = run_text(ctx, &cmd(&["pgrep", "-a", "-f", "SteamLaunch AppId="], 0));
    if !out.is_empty() {
        for l in out.split('\n') {
            if l.contains("pgrep") {
                continue;
            }
            if let Some(m) = APP_ID_RE.captures(l) {
                return Some(format!("Steam launched a game (AppId {})", &m[1]));
            }
        }
    }
    let out = run_text(ctx, &cmd(&["pgrep", "-x", "gamescope"], 0));
    if !out.is_empty() && out != "(not available here)" && out != "(nothing to report)" {
        return Some("gamescope is running".into());
    }
    None
}

// ---- system_fix ---------------------------------------------------------------------------------------------------

/// A repair's condition: why it is not done, when it is not.
type Condition = fn(&Ctx) -> Result<(), &'static str>;

/// Go's `Fix`: one repair (fixed commands at this user's level, a check afterwards, and a condition for some).
struct Fix {
    name: &'static str,
    what: &'static str,
    steps: &'static [&'static [&'static str]],
    check: &'static [&'static str],
    only: Option<Condition>,
}

fn bluetooth_on(ctx: &Ctx) -> Result<(), &'static str> {
    if !run_text(ctx, &cmd(&["bluetoothctl", "show"], 0)).contains("Powered: yes") {
        return Err("Bluetooth is off (or there is no controller); turning it on is the owner's choice, not a repair");
    }
    Ok(())
}

const FIXES: &[Fix] = &[
    Fix {
        name: "restart_audio",
        what: "restart the sound system for this user (PipeWire, its PulseAudio bridge and WirePlumber): for no sound, a device stuck on the wrong output, or crackling after sleep. Audio stops for about a second and programs reconnect by themselves.",
        steps: &[&["systemctl", "--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"]],
        check: &["systemctl", "--user", "is-active", "pipewire", "pipewire-pulse", "wireplumber"],
        only: None,
    },
    Fix {
        name: "restart_bluetooth",
        what: "turn the Bluetooth controller off and on again (bluetoothctl): for a controller or headset that will not reconnect. Paired devices reconnect by themselves. Only when Bluetooth is already on: turning it on is the owner's choice.",
        steps: &[&["bluetoothctl", "power", "off"], &["bluetoothctl", "power", "on"]],
        check: &["bluetoothctl", "show"],
        only: Some(bluetooth_on),
    },
];

static FIX_NAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| crate::goregexp::compile(r"^[a-z_]{3,40}$").expect("fixNameRE"));

fn find_fix(name: &str) -> Option<&'static Fix> {
    let name = gotext::to_lower(trim_space(name));
    if !FIX_NAME_RE.is_match(&name) {
        return None;
    }
    FIXES.iter().find(|f| f.name == name)
}

/// Go's `ApplyFix`: one named repair, each command's output and the check afterwards.
pub fn apply_fix(ctx: &Ctx, name: &str) -> Result<Vec<u8>, String> {
    let Some(f) = find_fix(name) else {
        let names: Vec<&str> = FIXES.iter().map(|f| f.name).collect();
        return Err(format!("no such repair {}: the repairs are {}", quote(name), names.join(", ")));
    };
    if let Some(only) = f.only {
        if let Err(why) = only(ctx) {
            return Err(format!("{} not done: {why}", f.name));
        }
    }
    let mut b = format!("## {}\n{}\n\n", f.name, f.what).into_bytes();
    for argv in f.steps {
        b.extend_from_slice(format!("$ {}\n", argv.join(" ")).as_bytes());
        b.extend_from_slice(&run(ctx, &cmd(argv, 0)));
        b.extend_from_slice(b"\n\n");
    }
    if !f.check.is_empty() {
        b.extend_from_slice(format!("## Check afterwards\n$ {}\n", f.check.join(" ")).as_bytes());
        b.extend_from_slice(&run(ctx, &cmd(f.check, 0)));
        b.extend_from_slice(b"\n\n");
    }
    b.extend_from_slice(
        b"[Evidence only: say what the check shows. If it does not say the parts are active, the repair did not take; say so.]",
    );
    Ok(b)
}
