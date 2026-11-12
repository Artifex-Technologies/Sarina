//! Go: `go/tools/readfile.go` and `writefile.go`: `read_file`, `list_dir`, `write_file`, `edit_file`, the per-context file
//! state behind their guards (edit loops, summary churn, files she wrote, files read in this step), and the bounds on what
//! they read (testdata/parity/files.json). The checks of code files after a change are `verify.rs`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Read};
use std::sync::{LazyLock, Mutex};

use regex::Regex;

use crate::goany::Any;
use crate::gotext::{self, to_valid_utf8, trim_space, trim_space_bytes};
use crate::tools::exfil::exfil_risk;
use crate::tools::fsutil::{preview, real_root, rel_path, resolve_for_write, resolve_in_root, truncate_output};
use crate::tools::private::{is_private, private_guard};
use crate::tools::registry::{Context, Output, ToolError};
use crate::tools::secret::secret_guard;
use crate::{goerr, goos, gopath, goregexp};

/// A whole-file read (and the file an edit or a rewrite reads first) is at most this large.
pub const MAX_WHOLE_FILE_READ: u64 = 32 << 20;
/// What a line-range read gathers at most before its answer is cut to MaxOutputChars.
pub const MAX_RANGE_READ: usize = 32 << 20;
const EDIT_LOOP_LIMIT: i64 = 5;
const SUMMARY_WRITE_LIMIT: i64 = 4;

const AUTHORED_BANNER: &str = "[note: this involves a file you wrote or edited earlier in this session. What it shows is not an \
                               independent measurement; treat results derived from your own files with skepticism.]\n";

fn re(go: &str) -> Regex {
    goregexp::compile(go).expect("a fixed pattern")
}

static DEFINITION_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^\s*((pub(\([\w:]+\))?|export|async|static|public|private|protected)\s+)*(fn|struct|enum|trait|impl|mod|func|def|class|function|interface|type)\b",
    )
});
static TOOL_OUTPUT_MARKER: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)<(read_file_output|tool_output|tool_result|function_results?)\b|"tool":\s*"read_file""#));
static CHURN_STEMS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"summary|finding|critical|session_|gaps?[-_]filled|quick[-_]reference|deliverables?|status[-_]report|progress[-_]report|wrap-up|completion[-_]report|implementation[-_]summary|final[-_]summary",
    )
});
static CHURN_EXEMPT: LazyLock<Regex> = LazyLock::new(|| re(r"^(readme|changelog|license|contributing)"));

/// The per-context memory behind the edit-loop, summary-churn and self-authored-file guards (Go's fileState). It belongs to a
/// Context, so it is empty again whenever the agent makes a new one, and it is never saved.
#[derive(Debug, Default)]
pub struct FileState {
    inner: Mutex<StateInner>,
}

#[derive(Debug, Default)]
struct StateInner {
    edits: HashMap<Vec<u8>, i64>,
    summary_writes: i64,
    authored: HashSet<Vec<u8>>,
    /// files read whole or in part, and the step of the last read
    read: HashMap<Vec<u8>, i64>,
    /// per Go package checked: the test that failed in the last check, run first next time
    failed_test: HashMap<String, String>,
}

