//! The parts of Python's behaviour the ported programs depend on, so that the Rust programs read, split, strip and write
//! exactly as the Python scripts did: Python's idea of whitespace and line breaks, its path functions, text files read with
//! universal newlines, the local clock, and an `Env` that keeps insertion order like a Python dict.
//!
//! Nothing here may differ from CPython 3.14 for text the programs meet in practice; where it can (rare Unicode corners),
//! docs/VOICE_RUST_PORT.md says so.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

/// An error that ended the Python program with a traceback (exit status 1): an exception the script did not catch.
#[derive(Debug)]
pub struct Crash(pub String);

impl Crash {
    pub fn new(what: impl Into<String>) -> Crash {
        Crash(what.into())
    }
}

pub type PyResult<T> = Result<T, Crash>;

/// str.isspace() for one character (what str.split(), str.strip() and re's \s use).
pub fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\x1c'..='\x1f' | ' ' | '\u{85}' | '\u{a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// str.lstrip(chars)
pub fn lstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_start_matches(|c| chars.contains(c))
}

/// str.split() with no argument.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|x| !x.is_empty()).collect()
}

/// str.splitlines() (no keepends): \n, \r, \r\n, \v, \f, \x1c, \x1d, \x1e, \x85,  ,   end a line.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if brk {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(_, '\n')) = it.peek() {
                    it.next();
                    end += 1;
                }
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// str.lower()
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// The number of characters (Python's len() of a str).
pub fn clen(s: &str) -> usize {
    s.chars().count()
}

/// s[:n] in characters.
pub fn take(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// s[n:] in characters.
pub fn skip(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

/// str.isalnum() for one character: a letter or any kind of number (Python's \w without the underscore).
pub fn is_alnum(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric();
    }
    crate::rx::is_py_alnum(c)
}

/// Universal newlines, as text mode and text=True read: \r\n and \r become \n.
pub fn universal_newlines(s: String) -> String {
    if !s.contains('\r') {
        return s;
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// bytes.decode("utf-8") (strict): an invalid byte is an exception the scripts never catch.
pub fn decode_strict(b: Vec<u8>) -> PyResult<String> {
    String::from_utf8(b).map_err(|e| Crash::new(format!("UnicodeDecodeError: {}", e)))
}

/// bytes.decode("utf-8", errors="replace")
pub fn decode_replace(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

// ---- the environment

/// os.environ, or a dict made from it: insertion order kept, like a Python dict.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Env {
    pub vars: Vec<(String, String)>,
}

impl Env {
    pub fn from_process() -> Env {
        let vars = std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect();
        Env { vars }
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.vars.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
    }

    /// d.get(k, default)
    pub fn get_or<'a>(&'a self, k: &str, default: &'a str) -> &'a str {
        self.get(k).unwrap_or(default)
    }

    /// d.get(k) or "" (a missing or empty value is falsy)
    pub fn truthy(&self, k: &str) -> Option<&str> {
        self.get(k).filter(|v| !v.is_empty())
    }

    pub fn contains(&self, k: &str) -> bool {
        self.get(k).is_some()
    }

    pub fn set(&mut self, k: &str, v: &str) {
        if let Some(e) = self.vars.iter_mut().find(|(kk, _)| kk == k) {
            e.1 = v.to_string();
        } else {
            self.vars.push((k.to_string(), v.to_string()));
        }
    }

    pub fn setdefault(&mut self, k: &str, v: &str) {
        if !self.contains(k) {
            self.vars.push((k.to_string(), v.to_string()));
        }
    }
}

/// Python's C locale coercion (PEP 538): started with no locale (or C/POSIX) and LC_ALL unset, Python sets LC_CTYPE=C.UTF-8 in its
/// own environment, which every program it starts inherits. Done once at start, before any thread exists.
pub fn coerce_c_locale() {
    let lc_all = std::env::var_os("LC_ALL").unwrap_or_default();
    if !lc_all.is_empty() {
        return;
    }
    if std::env::var_os("PYTHONCOERCECLOCALE").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    // SAFETY: setlocale with a valid C string, called before any other thread exists.
    let legacy = unsafe {
        let empty = CString::new("").unwrap();
        libc::setlocale(libc::LC_CTYPE, empty.as_ptr());
        let cur = libc::setlocale(libc::LC_CTYPE, std::ptr::null());
        !cur.is_null() && std::ffi::CStr::from_ptr(cur).to_bytes() == b"C"
    };
    if legacy {
        let ok = unsafe {
            let name = CString::new("C.UTF-8").unwrap();
            !libc::setlocale(libc::LC_CTYPE, name.as_ptr()).is_null()
        };
        if ok {
            // SAFETY: single-threaded at this point.
            unsafe { std::env::set_var("LC_CTYPE", "C.UTF-8") };
        }
    }
}

pub fn getuid() -> u32 {
    unsafe { libc::getuid() }
}

pub fn getpid() -> u32 {
    std::process::id()
}

/// os.getcwd()
pub fn getcwd() -> PyResult<String> {
    std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).map_err(|e| Crash::new(format!("FileNotFoundError: {}", e)))
}

// ---- paths (posixpath)

pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{}{}", a, b)
    } else {
        format!("{}/{}", a, b)
    }
}

