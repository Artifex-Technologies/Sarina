//! Go: `sysinfo/crash.go`: `crash_detail` (the latest crash of one program: what coredumpctl recorded, the frames named
//! from the program file with addr2line, the owning package, the log lines) and the crashes topic's summary.

use super::{cmd, run, run_text};
use crate::goctx::Ctx;
use crate::gotext::{self, fields, quote, trim_space};
use std::sync::LazyLock;

fn re(p: &str) -> regex::Regex {
    crate::goregexp::compile(p).expect("sysinfo pattern")
}

static EXE_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9._+/-]{1,120}$"));
static FRAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"#(\d+)\s+0x[0-9a-f]+\s+(\S+)\s+\((/[^ )]+) \+ (0x[0-9a-f]+)\)"));
static ANY_FRAME_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"#\d+\s+0x[0-9a-f]+\s+(\S+)\s+\("));
static GAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| re(r"(?i)(steamapps/common/|/proton|/wine|\.exe$|wine64|wineserver|gamescope|lutris|heroic|retroarch|/games/)"));

/// Go's `filepath.Base` of a string.
pub(crate) fn base(p: &str) -> String {
    gotext::lossy(&crate::gopath::base(p.as_bytes()))
}

/// Go's `findExecutable`: a file that is not a folder and that this user may run (eaccess; the mode bits where the
/// call is not there).
fn executable(p: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(m) = std::fs::metadata(p) else { return false };
    if m.is_dir() {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(p) else { return false };
    // SAFETY: a valid C string; faccessat only reads it.
    let r = unsafe { libc::faccessat(libc::AT_FDCWD, c.as_ptr(), libc::X_OK, libc::AT_EACCESS) };
    if r == 0 {
        return true;
    }
    let e = std::io::Error::last_os_error().raw_os_error();
    if e == Some(libc::ENOSYS) || e == Some(libc::EPERM) {
        return m.permissions().mode() & 0o111 != 0;
    }
    false
}

/// Go's `exec.LookPath`: a name with a slash as it is, else the first absolute match on PATH.
fn look_path(exe: &str) -> Option<String> {
    if exe.contains('/') {
        return executable(exe).then(|| exe.to_string());
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = gotext::lossy(&crate::gopath::join(&[dir.as_bytes(), exe.as_bytes()]));
        if executable(&p) {
            // Go refuses a program found relative to the current folder (ErrDot)
            return p.starts_with('/').then_some(p);
        }
    }
    None
}

/// Go's `regexp.QuoteMeta`.
fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Go's `CrashDetail`.
pub fn crash_detail(ctx: &Ctx, exe: &str) -> Result<Vec<u8>, String> {
    let exe = trim_space(exe);
    if !EXE_RE.is_match(exe) || exe.contains("..") || exe.starts_with('-') {
        return Err(format!("{} is not a program name or path", quote(exe)));
    }
    let info = run_text(ctx, &cmd(&["coredumpctl", "info", "--no-pager", "-1", exe], 80));
    let wh = resolve_frames(ctx, &info);
    let mut b: Vec<u8> = Vec::new();
    b.extend_from_slice(
        format!("## Summary\n{}\n\n## The crash record (selected lines)\n{}", crash_summary(exe, &info, &wh), record_lines(&info))
            .as_bytes(),
    );
    if !wh.is_empty() {
        b.extend_from_slice(b"\n\n## Where it crashed (frames named from the program file; it was not run)\n");
        b.extend_from_slice(wh.as_bytes());
    }
    let owner = if exe.starts_with('/') { std::fs::metadata(exe).is_ok().then(|| exe.to_string()) } else { look_path(exe) };
    if let Some(p) = owner {
        b.extend_from_slice(b"\n\n## Package that owns it\n");
        b.extend_from_slice(&run(ctx, &cmd(&["pacman", "-Qo", &p], 0)));
    }
    b.extend_from_slice(b"\n\n## Log lines mentioning it this boot\n");
    let pat = quote_meta(&base(exe));
    let out = run(ctx, &cmd(&["journalctl", "-b", "--no-pager", "-n", "40", "-g", &pat, "-o", "short"], 0));
    let mut logs: Vec<&[u8]> = out.split(|&c| c == b'\n').filter(|l| !l.is_empty() && l[0] != b' ' && l[0] != b'\t').collect();
    if logs.len() > 10 {
        logs.drain(..logs.len() - 10);
    }
    b.extend_from_slice(&logs.join(&b'\n'));
    b.extend_from_slice(b"\n\n[Evidence only: what the system recorded. A stack with no symbols means the package has no debug info; say so, and say what you cannot tell. Suggest a cause only as a guess and say how to check it.]");
    Ok(b)
}

/// Go's `resolveFrames`: the frames the record left unnamed, named with addr2line from files that exist (at most six).
fn resolve_frames(ctx: &Ctx, info: &str) -> String {
    let mut out = Vec::new();
    for m in FRAME_RE.captures_iter(info) {
        if out.len() == 6 {
            break;
        }
        let (num, name, path) = (&m[1], &m[2], &m[3]);
        let mut off = m[4].to_string();
        if !std::fs::metadata(path).is_ok_and(|f| f.is_file()) {
            continue;
        }
        // frames above the first hold return addresses, one past the call: look up the call itself
        if num != "0" {
            if let Ok(v) = u64::from_str_radix(&off[2..], 16) {
                if v > 0 {
                    off = format!("0x{:x}", v - 1);
                }
            }
        }
        let res = run_text(ctx, &cmd(&["addr2line", "-f", "-C", "-e", path, &off], 2));
        let lines: Vec<&str> = trim_space(&res).split('\n').collect();
        let mut func = name.to_string();
        let mut loc = String::new();
        if !lines.is_empty() && lines[0] != "??" && !lines[0].starts_with('(') {
            func = lines[0].to_string();
        }
        if lines.len() >= 2 && !lines[1].starts_with("??") {
            loc = format!(" at {}", lines[1]);
            let src = source_line(lines[1]);
            if !src.is_empty() {
                loc += &format!(": \"{src}\"");
            }
        }
        if func == "n/a" {
            continue;
        }
        out.push(format!("#{num} {func} in {}{loc}", base(path)));
    }
    out.join("\n")
}

fn field(info: &str, name: &str) -> String {
    let prefix = format!("{name}:");
    for l in info.split('\n') {
        let t = trim_space(l);
        if let Some(rest) = t.strip_prefix(&prefix) {
            return trim_space(rest).to_string();
        }
    }
    String::new()
}

fn record_lines(info: &str) -> String {
    let mut out = Vec::new();
    for k in ["Timestamp", "Signal", "Command Line", "Executable", "Package", "Storage"] {
        let v = field(info, k);
        if !v.is_empty() {
            out.push(format!("{k}: {v}"));
        }
    }
    if out.is_empty() {
        return info.to_string();
    }
    out.join("\n")
}

/// Go's `crashSummary`.
fn crash_summary(exe: &str, info: &str, wh: &str) -> String {
    let (sig, when) = (field(info, "Signal"), field(info, "Timestamp"));
    if sig.is_empty() {
        return format!("No crash of {} is recorded.", base(exe));
    }
    let mut s = format!("{} crashed with signal {sig}", base(exe));
    if !when.is_empty() {
        s += &format!(" at {when}");
    }
    s.push('.');
    if !wh.is_empty() {
        let lines: Vec<&str> = wh.split('\n').collect();
        let first = lines[0].strip_prefix("#0 ").unwrap_or(lines[0]);
        s += &format!(" It crashed in {first}");
        if lines.len() > 1 {
            s += &format!(", called from {}", lines[1].split_once(' ').map_or("", |x| x.1));
        }
        s.push('.');
    } else {
        let named = named_frames(info, 6);
        if !named.is_empty() {
            s += &format!(" Its stack, innermost first: {}.", named.join(" < "));
        } else {
            s += " The program has no symbols, so where in it the crash happened cannot be named.";
        }
    }
    let c = field(info, "Command Line");
    if fields(&c).len() > 1 {
        s += &format!(" It was running: {c}.");
    }
    s
}

/// Go's `namedFrames`: the names the first stack has, the unnamed ones skipped.
fn named_frames(info: &str, n: usize) -> Vec<String> {
    const MARK: &str = "Stack trace of thread";
    let mut first = info;
    if let Some(i) = info.find(MARK) {
        first = &info[i..];
        if let Some(j) = first[1..].find(MARK) {
            first = &first[..j + 1];
        }
    }
    let mut out = Vec::new();
    for m in ANY_FRAME_RE.captures_iter(first) {
        if &m[1] != "n/a" {
            out.push(m[1].to_string());
        }
        if out.len() == n {
            break;
        }
    }
    out
}

/// Go's `fmt.Sscan` of one int: a sign, then a base prefix as `%v` reads it (0b, 0o, 0x, or a leading 0 for octal),
/// then the digits it can take; None where Go's scan fails.
fn sscan_int(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let mut i = 0;
    let neg = match b.first() {
        Some(b'+') => {
            i = 1;
            false
        }
        Some(b'-') => {
            i = 1;
            true
        }
        _ => false,
    };
    let mut radix = 10;
    let mut zero = false;
    if b.get(i) == Some(&b'0') {
        i += 1;
        zero = true;
        match b.get(i) {
            Some(b'b' | b'B') => {
                radix = 2;
                i += 1;
                zero = false;
            }
            Some(b'o' | b'O') => {
                radix = 8;
                i += 1;
                zero = false;
            }
            Some(b'x' | b'X') => {
                radix = 16;
                i += 1;
                zero = false;
            }
            _ => radix = 8,
        }
    }
    let start = i;
    while i < b.len() && ((b[i] as char).is_digit(radix) || b[i] == b'_') {
        i += 1;
    }
    let digits: String = s[start..i].chars().filter(|&c| c != '_').collect();
    if digits.is_empty() {
        return if zero { Some(0) } else { None };
    }
    let v = i64::from_str_radix(&digits, radix).ok()?;
    Some(if neg { -v } else { v })
}

/// Go's `sourceLine`: the text of FILE:LINE when the source is on this computer (a regular file of at most 1 MB).
fn source_line(file_line: &str) -> String {
    let Some(i) = file_line.rfind(':') else { return String::new() };
    let (file, rest) = (&file_line[..i], &file_line[i + 1..]);
    let num = fields(rest);
    let Some(first) = num.first() else { return String::new() };
    let Some(n) = sscan_int(first) else { return String::new() };
    if n < 1 {
        return String::new();
    }
    let Ok(m) = std::fs::metadata(file) else { return String::new() };
    if !m.is_file() || m.len() > 1 << 20 {
        return String::new();
    }
    let Ok(data) = std::fs::read(file) else { return String::new() };
    let ls: Vec<&[u8]> = data.split(|&c| c == b'\n').collect();
    if n as usize > ls.len() {
        return String::new();
    }
    let t = gotext::trim_space_bytes(ls[n as usize - 1]);
    match super::rune_prefix(t, 120) {
        Some(mut cut) => {
            cut.extend_from_slice("…".as_bytes());
            gotext::lossy(&cut)
        }
        None => gotext::lossy(t),
    }
}

/// Go's `newestCrash`: which program crashed last, when, and whether it is a game.
pub fn newest_crash(list: &str) -> String {
    for l in list.split('\n') {
        let f = fields(l);
        if f.len() < 10 || !f[7].starts_with("SIG") {
            continue;
        }
        let exe = f[9];
        let kind = if GAME_RE.is_match(exe) {
            "it is a game or runs one (Steam, Proton or Wine)"
        } else {
            "it is not a game (not from Steam, Proton or Wine)"
        };
        return format!("The newest crash is {} ({exe}) at {} {} {}; {kind}.", base(exe), f[1], f[2], f[3]);
    }
    "No crash is recorded in the last 7 days.".into()
}