impl FileState {
    fn lock(&self) -> std::sync::MutexGuard<'_, StateInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn is_authored(&self, path: &[u8]) -> bool {
        self.lock().authored.contains(path)
    }

    pub fn mark_read(&self, path: &[u8], step: i64) {
        self.lock().read.insert(path.to_vec(), step);
    }

    pub fn was_read(&self, path: &[u8]) -> bool {
        self.lock().read.contains_key(path)
    }

    /// Whether the last read of path was in this very step (so its result was not yet in the model's hands).
    pub fn read_in_step(&self, path: &[u8], step: i64) -> bool {
        step > 0 && self.lock().read.get(path) == Some(&step)
    }

    pub fn mark_authored(&self, path: &[u8]) {
        self.lock().authored.insert(path.to_vec());
    }

    /// The sorted base names of authored files the command mentions (by full path or by base name).
    pub fn authored_in(&self, command: &str) -> Vec<String> {
        let st = self.lock();
        let mut names: Vec<String> = Vec::new();
        for p in &st.authored {
            let base = gotext::lossy(&gopath::base(p));
            let full = gotext::lossy(p);
            if (command.contains(&full) || command.contains(&base)) && !names.contains(&base) {
                names.push(base);
            }
        }
        names.sort();
        names
    }

    /// The test that failed in the last check of a package ("" for none).
    pub fn failed_test(&self, key: &str) -> String {
        self.lock().failed_test.get(key).cloned().unwrap_or_default()
    }

    /// Records the test that failed for a package ("" forgets it).
    pub fn set_failed_test(&self, key: &str, test: &str) {
        let mut st = self.lock();
        if test.is_empty() {
            st.failed_test.remove(key);
        } else {
            st.failed_test.insert(key.to_owned(), test.to_owned());
        }
    }

    /// Go's `ownTestOnOthersCode`: path is a test file she created in this session, and she created or edited no other
    /// (non-test) file in its folder, so a failure is about code she did not write. `is_test` says it of a base name.
    pub fn own_test_on_others_code(&self, path: &[u8], is_test: &dyn Fn(&[u8]) -> bool) -> bool {
        if !is_test(&gopath::base(path)) {
            return false;
        }
        let st = self.lock();
        if !st.authored.contains(path) {
            return false;
        }
        let dir = gopath::dir(path);
        let other = |p: &Vec<u8>| gopath::dir(p) == dir && !is_test(&gopath::base(p));
        !(st.authored.iter().any(other) || st.edits.iter().any(|(p, n)| *n > 0 && other(p)))
    }

    /// Counts one write or edit attempt on path; true when the edit-loop guard now refuses it.
    fn attempt(&self, path: &[u8]) -> (i64, bool) {
        let mut st = self.lock();
        let n = st.edits.entry(path.to_vec()).or_insert(0);
        *n += 1;
        (*n, *n > EDIT_LOOP_LIMIT)
    }
}

// ---- small argument helpers (the schema has already checked the types) ----

pub fn arg_str(args: &BTreeMap<String, Any>, key: &str) -> String {
    match args.get(key) {
        Some(Any::String(s)) => s.clone(),
        _ => String::new(),
    }
}

pub fn arg_int(args: &BTreeMap<String, Any>, key: &str, def: i64) -> i64 {
    crate::config::as_int(args.get(key)).unwrap_or(def)
}

pub fn arg_bool(args: &BTreeMap<String, Any>, key: &str, def: bool) -> bool {
    match args.get(key) {
        Some(Any::Bool(b)) => *b,
        _ => def,
    }
}

pub(crate) fn ok(content: String) -> Output {
    Output { content, ..Output::default() }
}

pub(crate) fn fail(text: String) -> Output {
    Output { error: Some(ToolError::Failed(text)), ..Output::default() }
}

pub(crate) fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn clip_text(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{} [...]", &s[..i]),
        None => s.to_owned(),
    }
}

fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

fn int(v: usize) -> Any {
    Any::Int(v as i64)
}

fn lossy(b: &[u8]) -> String {
    gotext::lossy(b)
}

/// The text Go's `errors.Is(err, fs.ErrNotExist)` looks for at the end of a path error.
const NOT_EXIST: &str = ": no such file or directory";

// ---- read_file -----------------------------------------------------------------------------------------------------------