pub fn join_all(parts: &[&str]) -> String {
    let mut p = parts[0].to_string();
    for b in &parts[1..] {
        p = join(&p, b);
    }
    p
}

pub fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

pub fn dirname(p: &str) -> String {
    let i = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let head = &p[..i];
    if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/').to_string()
    } else {
        head.to_string()
    }
}

pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let mut initial = if p.starts_with('/') { 1 } else { 0 };
    if initial == 1 && p.starts_with("//") && !p.starts_with("///") {
        initial = 2;
    }
    let mut comps: Vec<&str> = Vec::new();
    for c in p.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(c);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let path = comps.join("/");
    let path = format!("{}{}", "/".repeat(initial), path);
    if path.is_empty() {
        ".".to_string()
    } else {
        path
    }
}

pub fn isabs(p: &str) -> bool {
    p.starts_with('/')
}

/// os.path.abspath
pub fn abspath(p: &str) -> PyResult<String> {
    if isabs(p) {
        Ok(normpath(p))
    } else {
        Ok(normpath(&join(&getcwd()?, p)))
    }
}

/// os.path.expanduser for "~" and "~/..." (the only forms the programs use)
pub fn expanduser(p: &str, environ: &Env) -> String {
    if !p.starts_with('~') {
        return p.to_string();
    }
    let i = p[1..].find('/').map(|i| i + 1).unwrap_or(p.len());
    if i != 1 {
        return p.to_string(); // ~user: not used here
    }
    let home = match environ.get("HOME") {
        Some(h) => h.to_string(),
        None => home_from_passwd().unwrap_or_default(),
    };
    let home = home.trim_end_matches('/');
    let out = format!("{}{}", home, &p[i..]);
    if out.is_empty() {
        "/".to_string()
    } else {
        out
    }
}

fn home_from_passwd() -> Option<String> {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

/// os.path.isdir
pub fn isdir(p: &str) -> bool {
    !p.contains('\0') && fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false)
}

/// os.path.exists
pub fn exists(p: &str) -> bool {
    !p.contains('\0') && fs::metadata(p).is_ok()
}

/// os.access(p, os.X_OK)
pub fn access_x(p: &str) -> bool {
    match CString::new(p) {
        Ok(c) => unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 },
        Err(_) => false,
    }
}

/// os.path.realpath (strict=False), the CPython 3.14 algorithm.
pub fn realpath(filename: &str) -> PyResult<String> {
    // the stack of unresolved parts; None marks "the symlink above was resolved"
    let mut rest: Vec<Option<String>> = filename.split('/').rev().map(|s| Some(s.to_string())).collect();
    let mut part_count = rest.len();
    let mut path = if filename.starts_with('/') { "/".to_string() } else { getcwd()? };
    let mut seen: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    while part_count > 0 {
        let name = match rest.pop() {
            Some(Some(n)) => n,
            Some(None) => {
                if let Some(Some(link)) = rest.pop() {
                    seen.insert(link, Some(path.clone()));
                }
                continue;
            }
            None => break,
        };
        part_count -= 1;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            let i = path.rfind('/').unwrap_or(0);
            path = if i == 0 { "/".to_string() } else { path[..i].to_string() };
            continue;
        }
        let newpath = if path == "/" { format!("/{}", name) } else { format!("{}/{}", path, name) };
        let target = match fs::symlink_metadata(&newpath) {
            Err(_) => {
                path = newpath;
                continue;
            }
            Ok(m) if !m.file_type().is_symlink() => {
                path = newpath;
                continue;
            }
            Ok(_) => {
                if let Some(v) = seen.get(&newpath) {
                    match v {
                        Some(p) => {
                            path = p.clone();
                            continue;
                        }
                        None => {
                            path = newpath;
                            continue;
                        }
                    }
                }
                match fs::read_link(&newpath) {
                    Ok(t) => t.to_string_lossy().into_owned(),
                    Err(_) => {
                        path = newpath;
                        continue;
                    }
                }
            }
        };
        if target.starts_with('/') {
            path = "/".to_string();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Some(newpath));
        rest.push(None);
        let parts: Vec<Option<String>> = target.split('/').rev().map(|s| Some(s.to_string())).collect();
        part_count += parts.len();
        rest.extend(parts);
    }
    Ok(path)
}