/// Go's `read_file` handler.
pub fn read_file(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let given = arg_str(args, "path");
    let path = match resolve_in_root(&tc.root, &given) {
        Ok(p) => p,
        Err(e) => {
            let nf = e.ends_with(NOT_EXIST);
            return fail(same_name_hint(&tc.root, &given, e, nf));
        }
    };
    if let Err(e) = secret_guard(&given, &path, "read").and_then(|_| private_guard(&given, &path, "read")) {
        return fail(e);
    }
    let st = match std::fs::metadata(gopath::os_path(&path)) {
        Ok(m) => m,
        Err(e) => {
            let text = format!("cannot read {given}: {}", goerr::path("stat", &path, &e));
            return fail(same_name_hint(&tc.root, &given, text, e.raw_os_error() == Some(2)));
        }
    };
    if !st.is_file() {
        return fail(format!("{given} is not a regular file"));
    }
    tc.state.mark_read(&path, tc.step);
    let whole = !args.contains_key("start_line") && !args.contains_key("end_line");
    let mut out;
    if whole {
        if st.len() > MAX_WHOLE_FILE_READ {
            return fail(format!("{given} is {} bytes, too large to read whole; use start_line and end_line", st.len()));
        }
        match read_bounded(&path) {
            Ok(data) => out = to_valid_utf8(&data),
            Err(e) => return fail(format!("cannot read {given}: {e}")),
        }
    } else {
        let (start, end) = (arg_int(args, "start_line", 1), arg_int(args, "end_line", i64::MAX));
        out = match read_lines(&path, start, end) {
            Ok(o) => o,
            Err(e) => return fail(format!("cannot read {given}: {e}")),
        };
        if out.is_empty() {
            out = "(no lines in the requested range)".into();
        }
        let total = count_lines(&path);
        if end < total {
            out.push_str(&format!(
                "\n[lines {start} to {end} of {total} shown; {} more after these. grep_search finds a name in the whole file faster than reading on]",
                total - end
            ));
        }
    }
    if tc.state.is_authored(&path) {
        out = format!("{AUTHORED_BANNER}{out}");
    }
    if whole {
        return ok(truncate_file(&out, tc.max_output_chars));
    }
    ok(truncate_output(&out, tc.max_output_chars))
}

/// Go's `readBounded`: a whole file of at most MAX_WHOLE_FILE_READ bytes; a larger one (it may have grown since its size was
/// checked) is refused, not read.
pub fn read_bounded(path: &[u8]) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(gopath::os_path(path)).map_err(|e| goerr::path("open", path, &e))?;
    let mut data = Vec::new();
    f.take(MAX_WHOLE_FILE_READ + 1).read_to_end(&mut data).map_err(|e| goerr::path("read", path, &e))?;
    if data.len() as u64 > MAX_WHOLE_FILE_READ {
        return Err(format!("it is larger than {MAX_WHOLE_FILE_READ} bytes"));
    }
    Ok(data)
}

/// Go's `sameNameHint`: a "no such file" error names the workspace files with the same name.
fn same_name_hint(root: &str, given: &str, err: String, not_exist: bool) -> String {
    if !not_exist || root.is_empty() {
        return err;
    }
    let found = same_name_files(&real_root(root.as_bytes()), &gopath::base(given.as_bytes()), 5);
    if found.is_empty() {
        return err;
    }
    format!("{err}. The workspace has a file with that name: {} (relative to the workspace). Read that one", found.join(", "))
}

/// Go's `sameNameFiles`: up to max files named name under root (relative), skipping private and version-control folders,
/// after a bounded walk.
fn same_name_files(root: &[u8], name: &[u8], max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = 0;
    let _ = gopath::walk_dir(root, &mut |p, d, err| {
        if out.len() >= max || seen > 20000 {
            return gopath::Walk::SkipAll;
        }
        seen += 1;
        let Some(d) = d else { return gopath::Walk::Continue };
        if err.is_some() || p == root {
            return gopath::Walk::Continue;
        }
        if d.kind == gopath::Kind::Dir {
            if is_private(p) || d.name == b".git" || d.name == b"node_modules" {
                return gopath::Walk::SkipDir;
            }
            return gopath::Walk::Continue;
        }
        if d.name == name && d.kind == gopath::Kind::Regular && !is_private(p) {
            out.push(lossy(&gopath::rel(root, p).unwrap_or_default()));
        }
        gopath::Walk::Continue
    });
    out
}

/// Go's `truncateFile`: a whole-file read cut as TruncateOutput cuts, naming the lines left out and the definitions in them.
fn truncate_file(s: &str, max: i64) -> String {
    let r: Vec<char> = s.chars().collect();
    if max <= 0 || r.len() as i64 <= max {
        return s.to_owned();
    }
    let max = max as usize;
    let head = max * 2 / 3;
    let tail = max - head;
    let nl = |cs: &[char]| cs.iter().filter(|&&c| c == '\n').count();
    let head_lines = nl(&r[..head]);
    let total = s.matches('\n').count() + 1;
    let tail_lines = nl(&r[r.len() - tail..]);
    let (from, to) = (head_lines + 1, total - tail_lines);
    let mut out: String = r[..head].iter().collect();
    out.push_str(&format!("\n[... lines {from}-{to} of {total} omitted: read_file start_line/end_line, or grep_search ...]\n"));
    out.push_str(&outline(s, from, to));
    out.extend(&r[r.len() - tail..]);
    out
}

/// Go's `outline`: the definitions in the lines a cut read left out, with their numbers.
fn outline(s: &str, from: usize, to: usize) -> String {
    let mut b = String::new();
    let mut n = 0;
    for (i, l) in s.split('\n').enumerate() {
        if i + 1 < from || i + 1 > to || !DEFINITION_RE.is_match(l) {
            continue;
        }
        if n == 40 {
            b.push_str("  (more; grep_search finds the rest)\n");
            break;
        }
        b.push_str(&format!("  {}: {}\n", i + 1, clip_text(trim_space(l), 120)));
        n += 1;
    }
    if n == 0 {
        return String::new();
    }
    format!(
        "[defined in the omitted lines, first line only: read a definition's lines (start_line/end_line) before you say what it does or quote it]\n{b}[...]\n"
    )
}

/// Go's `readLines`: lines start to end, numbered, gathering at most MAX_RANGE_READ bytes; lines before start are skipped
/// without being kept.
pub fn read_lines(path: &[u8], start: i64, end: i64) -> Result<String, String> {
    let f = std::fs::File::open(gopath::os_path(path)).map_err(|e| goerr::path("open", path, &e))?;
    let mut br = std::io::BufReader::with_capacity(4096, f);
    let mut b = String::new();
    let mut n: i64 = 1;
    while n <= end {
        let mut raw: Vec<u8> = Vec::new();
        let mut eof = false;
        loop {
            let buf = match br.fill_buf() {
                Ok(buf) => buf,
                Err(e) => return Err(goerr::path("read", path, &e)),
            };
            if buf.is_empty() {
                eof = true;
                break;
            }
            let (take, done) = match buf.iter().position(|&c| c == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            if n >= start {
                if b.len() + raw.len() + take > MAX_RANGE_READ {
                    return Err(format!("the lines asked for hold more than {MAX_RANGE_READ} bytes; ask for fewer lines"));
                }
                raw.extend_from_slice(&buf[..take]);
            }
            br.consume(take);
            if done {
                break;
            }
        }
        if !raw.is_empty() {
            let mut line: &[u8] = &raw;
            line = line.strip_suffix(b"\n").unwrap_or(line);
            line = line.strip_suffix(b"\r").unwrap_or(line);
            b.push_str(&format!("{n}: {}\n", to_valid_utf8(line)));
        }
        if eof {
            break;
        }
        n += 1;
    }
    Ok(b.strip_suffix('\n').map(str::to_owned).unwrap_or(b))
}

/// Go's `countLines`: the number of lines, a last line without a line break counted (0 for a file it cannot open).
fn count_lines(path: &[u8]) -> i64 {
    let Ok(mut f) = std::fs::File::open(gopath::os_path(path)) else { return 0 };
    let mut buf = vec![0u8; 64 * 1024];
    let (mut n, mut last) = (0i64, b'\n');
    loop {
        match f.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                n += buf[..k].iter().filter(|&&c| c == b'\n').count() as i64;
                last = buf[k - 1];
            }
        }
    }
    if last != b'\n' {
        n += 1;
    }
    n
}

// ---- list_dir ------------------------------------------------------------------------------------------------------------