// ---- files

pub enum ReadErr {
    /// OSError: the scripts catch it
    Os,
    /// UnicodeDecodeError: the scripts do not
    Crash(Crash),
}

/// open(path).read() in text mode (UTF-8, universal newlines).
pub fn read_text(path: &str) -> Result<String, ReadErr> {
    if path.contains('\0') {
        return Err(ReadErr::Crash(Crash::new("ValueError: embedded null byte")));
    }
    let b = fs::read(path).map_err(|_| ReadErr::Os)?;
    match String::from_utf8(b) {
        Ok(s) => Ok(universal_newlines(s)),
        Err(e) => Err(ReadErr::Crash(Crash::new(format!("UnicodeDecodeError: {}", e)))),
    }
}

/// read_text where an OSError is "nothing" (None) and a decode error ends the program.
pub fn read_text_opt(path: &str) -> PyResult<Option<String>> {
    match read_text(path) {
        Ok(s) => Ok(Some(s)),
        Err(ReadErr::Os) => Ok(None),
        Err(ReadErr::Crash(c)) => Err(c),
    }
}

/// The lines of a text file as `for line in f` gives them (each with its "\n", the last maybe without).
pub fn lines_keepends(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c == '\n' {
            out.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// open(path, "a").write(text)
pub fn append_text(path: &str, text: &str) -> io::Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new().append(true).create(true).mode(0o666).open(path)?;
    f.write_all(text.as_bytes())
}

/// open(path, "w").write(text)
pub fn write_text(path: &str, text: &str) -> io::Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o666).open(path)?;
    f.write_all(text.as_bytes())
}