/// Go's `list_dir` handler.
pub fn list_dir(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let mut given = arg_str(args, "path");
    if given.is_empty() {
        given = ".".into();
    }
    let dir = match resolve_in_root(&tc.root, &given) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    if let Err(e) = private_guard(&given, &dir, "list") {
        return fail(e);
    }
    let entries = match gopath::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return fail(format!("cannot list {given}: {e}")),
    };
    let mut list: Vec<(Vec<u8>, bool)> = entries
        .into_iter()
        .map(|e| {
            let mut is_dir = e.kind == gopath::Kind::Dir;
            if e.kind == gopath::Kind::Symlink {
                if let Ok(m) = std::fs::metadata(gopath::os_path(&[&dir[..], b"/", &e.name].concat())) {
                    is_dir = m.is_dir();
                }
            }
            (e.name, is_dir)
        })
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| gotext::to_lower_bytes(&a.0).cmp(&gotext::to_lower_bytes(&b.0))));
    if list.is_empty() {
        return ok("(empty directory)".into());
    }
    let limit = arg_int(args, "max_entries", 200);
    let mut b = String::new();
    for (i, (name, is_dir)) in list.iter().enumerate() {
        if i as i64 >= limit {
            b.push_str(&format!("[listing truncated at {limit} entries]\n"));
            break;
        }
        let kind = if *is_dir { "dir" } else { "file" };
        b.push_str(&format!("{kind}\t{}\n", lossy(&rel_path(&tc.root, &[&dir[..], b"/", name].concat()))));
    }
    ok(truncate_output(b.trim_end_matches('\n'), tc.max_output_chars))
}

// ---- write_file and edit_file ----------------------------------------------------------------------------------------------

fn edit_loop_error(path: &str, n: i64) -> String {
    format!(
        "refusing attempt {n} to change {path}: you have already tried more than {EDIT_LOOP_LIMIT} times in this session. Stop editing it. \
         Before touching it again, explain to the user what fundamentally different change or hypothesis you are testing and why the \
         earlier attempts did not work"
    )
}

pub(crate) fn exfil_refusal(path: &str, why: &str) -> String {
    format!(
        "refused: the content for {path} would take keys or passwords off this computer: {why}. This is not written, whoever asks. \
         Tell the user plainly that you will not write it, and why; offer a safe alternative only if one fits (backing up keys to a \
         local, encrypted place the owner controls, say)."
    )
}

/// Go's strings.EqualFold(s, "docs"), with the one non-ASCII fold that reaches it (U+017F folds to s).
fn is_docs(seg: &str) -> bool {
    let c: Vec<char> = seg.chars().collect();
    c.len() == 4
        && c[0].eq_ignore_ascii_case(&'d')
        && c[1].eq_ignore_ascii_case(&'o')
        && c[2].eq_ignore_ascii_case(&'c')
        && (c[3].eq_ignore_ascii_case(&'s') || c[3] == '\u{17F}')
}

/// Go's `isChurnMarkdown`: the throwaway "summary of what I did" documents a weak model writes instead of making progress.
fn is_churn_markdown(path: &str) -> bool {
    let segs: Vec<&str> = path.split('/').collect();
    let base = gotext::to_lower(segs[segs.len() - 1]);
    if !base.ends_with(".md") || CHURN_EXEMPT.is_match(&base) {
        return false;
    }
    if segs[..segs.len() - 1].iter().any(|s| is_docs(s)) {
        return false;
    }
    CHURN_STEMS.is_match(&base)
}

/// Go's `isBinary`: a NUL byte in the first 8 KiB, or text that is not UTF-8 (at its start and as a whole).
fn is_binary(data: &[u8]) -> bool {
    let head = &data[..data.len().min(8192)];
    head.contains(&0) || std::str::from_utf8(head).is_err() && std::str::from_utf8(data).is_err()
}

fn final_line_break(s: &[u8]) -> &'static str {
    if s.ends_with(b"\r\n") {
        "\r\n"
    } else if s.ends_with(b"\n") {
        "\n"
    } else {
        ""
    }
}

/// Go's `verifyOnDisk`: the file read back (at most one byte more than was written) holds exactly what was meant.
fn verify_on_disk(path: &[u8], want: &[u8]) -> Result<(), String> {
    let f = std::fs::File::open(gopath::os_path(path)).map_err(|e| format!("cannot read it back: {}", goerr::path("open", path, &e)))?;
    let mut got = Vec::new();
    f.take(want.len() as u64 + 1).read_to_end(&mut got).map_err(|e| format!("cannot read it back: {}", goerr::path("read", path, &e)))?;
    if got.len() > want.len() {
        return Err(format!("it holds more than {} bytes, expected {}", want.len(), want.len()));
    }
    if got != want {
        return Err(format!("it holds {} bytes, expected {}", got.len(), want.len()));
    }
    Ok(())
}

fn verify_code(path: &[u8], tc: &Context) -> (String, BTreeMap<String, Any>) {
    crate::tools::verify::verify_code(path, tc)
}

/// Byte-string search with std's string search: text that is not UTF-8 is mapped byte for byte to the characters U+0000 to
/// U+00FF first (on both sides, so the positions agree), never held in a `str` it is not.
fn as_l1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn from_l1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u8).collect()
}

fn utf8_pair<'a>(a: &'a [u8], b: &'a [u8]) -> Option<(&'a str, &'a str)> {
    Some((std::str::from_utf8(a).ok()?, std::str::from_utf8(b).ok()?))
}

fn bcount(hay: &[u8], needle: &[u8]) -> usize {
    match utf8_pair(hay, needle) {
        Some((h, n)) => h.matches(n).count(),
        None => as_l1(hay).matches(&as_l1(needle)).count(),
    }
}

fn bcontains(hay: &[u8], needle: &[u8]) -> bool {
    match utf8_pair(hay, needle) {
        Some((h, n)) => h.contains(n),
        None => as_l1(hay).contains(&as_l1(needle)),
    }
}

fn breplace(hay: &[u8], old: &[u8], new: &[u8], all: bool) -> Vec<u8> {
    let n = if all { usize::MAX } else { 1 };
    match (utf8_pair(hay, old), std::str::from_utf8(new)) {
        (Some((h, o)), Ok(w)) => h.replacen(o, w, n).into_bytes(),
        _ => from_l1(&as_l1(hay).replacen(&as_l1(old), &as_l1(new), n)),
    }
}

/// Go's `write_file` handler.
pub fn write_file(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let given = arg_str(args, "path");
    let mut content = arg_str(args, "content");
    let path = match resolve_for_write(&tc.root, &given) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    if let Err(e) = secret_guard(&given, &path, "write").and_then(|_| private_guard(&given, &path, "write")) {
        return fail(e);
    }
    let st = &tc.state;
    let rel = lossy(&rel_path(&tc.root, &path));
    if is_churn_markdown(&rel) {
        let over = {
            let mut g = st.lock();
            g.summary_writes += 1;
            g.summary_writes > SUMMARY_WRITE_LIMIT
        };
        if over {
            return fail(format!(
                "refusing to write {given}: you have written several summary or report documents already. More of them is not progress. \
                 Write code or tests, or tell the user the result directly"
            ));
        }
    }
    let (n, refused) = st.attempt(&path);
    if refused {
        return fail(edit_loop_error(&given, n));
    }
    let known = st.is_authored(&path) || st.was_read(&path);
    if st.read_in_step(&path, tc.step) && !st.is_authored(&path) {
        return fail(format!(
            "refused: you asked to read {given} and to write it in the same step, so this content was written before you saw the file. \
             Read the result first, then write the whole new content in your next step, or add to it with edit_file"
        ));
    }
    if TOOL_OUTPUT_MARKER.is_match(&content) {
        return fail(format!(
            "refused: the content for {given} holds a tool-output marker (such as <read_file_output>) instead of the file's text. Write \
             the real text of the whole file, or add to it with edit_file"
        ));
    }
    let why = exfil_risk(&content);
    if !why.is_empty() {
        return fail(exfil_refusal(&given, &why));
    }
    let mut md = BTreeMap::from([
        ("action".to_owned(), s("write_file")),
        ("path".to_owned(), Any::String(rel.clone())),
        ("content_length".to_owned(), int(content.chars().count())),
    ]);
    let mut kept_eol = "";
    match std::fs::metadata(gopath::os_path(&path)) {
        Ok(info) if info.is_dir() => return fail(format!("{given} is a directory")),
        // a named pipe or a device: reading what it "holds" would wait for a writer forever
        Ok(info) if !info.is_file() => return fail(format!("{given} is not a regular file")),
        Ok(_) => {
            let data = match read_bounded(&path) {
                Ok(d) => d,
                Err(e) => return fail(format!("cannot read the existing {given}: {e}")),
            };
            if is_binary(&data) {
                return fail(format!(
                    "refused: {given} is a binary file (an image, archive or program); writing text into it would break it. Leave it as \
                     it is and tell the user"
                ));
            }
            let before_trim = trim_space_bytes(&data);
            if !before_trim.is_empty() && !known && !bcontains(content.as_bytes(), before_trim) {
                return fail(format!(
                    "refused: {given} already has content you have not read in this task, and writing replaces all of it. Read it first, \
                     then write the whole new content; or change part of it with edit_file"
                ));
            }
            if !before_trim.is_empty() && trim_space(&content).is_empty() {
                return fail(format!(
                    "refused: writing nothing over {given} would erase what is in it. There is no tool here to delete or empty files: leave \
                     it as it is and tell the user so"
                ));
            }
            md.insert("before_exists".into(), Any::Bool(true));
            md.insert("before_sha256".into(), Any::String(sha256_hex(&data)));
            md.insert("before_size".into(), int(data.len()));
            md.insert("before_preview".into(), Any::String(preview(&lossy(&data), 240)));
            let eol = final_line_break(&data);
            if !eol.is_empty() && !content.is_empty() && !content.ends_with('\n') {
                content.push_str(eol);
                kept_eol = " The file ended with a line break, and still does.";
            }
        }
        Err(_) => {
            for k in ["before_sha256", "before_size", "before_preview"] {
                md.insert(k.into(), Any::Null);
            }
            md.insert("before_exists".into(), Any::Bool(false));
        }
    }
    // authored only once every refusal is behind (a refused write that marked it let the same write through on a retry)
    st.mark_authored(&path);
    if let Err(e) = goos::mkdir_all(&gopath::dir(&path)) {
        return fail(format!("cannot create directories for {given}: {e}"));
    }
    if let Err(e) = goos::write_file(&path, content.as_bytes(), 0o644) {
        return fail(format!("cannot write {given}: {e}"));
    }
    if let Err(e) = verify_on_disk(&path, content.as_bytes()) {
        return fail(format!("wrote {given} but reading it back does not match: {e}"));
    }
    md.insert("after_sha256".into(), Any::String(sha256_hex(content.as_bytes())));
    md.insert("after_size".into(), int(content.len()));
    md.insert("after_preview".into(), Any::String(preview(&content, 240)));
    md.insert("verified_on_disk".into(), Any::Bool(true));
    let (note, vmd) = verify_code(&path, tc);
    md.extend(vmd);
    Output {
        content: format!("Wrote {rel} ({} characters). Read back from disk: it matches.{kept_eol}{note}", content.chars().count()),
        metadata: md,
        error: None,
    }
}