/// os.makedirs(path, mode, exist_ok=True): the mode is only for the last folder (the ones above get 0o777, less the umask).
pub fn makedirs(path: &str, mode: u32) -> io::Result<()> {
    if path.is_empty() {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    let head = dirname(path);
    let tail = basename(path);
    let (head, tail) = if tail.is_empty() {
        let h = dirname(&head);
        let t = basename(&head).to_string();
        (h, t)
    } else {
        (head, tail.to_string())
    };
    if !head.is_empty() && !tail.is_empty() && !exists(&head) {
        match makedirs(&head, 0o777) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        if tail == "." {
            return Ok(());
        }
    }
    match fs::DirBuilder::new().mode(mode).create(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            if isdir(path) {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

/// os.listdir(path)
pub fn listdir(path: &str) -> io::Result<Vec<String>> {
    Ok(fs::read_dir(path)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
}

// ---- the clock

/// A local time, as datetime.datetime.now() gives it (seconds are enough here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Monday, as datetime.weekday()
    pub weekday: u32,
}

const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

impl LocalTime {
    pub fn now() -> LocalTime {
        if let Some(t) = test_now() {
            return t;
        }
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            LocalTime {
                year: tm.tm_year + 1900,
                month: (tm.tm_mon + 1) as u32,
                day: tm.tm_mday as u32,
                hour: tm.tm_hour as u32,
                minute: tm.tm_min as u32,
                second: tm.tm_sec as u32,
                weekday: ((tm.tm_wday + 6) % 7) as u32,
            }
        }
    }

    pub fn from_parts(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> LocalTime {
        let wd = (days_from_civil(year, month, day) + 3).rem_euclid(7) as u32; // 1970-01-01 was a Thursday
        LocalTime { year, month, day, hour, minute, second, weekday: wd }
    }

    /// %A
    pub fn day_name(&self) -> &'static str {
        DAYS[self.weekday as usize]
    }

    /// %H:%M
    pub fn hm(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// %Y-%m-%d (date.isoformat())
    pub fn date_iso(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// %Y-%m
    pub fn year_month(&self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }

    /// %Y-%m-%d %H:%M
    pub fn ymd_hm(&self) -> String {
        format!("{} {}", self.date_iso(), self.hm())
    }

    /// %Y-%m-%d %H:%M:%S
    pub fn ymd_hms(&self) -> String {
        format!("{} {}:{:02}", self.date_iso(), self.hm(), self.second)
    }

    /// %Y-%m-%dT%H:%M:%S
    pub fn iso_t(&self) -> String {
        format!("{}T{}:{:02}", self.date_iso(), self.hm(), self.second)
    }

    pub fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// This moment minus whole days/hours (datetime - timedelta), in local wall time as Python's naive datetime does.
    pub fn minus_seconds(&self, secs: i64) -> LocalTime {
        let total = self.days() * 86400 + (self.hour as i64) * 3600 + (self.minute as i64) * 60 + self.second as i64 - secs;
        let days = total.div_euclid(86400);
        let rem = total.rem_euclid(86400);
        let (y, m, d) = civil_from_days(days);
        LocalTime::from_parts(y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as u32)
    }
}

/// The test hook may fix the clock (SARINA_VOICE_TEST_NOW=YYYY-MM-DDTHH:MM:SS); the installed binary is built without it.
fn test_now() -> Option<LocalTime> {
    #[cfg(feature = "testhook")]
    {
        let v = std::env::var("SARINA_VOICE_TEST_NOW").ok()?;
        return parse_iso_datetime(&v);
    }
    #[cfg(not(feature = "testhook"))]
    None
}

pub fn parse_iso_datetime(v: &str) -> Option<LocalTime> {
    let b = v.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let n = |s: &str| s.parse::<u32>().ok();
    let (y, m, d) = (v[0..4].parse::<i32>().ok()?, n(&v[5..7])?, n(&v[8..10])?);
    Some(LocalTime::from_parts(y, m, d, n(&v[11..13])?, n(&v[14..16])?, n(&v[17..19])?))
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// datetime.date.fromisoformat for "YYYY-MM-DD" (ASCII digits only, a real date), else None (Python: ValueError).
pub fn date_fromiso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !s.chars().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()) {
        return None;
    }
    let y: i32 = s[0..4].parse().ok()?;
    let m: u32 = s[5..7].parse().ok()?;
    let d: u32 = s[8..10].parse().ok()?;
    if y < 1 || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
    }
}

/// time.monotonic()
pub fn monotonic() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

/// time.time()
pub fn time_now() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

// ---- numbers

/// float(s) as Python parses it, or None (ValueError).
pub fn parse_float(s: &str) -> Option<f64> {
    let t = strip(s);
    if t.is_empty() {
        return None;
    }
    // underscores only between digits
    let mut cleaned = String::with_capacity(t.len());
    let chars: Vec<char> = t.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' {
            let ok = i > 0 && i + 1 < chars.len() && chars[i - 1].is_ascii_digit() && chars[i + 1].is_ascii_digit();
            if !ok {
                return None;
            }
            continue;
        }
        if !c.is_ascii() {
            return None;
        }
        cleaned.push(c);
    }
    let l = cleaned.to_ascii_lowercase();
    let body = l.trim_start_matches(['+', '-']);
    if l.len() - body.len() > 1 {
        return None;
    }
    match body {
        "inf" | "infinity" | "nan" => {}
        _ => {
            if !body.chars().all(|c| c.is_ascii_digit() || c == '.' || c == 'e' || c == '+' || c == '-') {
                return None;
            }
        }
    }
    cleaned.parse::<f64>().ok()
}

/// int(s) as Python parses a str (whitespace around, a sign, digits of any script, "_" between digits), or None (ValueError).
pub fn parse_int(s: &str) -> Option<i128> {
    let t = strip(s);
    let (neg, body) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut v: i128 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = match c.to_digit(10) {
            Some(d) if c.is_ascii() => d as i128,
            _ if !c.is_ascii() && crate::rx::is_unicode_decimal(c) => crate::rx::unicode_digit_value(c) as i128,
            _ => return None,
        };
        v = v.checked_mul(10)?.checked_add(d)?;
    }
    Some(if neg { -v } else { v })
}

/// "%.2f" % x
pub fn fmt2(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    format!("{:.2}", x)
}

/// repr(float) (also str(float)): the shortest digits that read back the same, in Python's layout.
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let e = format!("{:e}", x);
    let (mant, exp) = e.split_once('e').unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let exp: i32 = exp.parse().unwrap();
    let decpt = exp + 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if decpt > 16 || decpt < -3 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let ee = decpt - 1;
        out.push('e');
        out.push(if ee < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", ee.abs()));
    } else if decpt <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-decpt) as usize));
        out.push_str(&digits);
    } else if decpt as usize >= digits.len() {
        out.push_str(&digits);
        out.push_str(&"0".repeat(decpt as usize - digits.len()));
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// repr(str)
pub fn str_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if !c.is_ascii() && !crate::rx::is_printable(c) => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{:02x}", n));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{:04x}", n));
                } else {
                    out.push_str(&format!("\\U{:08x}", n));
                }
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// sys.exit(code) for a subprocess's returncode (negative for a signal): the status is the low 8 bits.
pub fn exit_status(code: i32) -> i32 {
    code & 0xff
}

/// Write to stdout and flush; a closed pipe ends the program like Python's BrokenPipeError does (status 1).
pub fn print_flush(s: &str) {
    use std::io::Write;
    let mut o = io::stdout().lock();
    if o.write_all(s.as_bytes()).and_then(|_| o.write_all(b"\n")).and_then(|_| o.flush()).is_err() {
        std::process::exit(1);
    }
}