/// Go's `nearestLines`: where old_text's first line is in the file, and what follows it there, numbered.
fn nearest_lines(file: &[u8], old: &str) -> String {
    let Some(first) = old.split('\n').map(trim_space).find(|l| !l.is_empty()) else { return String::new() };
    let lines: Vec<&[u8]> = file.split(|&c| c == b'\n').collect();
    for (i, l) in lines.iter().enumerate() {
        if trim_space_bytes(l) != first.as_bytes() {
            continue;
        }
        let end = lines.len().min(i + old.matches('\n').count() + 3);
        let mut b = String::new();
        for (j, l) in lines.iter().enumerate().take(end).skip(i) {
            b.push_str(&format!("{}: {}\n", j + 1, lossy(l)));
        }
        return format!(
            ". Its first line is at line {}; the file has there (blank lines and spaces count):\n{}",
            i + 1,
            b.trim_end_matches('\n')
        );
    }
    String::new()
}

/// Go's `edit_file` handler.
pub fn edit_file(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let (given, old_t, new_t) = (arg_str(args, "path"), arg_str(args, "old_text"), arg_str(args, "new_text"));
    let path = match resolve_in_root(&tc.root, &given) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    if let Err(e) = secret_guard(&given, &path, "edit").and_then(|_| private_guard(&given, &path, "edit")) {
        return fail(e);
    }
    let st = &tc.state;
    // counted before validation on purpose: failed edits are still attempts
    let (n, refused) = st.attempt(&path);
    if refused {
        return fail(edit_loop_error(&given, n));
    }
    let info = match std::fs::metadata(gopath::os_path(&path)) {
        Ok(i) if i.is_file() => i,
        _ => return fail(format!("{given} is not a regular file")),
    };
    if info.len() > MAX_WHOLE_FILE_READ {
        return fail(format!("{given} is too large to edit"));
    }
    let before = match read_bounded(&path) {
        Ok(d) => d,
        Err(e) => return fail(format!("cannot read {given}: {e}")),
    };
    if is_binary(&before) {
        return fail(format!(
            "refused: {given} is a binary file (an image, archive or program); editing it as text would break it. Leave it as it is and \
             tell the user"
        ));
    }
    let count = bcount(&before, old_t.as_bytes());
    let replace_all = arg_bool(args, "replace_all", false);
    if old_t == new_t {
        return fail("old_text and new_text are identical; nothing to change".into());
    } else if new_t.contains(&old_t) && bcontains(&before, new_t.as_bytes()) {
        return fail(format!(
            "refused: {given} already contains this change (an earlier edit made it); repeating it would add a second copy. Read the file \
             if you are unsure what it holds now"
        ));
    } else if count == 0 {
        return fail(format!(
            "old_text was not found in {given}. It must match exactly, including whitespace; read the file again and copy the text{}",
            nearest_lines(&before, &old_t)
        ));
    } else if count > 1 && !replace_all {
        return fail(format!(
            "old_text matches {count} places in {given}. Add surrounding lines to make it unique, or set replace_all to change every one"
        ));
    }
    let replaced = if replace_all { count } else { 1 };
    let after = breplace(&before, old_t.as_bytes(), new_t.as_bytes(), replace_all);
    let why = exfil_risk(&lossy(&after));
    if !why.is_empty() && exfil_risk(&lossy(&before)).is_empty() {
        return fail(exfil_refusal(&given, &why));
    }
    if !trim_space_bytes(&before).is_empty() && trim_space_bytes(&after).is_empty() {
        return fail(format!(
            "refused: that edit would leave {given} empty, which erases it. There is no tool here to delete or empty files: leave it as it is \
             and tell the user so"
        ));
    }
    // authored only when it is changed (a failed edit let a later write_file replace the file unread)
    st.mark_authored(&path);
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = goos::write_file(&path, &after, info.permissions().mode() & 0o777) {
        return fail(format!("cannot write {given}: {e}"));
    }
    let rel = lossy(&rel_path(&tc.root, &path));
    let mut md = BTreeMap::from([
        ("action".to_owned(), s("edit_file")),
        ("path".to_owned(), Any::String(rel.clone())),
        ("before_sha256".to_owned(), Any::String(sha256_hex(&before))),
        ("after_sha256".to_owned(), Any::String(sha256_hex(&after))),
        ("before_size".to_owned(), int(before.len())),
        ("after_size".to_owned(), int(after.len())),
        ("before_preview".to_owned(), Any::String(preview(&lossy(&before), 240))),
        ("after_preview".to_owned(), Any::String(preview(&lossy(&after), 240))),
        ("old_text_preview".to_owned(), Any::String(preview(&old_t, 240))),
        ("new_text_preview".to_owned(), Any::String(preview(&new_t, 240))),
        ("replaced_occurrences".to_owned(), int(replaced)),
    ]);
    if let Err(e) = verify_on_disk(&path, &after) {
        return fail(format!("edited {given} but reading it back does not match: {e}"));
    }
    md.insert("verified_on_disk".into(), Any::Bool(true));
    let (note, vmd) = verify_code(&path, tc);
    md.extend(vmd);
    let plural = if replaced == 1 { "" } else { "s" };
    Output {
        content: format!("Edited {rel} ({replaced} replacement{plural}). Read back from disk: it matches.{note}"),
        metadata: md,
        error: None,
    }
}

/// The file tools as Go's DefaultTools lists them: read_file, list_dir, write_file, edit_file, glob_search, grep_search.
pub fn file_tools() -> Vec<crate::tools::registry::Tool> {
    use crate::tools::defs::tool;
    use crate::tools::search::{glob_search, grep_search};
    use std::sync::Arc;
    vec![
        tool("read_file", Arc::new(read_file)),
        tool("list_dir", Arc::new(list_dir)),
        tool("write_file", Arc::new(write_file)),
        tool("edit_file", Arc::new(edit_file)),
        tool("glob_search", Arc::new(glob_search)),
        tool("grep_search", Arc::new(grep_search)),
    ]
}
