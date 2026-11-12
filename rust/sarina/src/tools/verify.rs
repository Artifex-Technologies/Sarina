//! Go: `go/tools/verify.go`, `localise.go` and `checkcode.go`: the check after write_file or edit_file changes a code file
//! (a build or syntax check of its language, and the project's tests where they may run: in a bubblewrap sandbox on a
//! copy of the project, or on the project itself only when shell commands are allowed and there is no sandbox), the line
//! that says which test failed and where its function is, and `check_code`, which runs the same checks on demand
//! (testdata/parity/checks.json, against stand-ins for every program). Everything read from a check is bounded: the
//! first and the last MiB of its output.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::goany::Any;
use crate::gotext::{self, to_valid_utf8, trim_space};
use crate::tools::exec::{child_env, exit_error, go_duration, look_path_go, parent_env, run_program, Capture, HeadTail, Prog};
use crate::tools::files::{arg_str, clip_text, fail, read_bounded, read_lines};
use crate::tools::fsutil::{rel_path, resolve_for_write, resolve_in_root};
use crate::tools::private::private_guard;
use crate::tools::registry::{Context, Output};
use crate::tools::secret::secret_guard;
use crate::{gopath, goregexp};

const VERIFY_TIMEOUT: Duration = Duration::from_secs(90);
/// What is kept of a check's output at each end.
pub const MAX_CHECK_OUTPUT: usize = 1 << 20;
const SANDBOX_MAX_BYTES: u64 = 200 << 20;

fn re(go: &str) -> Regex {
    goregexp::compile(go).expect("a fixed pattern")
}

static CARGO_MANIFEST_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\[package\]"));
static ERR_AT_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?m)(?:^|[\s"(])((?:\.{0,2}/)?[\w./-]+\.(?:go|py|rs|sh|js|mjs)):(\d+)(?::\d+)?|File "([^"]+)", line (\d+)"#));
static INNER_QUOTE_RE: LazyLock<Regex> = LazyLock::new(|| re(r#""[^"\\\n]*[A-Za-z:=] ?"[^",)\n]+"[^"\n]*%[a-z]"#));
static QUOTED_GLOB_RE: LazyLock<Regex> = LazyLock::new(|| re(r#""[^"\n]*/\*[^"\n]*""#));
static TEST_FILE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(^test_.*\.py$|_test\.(py|go)$|\.(test|spec)\.(js|ts|mjs)$|^tests?\.rs$)"));
static GO_FAIL_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*--- FAIL: (\S+)"));
static GO_ASSERT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s+([\w.-]+_test\.go):(\d+): (.*)$"));
static GO_PANIC_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^panic: (.*?)(?: \[recovered[^\]]*\])?$"));
static GO_FRAME_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^(\S+)\(.*\)\n\t(\S+\.go):(\d+)"));
static PY_FRAME_RE: LazyLock<Regex> = LazyLock::new(|| re(r#"(?m)^[ \t]*File "([^"]+)", line (\d+)(?:, in (\S+))?\n(?:[ \t]+(.*)\n)?"#));
static PY_TEST_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^(?:FAIL|ERROR): (\S+) \(([\w.]+)\)"));
static PY_ERROR_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^((?:\w+\.)*\w*(?:Error|Exception|Exit|Interrupt)\b.*)$"));
static CALL_NAME_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\b([A-Za-z_]\w*)\s*\("));
static GO_BUILD_BAD_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\[(?:build|setup) failed\]"));
static GO_FAIL_CALL_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\.(?:Error|Errorf|Fatal|Fatalf|Fail|FailNow)\("));

const CODE_EXTS: &[&str] = &[".go", ".py", ".rs", ".sh", ".bash", ".js", ".mjs"];

fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

fn lossy(b: &[u8]) -> String {
    gotext::lossy(b)
}

fn join(a: &str, b: &str) -> String {
    lossy(&gopath::join(&[a.as_bytes(), b.as_bytes()]))
}

fn dir_of(p: &str) -> String {
    lossy(&gopath::dir(p.as_bytes()))
}

fn base_of(p: &str) -> String {
    lossy(&gopath::base(p.as_bytes()))
}

/// Go's `filepath.Ext`.
fn ext_of(p: &str) -> &str {
    let last = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    match p[last..].rfind('.') {
        Some(i) => &p[last + i..],
        None => "",
    }
}

fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

/// Go's `strconv.Atoi` as these callers use it: the error ignored, an overflow clamped as Go clamps it.
fn atoi(s: &str) -> i64 {
    let mut n: i64 = 0;
    for c in s.bytes() {
        if !c.is_ascii_digit() {
            return 0;
        }
        n = n.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    n
}

/// Go's `regexp.QuoteMeta`.
fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Go's `clipLines`: at most n lines, the first and the last half, with how many were left out.
pub fn clip_lines(s: &str, n: usize) -> String {
    let ls: Vec<&str> = s.split('\n').collect();
    if ls.len() <= n {
        return s.to_owned();
    }
    let h = n / 2;
    format!("{}\n[... {} lines left out ...]\n{}", ls[..h].join("\n"), ls.len() - n, ls[ls.len() - h..].join("\n"))
}

// ---- the decisions --------------------------------------------------------------------------------------------------------

/// Go's `verifyMode`: whether a change's tests run, on the project itself (trusted), in a sandbox. `tier` is asked only
/// when the first two reasons to want tests are absent (it may read Heimdall's grants file).
pub fn verify_mode_with(shell: bool, getenv: &dyn Fn(&str) -> String, bwrap: bool, tier: &dyn Fn() -> i64) -> (bool, bool, bool) {
    let wanted = shell || getenv("SARINA_VERIFY_TESTS") == "on" || tier() >= 2;
    let sandbox = bwrap && getenv("SARINA_SANDBOX_TESTS") != "off";
    let trusted = wanted && !sandbox && shell;
    (trusted || sandbox, trusted, sandbox)
}

fn getenv(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}

fn have_bwrap() -> bool {
    look_path_go("bwrap").is_ok()
}

/// Go's `verifyCode`: the checks for a changed file, in the mode this process's settings give.
pub fn verify_code(path: &[u8], tc: &Context) -> (String, BTreeMap<String, Any>) {
    let tier = || crate::config::access_tier_from_env(&getenv);
    let (tests, trusted, sandbox) = verify_mode_with(tc.permissions.allow_shell_commands, &getenv, have_bwrap(), &tier);
    verify_run(&lossy(path), tc, tests, trusted, sandbox, "shell commands are off in this session")
}

/// Go's `findUp`: the nearest folder at or above dir holding name.
fn find_up(dir: &str, name: &str) -> Option<String> {
    let mut d = dir.to_owned();
    loop {
        if std::fs::metadata(join(&d, name)).is_ok() {
            return Some(d);
        }
        let parent = dir_of(&d);
        if parent == d {
            return None;
        }
        d = parent;
    }
}

fn dir_exists(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// Go's `hasPyTests`: a test_*.py in the folder.
fn has_py_tests(dir: &str) -> bool {
    gopath::read_dir(dir.as_bytes()).is_ok_and(|es| es.iter().any(|e| gopath::path_match(b"test_*.py", &e.name) == Ok(true)))
}

/// Go's `shellLint`: a glob inside double quotes, which the shell takes literally.
fn shell_lint(path: &str) -> String {
    let Ok(b) = read_bounded(path.as_bytes()) else { return String::new() };
    let text = lossy(&b);
    for (i, l) in text.split('\n').enumerate() {
        let t = trim_space(l);
        if t.is_empty() || t.starts_with('#') || t.starts_with("echo ") || t.starts_with("printf ") {
            continue;
        }
        if let Some(m) = QUOTED_GLOB_RE.find(l) {
            return format!(
                "line {}: {} has the * inside double quotes, where the shell does not expand it, so it matches only a file literally \
                 named that. Quote the variable and leave the pattern outside, e.g. \"$1\"/*.tmp",
                i + 1,
                m.as_str()
            );
        }
    }
    String::new()
}

/// Go's `sandboxArgs`: the bubblewrap command line for step, run in copy_dir (the copy of dir in tmp).
#[allow(clippy::too_many_arguments)]
pub fn sandbox_args(
    dir: &str,
    copy_dir: &str,
    tmp: &str,
    home: &str,
    gocache: &str,
    path: &str,
    step: &[String],
    exists: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let args: Vec<String> = step.iter().map(|a| a.replacen(dir, copy_dir, 1)).collect();
    let mut bw: Vec<String> =
        ["bwrap", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp", "--tmpfs", "/run", "--tmpfs", home]
            .iter()
            .map(|s| s.to_string())
            .collect();
    for d in [".cargo", "go", ".local/lib"] {
        let p = join(home, d);
        if exists(&p) {
            bw.extend(["--overlay-src".to_owned(), p.clone(), "--tmp-overlay".to_owned(), p]);
        }
    }
    let path = if path.is_empty() { "/usr/local/bin:/usr/bin" } else { path };
    let tail = [
        "--bind",
        tmp,
        tmp,
        "--bind",
        gocache,
        gocache,
        "--unshare-all",
        "--new-session",
        "--die-with-parent",
        "--clearenv",
        "--setenv",
        "PATH",
        path,
        "--setenv",
        "HOME",
        tmp,
        "--setenv",
        "TMPDIR",
        "/tmp",
        "--setenv",
        "LANG",
        "C.UTF-8",
        "--setenv",
        "GOCACHE",
        &join(gocache, "build"),
        "--setenv",
        "GOMODCACHE",
        &join(gocache, "mod"),
        "--setenv",
        "GOPROXY",
        "off",
        "--setenv",
        "GOFLAGS",
        "-mod=mod",
        "--setenv",
        "GOTOOLCHAIN",
        "local",
        "--setenv",
        "CARGO_NET_OFFLINE",
        "true",
        "--setenv",
        "NO_COLOR",
        "1",
        "--chdir",
        copy_dir,
        "--",
    ];
    bw.extend(tail.iter().map(|s| s.to_string()));
    bw.extend(args);
    bw
}

/// A temporary folder removed when dropped (Go's deferred cleanup of the sandbox copy).
struct TempDir(String);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Go's `os.MkdirTemp("", pattern)`: a new folder (mode 0700) in $TMPDIR or /tmp, named pattern + Go's random number.
fn mkdir_temp(pattern: &str) -> Result<String, String> {
    use ring::rand::SecureRandom;
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::var("TMPDIR").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "/tmp".into());
    let mut tries = 0;
    loop {
        let mut r = [0u8; 4];
        let _ = ring::rand::SystemRandom::new().fill(&mut r);
        let name = join(&base, &format!("{pattern}{}", u32::from_le_bytes(r)));
        match std::fs::DirBuilder::new().mode(0o700).create(&name) {
            Ok(()) => return Ok(name),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 10_000 => tries += 1,
            Err(e) => return Err(crate::goerr::path("mkdir", name.as_bytes(), &e)),
        }
    }
}

/// Go's `sandboxStep`: the project copied to a temporary folder, and the step to run there inside bubblewrap; Err is why
/// there is no sandbox for it.
fn sandbox_step(dir: &str, step: &[String], deadline: Instant, tc: &Context) -> Result<(String, Vec<String>, TempDir), String> {
    let mut size: u64 = 0;
    let _ = gopath::walk_dir(dir.as_bytes(), &mut |p, d, err| {
        let Some(d) = d else { return gopath::Walk::Continue };
        if err.is_some() {
            return gopath::Walk::Continue;
        }
        if d.kind == gopath::Kind::Dir && matches!(&d.name[..], b".git" | b"node_modules" | b"target") {
            return gopath::Walk::SkipDir;
        }
        if d.kind != gopath::Kind::Dir {
            if let Ok(m) = std::fs::symlink_metadata(gopath::os_path(p)) {
                size += m.len();
            }
        }
        if size > SANDBOX_MAX_BYTES {
            return gopath::Walk::SkipAll;
        }
        gopath::Walk::Continue
    });
    if size > SANDBOX_MAX_BYTES {
        return Err("the project is too large to copy into a sandbox".into());
    }
    let tmp = mkdir_temp("sarina-sandbox-").map_err(|e| format!("no sandbox folder: {e}"))?;
    let guard = TempDir(tmp.clone());
    let copy_dir = join(&tmp, "project");
    let mut out = HeadTail::new(MAX_CHECK_OUTPUT);
    let args = vec!["-a".to_owned(), dir.to_owned(), copy_dir.clone()];
    let ok = match look_path_go("cp") {
        Err(_) => false,
        Ok(cp) => {
            let env = parent_env();
            let prog = Prog { path: &cp, args: &args, env: &env, cwd: None, capture: Capture::Both };
            matches!(run_program(&prog, &mut |b| out.write(b), deadline, &tc.ctx), Ok(Some(st)) if st.success())
        }
    };
    if !ok {
        return Err(format!("copying the project failed: {}", trim_space(&to_valid_utf8(&out.bytes()))));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let gocache = join(&home, ".cache/sarina/sandbox-go");
    for sub in ["build", "mod"] {
        use std::os::unix::fs::DirBuilderExt;
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(join(&gocache, sub));
    }
    let argv = sandbox_args(dir, &copy_dir, &tmp, &home, &gocache, &getenv("PATH"), step, &dir_exists);
    Ok((copy_dir, argv, guard))
}

/// One run of a check's program: its output (both ends kept) and Go's error text, if it failed.
fn run_check(argv: &[String], dir: &str, env: &[Vec<u8>], deadline: Instant, tc: &Context) -> (String, Option<String>) {
    let mut out = HeadTail::new(MAX_CHECK_OUTPUT);
    let err = match look_path_go(&argv[0]) {
        Err(e) => Some(e),
        Ok(path) => {
            let prog = Prog { path: &path, args: &argv[1..], env, cwd: Some(dir), capture: Capture::Both };
            match run_program(&prog, &mut |b| out.write(b), deadline, &tc.ctx) {
                Err(e) => Some(e),
                Ok(Some(st)) => exit_error(st),
                Ok(None) => Some("signal: killed".into()),
            }
        }
    };
    (trim_space(&to_valid_utf8(&out.bytes())).to_owned(), err)
}

fn with_vars(mut env: Vec<Vec<u8>>, vars: &[&str]) -> Vec<Vec<u8>> {
    env.extend(vars.iter().map(|v| v.as_bytes().to_vec()));
    env
}

/// Go's `verifyRun`: the checks for path in the given mode; no_tests is why tests did not run when tests is false.
pub fn verify_run(path: &str, tc: &Context, tests: bool, trusted: bool, sandbox: bool, no_tests: &str) -> (String, BTreeMap<String, Any>) {
    let mut md = BTreeMap::from([("verify".to_owned(), s("skipped"))]);
    let ext = gotext::to_lower(ext_of(path));
    let mut dir = dir_of(path);
    let mut steps: Vec<Vec<String>> = Vec::new();
    let mut label = String::new();
    let mut fail_key = String::new();
    let mut test_from = 1;
    let mut vet_fallback: Option<Vec<String>> = None;
    let v = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    match ext.as_str() {
        ".go" => match find_up(&dir, "go.mod") {
            Some(root) => {
                let rel = lossy(&gopath::rel(root.as_bytes(), dir.as_bytes()).unwrap_or_default());
                let pkg = if rel == "." { "./...".to_owned() } else { format!("./{rel}") };
                if tests {
                    fail_key = format!("{root}|{pkg}");
                    let prev = tc.state.failed_test(&fail_key);
                    if !prev.is_empty() {
                        steps.push(v(&["go", "test", "-count=1", "-timeout", "60s", "-run", &format!("^{}$", quote_meta(&prev)), &pkg]));
                    }
                    steps.push(v(&["go", "test", "-count=1", "-timeout", "60s", &pkg]));
                    label = format!("go test {pkg} (it vets the package too)");
                    test_from = 0;
                    vet_fallback = Some(v(&["go", "vet", &pkg]));
                } else {
                    steps.push(v(&["go", "vet", &pkg]));
                    label = format!("go vet {pkg}");
                }
                dir = root;
            }
            None => {
                steps.push(v(&["gofmt", "-l", "-e", path]));
                label = "gofmt -e (no go.mod found, so no build)".into();
            }
        },
        ".py" => {
            steps.push(v(&["python3", "-I", "-m", "py_compile", path]));
            label = "python3 -m py_compile".into();
            if tests && (base_of(path).starts_with("test_") || path.ends_with("_test.py")) {
                steps.push(v(&["python3", "-m", "unittest", "-q", path]));
                label.push_str(" and unittest");
            } else if tests && has_py_tests(&dir) {
                steps.push(v(&["python3", "-m", "unittest", "-q"]));
                label.push_str(" and the folder's unittest tests");
            }
        }
        ".sh" | ".bash" => {
            let l = shell_lint(path);
            if !l.is_empty() {
                md.insert("verify".into(), s("failed"));
                md.insert("verify_output".into(), s(&l));
                return (format!("\nCHECK FAILED (lint): {l}\nFix this before you answer; do not say the script works."), md);
            }
            steps.push(v(&["bash", "-n", path]));
            label = "bash -n".into();
            if look_path_go("shellcheck").is_ok() {
                steps.push(v(&["shellcheck", "-S", "warning", path]));
                label.push_str(" and shellcheck");
            }
        }
        ".js" | ".mjs" => {
            steps.push(v(&["node", "--check", path]));
            label = "node --check".into();
        }
        ".rs" => {
            if let Some(root) = find_up(&dir, "Cargo.toml") {
                if read_bounded(join(&root, "Cargo.toml").as_bytes()).is_ok_and(|d| CARGO_MANIFEST_RE.is_match(&lossy(&d))) {
                    steps.push(v(&["cargo", "check", "--offline", "--quiet"]));
                    label = "cargo check --offline".into();
                    if tests {
                        steps.push(v(&["cargo", "test", "--offline", "--quiet"]));
                        label.push_str(" and cargo test");
                    }
                    dir = root;
                }
            }
        }
        _ => return (String::new(), md),
    }
    if steps.is_empty() {
        return (String::new(), md);
    }
    let mut deadline = Instant::now() + VERIFY_TIMEOUT;
    if let Some(d) = tc.ctx.deadline() {
        deadline = deadline.min(d);
    }
    let mut out = String::new();
    for (i, step) in steps.iter().enumerate() {
        if look_path_go(&step[0]).is_err() {
            return (format!("\nCheck skipped: {} is not installed, so the change was not verified.", step[0]), md);
        }
        let (mut run_dir, mut argv) = (dir.clone(), step.clone());
        let builds = step[0] == "cargo";
        if !trusted && !sandbox && builds {
            md.insert("verify".into(), s("skipped"));
            return (
                format!(
                    "\nCheck skipped: {} would run the project's own build code, and there is no sandbox here (bubblewrap): the change was \
                     not verified.",
                    step[..2].join(" ")
                ),
                md,
            );
        }
        let mut _guard = None;
        if sandbox && (i >= test_from || builds) && step[0] != "shellcheck" {
            match sandbox_step(&dir, step, deadline, tc) {
                Err(why) => {
                    if let (Some(vf), 0) = (&vet_fallback, i) {
                        return vet_only(tc, &dir, path, vf, &why, md, deadline);
                    }
                    md.insert("verify".into(), s("ok"));
                    return (format!("\nCheck passed: the build. Tests were not run ({why}): do not say the tests pass."), md);
                }
                Ok((sd, sargv, guard)) => {
                    run_dir = sd;
                    argv = sargv;
                    _guard = Some(guard);
                }
            }
        }
        let env =
            with_vars(child_env(tc), &["GOPROXY=off", "GOFLAGS=-mod=mod", "GOTOOLCHAIN=local", "CARGO_NET_OFFLINE=true", "NO_COLOR=1"]);
        let (mut text, mut err) = run_check(&argv, &run_dir, &env, deadline, tc);
        if step[0] == "gofmt" && err.is_none() && !text.is_empty() {
            err = Some("syntax errors".into()); // gofmt -l -e prints the problems and exits 0 for some of them
        }
        if let Some(e) = err {
            if Instant::now() >= deadline || tc.ctx.err().is_some() {
                text = format!("the check ran out of time ({})", go_duration(VERIFY_TIMEOUT));
            }
            if text.is_empty() {
                text = e;
            }
            md.insert("verify".into(), s("failed"));
            md.insert("verify_output".into(), s(&clip_text(&text, 12000)));
            let what = if i >= test_from && !GO_BUILD_BAD_RE.is_match(&text) { "tests" } else { "build" };
            md.insert("verify_kind".into(), s(what));
            let (mut head, mut tail) = (String::new(), around(&text, &dir, path));
            if what == "tests" {
                let (summary, lines) = localise(&text, &dir_of(path));
                if !summary.is_empty() {
                    head = format!("{summary}\n");
                }
                if !lines.is_empty() {
                    tail = format!(
                        "\nThe code it points at, as it is now (read it before you edit; an edit's old_text must match these lines):\n{lines}"
                    );
                }
                if let (Some(c), false) = (GO_FAIL_RE.captures(&text), fail_key.is_empty()) {
                    let name = c[1].split('/').next().unwrap_or("").to_owned();
                    tc.state.set_failed_test(&fail_key, &name);
                }
            }
            let mut advice = "Fix this before you answer; do not say the code works.";
            if what == "tests" && tc.state.own_test_on_others_code(path.as_bytes(), &|b| TEST_FILE_RE.is_match(&lossy(b))) {
                advice =
                    "Your test fails on the code it tests: report it (what fails, and whether the test or the code looks wrong); change \
                          that code only if you were asked to fix it.";
                md.insert("verify_own_test".into(), Any::Bool(true));
            }
            return (
                format!("\nCHECK FAILED ({what}, {}):\n{head}{}{tail}\n{advice}", step.join(" "), clip_text(&clip_lines(&text, 40), 3000)),
                md,
            );
        }
        if i >= test_from {
            md.insert("verify_tests".into(), Any::Bool(true));
        }
        if !text.is_empty() {
            out.push_str(&clip_text(&text, 1200));
        }
    }
    if !fail_key.is_empty() {
        tc.state.set_failed_test(&fail_key, "");
    }
    md.insert("verify".into(), s("ok"));
    let mut note = format!("\nCheck passed: {label}.");
    if sandbox && md.get("verify_tests") == Some(&Any::Bool(true)) {
        note.push_str(" (The tests ran in a sandbox: a copy of the project, no network.)");
    }
    if !tests && matches!(ext.as_str(), ".go" | ".rs" | ".py") {
        note.push_str(&format!(" Tests were not run ({no_tests}): do not say the tests pass."));
    }
    let o = trim_space(&out);
    if !o.is_empty() {
        note.push('\n');
        note.push_str(o);
    }
    (note, md)
}

/// Go's `vetOnly`: the build check of a Go change whose tests could not be put in the sandbox.
fn vet_only(
    tc: &Context,
    dir: &str,
    path: &str,
    step: &[String],
    why: &str,
    mut md: BTreeMap<String, Any>,
    deadline: Instant,
) -> (String, BTreeMap<String, Any>) {
    let env = with_vars(child_env(tc), &["GOPROXY=off", "GOFLAGS=-mod=mod", "GOTOOLCHAIN=local", "NO_COLOR=1"]);
    let (mut text, err) = run_check(step, dir, &env, deadline, tc);
    if let Some(e) = err {
        if text.is_empty() {
            text = e;
        }
        md.insert("verify".into(), s("failed"));
        md.insert("verify_output".into(), s(&clip_text(&text, 12000)));
        md.insert("verify_kind".into(), s("build"));
        return (
            format!(
                "\nCHECK FAILED (build, {}):\n{}{}\nFix this before you answer; do not say the code works.",
                step.join(" "),
                clip_text(&clip_lines(&text, 40), 3000),
                around(&text, dir, path)
            ),
            md,
        );
    }
    md.insert("verify".into(), s("ok"));
    (format!("\nCheck passed: the build ({}). Tests were not run ({why}): do not say the tests pass.", step.join(" ")), md)
}

/// Go's `around`: the lines of the file the first error points at, as they are now.
fn around(out: &str, dir: &str, edited: &str) -> String {
    let Some(m) = ERR_AT_RE.captures(out) else { return String::new() };
    let (name, num) = match m.get(1) {
        Some(n) if !n.as_str().is_empty() => (n.as_str().to_owned(), m.get(2).map_or("", |x| x.as_str()).to_owned()),
        _ => (m.get(3).map_or("", |x| x.as_str()).to_owned(), m.get(4).map_or("", |x| x.as_str()).to_owned()),
    };
    let line = atoi(&num);
    let mut p = name.clone();
    if !is_abs(&p) {
        p = join(dir, &p);
        if std::fs::metadata(&p).is_err() {
            p = join(&dir_of(edited), &name);
        }
    }
    let text = match read_lines(p.as_bytes(), (line - 5).max(1), line.wrapping_add(3)) {
        Ok(t) => t,
        Err(_) => return String::new(),
    };
    if text.is_empty() || line <= 0 {
        return String::new();
    }
    let mut hint = String::new();
    for l in text.split('\n') {
        if l.starts_with(&format!("{line}: ")) && INNER_QUOTE_RE.is_match(l) {
            hint = format!(
                "\nLine {line} has a double quote inside a \"...\" string, which ends the string there. Write the inner quote as \\\" (in \
                 your edit's JSON that is \\\\\\\"), or use a `backquoted` string, or print the value with %q."
            );
        }
    }
    format!(
        "\n{} as it is now, around line {line} (read it before you edit again; an edit's old_text must match these lines):\n{text}{hint}",
        base_of(&p)
    )
}

// ---- localise.go --------------------------------------------------------------------------------------------------------------

/// Go's `localise`: the computed line that says which test failed and where, and the lines of the function under test.
pub fn localise(out: &str, pkg_dir: &str) -> (String, String) {
    if out.contains("Traceback (most recent call last)") || PY_TEST_RE.is_match(out) {
        return localise_python(out, pkg_dir);
    }
    localise_go(out, pkg_dir)
}

fn or_word(s: &str, alt: &str) -> String {
    if s.is_empty() {
        alt.to_owned()
    } else {
        s.to_owned()
    }
}

fn last_dot(s: &str) -> &str {
    match s.rfind('.') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

fn lines_of(p: &str, start: i64, end: i64) -> String {
    read_lines(p.as_bytes(), start, end).unwrap_or_default()
}

fn localise_go(out: &str, pkg_dir: &str) -> (String, String) {
    let mut test = GO_FAIL_RE.captures(out).map(|c| c[1].to_owned()).unwrap_or_default();
    if let Some(pm) = GO_PANIC_RE.captures(out) {
        let what = pm.get(1).map_or("", |x| x.as_str());
        for f in GO_FRAME_RE.captures_iter(out) {
            let (func, file, num) = (&f[1], &f[2], &f[3]);
            if file.contains("/src/runtime/") || file.contains("/src/testing/") || file.ends_with("_test.go") {
                if test.is_empty() && file.ends_with("_test.go") {
                    test = last_dot(func).to_owned();
                }
                continue;
            }
            let p = project_file(file, pkg_dir);
            let line = atoi(num);
            let s = format!("{} panicked: {what}, in {} at {}:{line}.", or_word(&test, "A test"), last_dot(func), base_of(file));
            return (s, lines_of(&p, (line - 3).max(1), line.wrapping_add(3)));
        }
        return (format!("{} panicked: {what}.", or_word(&test, "A test")), String::new());
    }
    let all: Vec<regex::Captures> = GO_ASSERT_RE.captures_iter(out).collect();
    let mut m = None;
    for c in &all {
        let n = atoi(&c[2]);
        if read_lines(join(pkg_dir, &c[1]).as_bytes(), n, n).is_ok_and(|src| GO_FAIL_CALL_RE.is_match(&src)) {
            m = Some(c);
            break;
        }
    }
    if m.is_none() {
        m = all.last();
    }
    let Some(m) = m else {
        if !test.is_empty() {
            return (format!("{test} failed."), String::new());
        }
        return (String::new(), String::new());
    };
    let (file, line, msg) = (m[1].to_owned(), atoi(&m[2]), trim_space(&m[3]).to_owned());
    let s = format!("{} in {file}:{line} failed: {}", or_word(&test, "A test"), clip_text(&msg, 200));
    let mut names = calls_before(&join(pkg_dir, &file), line, "func ");
    names.extend(call_names(&msg));
    if let Some((name, at, text)) = find_def(pkg_dir, &names, ".go") {
        return (format!("{s}; {name} is at {at}."), text);
    }
    (s, String::new())
}

fn localise_python(out: &str, pkg_dir: &str) -> (String, String) {
    let mut block = out;
    const TB: &str = "Traceback (most recent call last)";
    if let Some(i) = out.find(TB) {
        block = &out[i..];
        if let Some(j) = block[1..].find(TB) {
            block = &block[..j + 1];
        }
    }
    let test = PY_TEST_RE.captures(out).map(|m| format!("{} ({})", &m[1], &m[2])).unwrap_or_default();
    let err_line = PY_ERROR_RE.find_iter(block).last().map(|m| trim_space(m.as_str()).to_owned()).unwrap_or_default();
    let (mut file, mut func, mut src, mut line) = (String::new(), String::new(), String::new(), 0i64);
    for f in PY_FRAME_RE.captures_iter(block) {
        let p = &f[1];
        if p.contains("/lib/python") || p.starts_with('<') || p.contains("site-packages") {
            continue;
        }
        file = p.to_owned();
        func = f.get(3).map_or("", |x| x.as_str()).to_owned();
        src = trim_space(f.get(4).map_or("", |x| x.as_str())).to_owned();
        line = atoi(&f[2]);
    }
    if file.is_empty() {
        if !test.is_empty() || !err_line.is_empty() {
            return (trim_space(&format!("{} failed: {err_line}", or_word(&test, "A test"))).to_owned(), String::new());
        }
        return (String::new(), String::new());
    }
    let base = base_of(&file);
    let mut s = format!("{} failed at {base}:{line}", or_word(&test, "A test"));
    if !func.is_empty() && func != "<module>" {
        s.push_str(&format!(" (in {func})"));
    }
    if !err_line.is_empty() {
        s.push_str(&format!(": {}", clip_text(&err_line, 200)));
    }
    if !is_py_test_file(&base) {
        let text = lines_of(&project_file(&file, pkg_dir), (line - 3).max(1), line.wrapping_add(3));
        return (format!("{s}."), text);
    }
    let mut names = call_names(&src);
    names.extend(calls_before(&project_file(&file, pkg_dir), line, "def "));
    if let Some((name, at, text)) = find_def(pkg_dir, &names, ".py") {
        return (format!("{s}; {name} is at {at}."), text);
    }
    (format!("{s}."), String::new())
}

/// Go's `callNames`: the names called in a line of code, nearest first.
fn call_names(src: &str) -> Vec<String> {
    CALL_NAME_RE.captures_iter(src).map(|m| m[1].to_owned()).collect()
}

/// Go's `callsBefore`: the names called on the failing line and the lines above it, up to the start of the test.
fn calls_before(path: &str, line: i64, stop: &str) -> Vec<String> {
    let Ok(text) = read_lines(path.as_bytes(), (line - 15).max(1), line) else { return Vec::new() };
    let mut out = Vec::new();
    for l in text.split('\n').rev() {
        let code = match l.find(": ") {
            Some(k) => &l[k + 2..],
            None => l,
        };
        out.extend(call_names(code));
        if trim_space(code).starts_with(stop) {
            break;
        }
    }
    out
}

/// Go's `findDef`: the first of names the project's non-test files with the extension define, "file:line", and the
/// definition's first lines.
fn find_def(dir: &str, names: &[String], ext: &str) -> Option<(String, String, String)> {
    let entries = gopath::read_dir(dir.as_bytes()).ok()?;
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if !seen.insert(name.clone()) || name == "func" || name == "def" {
            continue;
        }
        let pat = if ext == ".go" {
            format!(r"^func (?:\([^)]*\) )?{}\s*[\[(]", quote_meta(name))
        } else {
            format!(r"^\s*def {}\s*\(", quote_meta(name))
        };
        let Ok(re) = goregexp::compile(&pat) else { continue };
        for e in &entries {
            let fname = lossy(&e.name);
            if e.kind == gopath::Kind::Dir || ext_of(&fname) != ext || fname.ends_with("_test.go") || is_py_test_file(&fname) {
                continue;
            }
            let p = join(dir, &fname);
            // a planted file of gigabytes is skipped, not read
            let Ok(data) = read_bounded(p.as_bytes()) else { continue };
            for (i, l) in lossy(&data).split('\n').enumerate() {
                if re.is_match(l) {
                    let text = lines_of(&p, i as i64 + 1, i as i64 + 12);
                    return Some((name.clone(), format!("{fname}:{}", i + 1), def_body(&text)));
                }
            }
        }
    }
    None
}

/// Go's `defBody`: numbered lines up to the definition's end.
fn def_body(text: &str) -> String {
    let ls: Vec<&str> = text.split('\n').collect();
    for i in 1..ls.len() {
        let code = match ls[i].find(": ") {
            Some(k) => &ls[i][k + 2..],
            None => ls[i],
        };
        if code == "}" {
            return ls[..=i].join("\n");
        }
        if code.starts_with("def ") || code.starts_with("func ") || code.starts_with("class ") {
            return ls[..i].join("\n");
        }
    }
    text.to_owned()
}

/// Go's `projectFile`: a path the check printed, mapped to the project (the sandbox copy is gone by now).
fn project_file(p: &str, pkg_dir: &str) -> String {
    if !is_abs(p) {
        return join(pkg_dir, p);
    }
    if std::fs::metadata(p).is_ok() {
        return p.to_owned();
    }
    join(pkg_dir, &base_of(p))
}

fn is_py_test_file(name: &str) -> bool {
    name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
}

// ---- check_code --------------------------------------------------------------------------------------------------------------

/// Go's `codeFileIn`: the file a folder is checked through.
fn code_file_in(dir: &str) -> String {
    let Ok(entries) = gopath::read_dir(dir.as_bytes()) else { return String::new() };
    for ext in CODE_EXTS {
        let mut test = String::new();
        for e in &entries {
            let n = lossy(&e.name);
            if e.kind != gopath::Kind::Regular || gotext::to_lower(ext_of(&n)) != *ext {
                continue;
            }
            if n.starts_with("test_") || n.ends_with("_test.go") || n.ends_with("_test.py") {
                if test.is_empty() {
                    test = n;
                }
                continue;
            }
            return join(dir, &n);
        }
        if !test.is_empty() {
            return join(dir, &test);
        }
        if *ext == ".go" && has_go_below(dir) && std::fs::metadata(join(dir, "go.mod")).is_ok() {
            return join(dir, "doc.go");
        }
    }
    String::new()
}

/// Go's `hasGoBelow`.
fn has_go_below(dir: &str) -> bool {
    let mut found = false;
    let _ = gopath::walk_dir(dir.as_bytes(), &mut |p, d, err| {
        let Some(d) = d else { return gopath::Walk::Continue };
        if err.is_some() {
            return gopath::Walk::Continue;
        }
        let is_dir = d.kind == gopath::Kind::Dir;
        if is_dir && p != dir.as_bytes() && (d.name.starts_with(b".") || d.name == b"node_modules" || d.name == b"vendor") {
            return gopath::Walk::SkipDir;
        }
        if !is_dir && d.name.ends_with(b".go") {
            found = true;
            return gopath::Walk::SkipAll;
        }
        gopath::Walk::Continue
    });
    found
}

/// Go's `check_code` handler.
pub fn check_code(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let given = arg_str(args, "path");
    let path = match resolve_in_root(&tc.root, &given) {
        Ok(p) => lossy(&p),
        Err(e) => {
            if let Ok(p) = resolve_for_write(&tc.root, &given) {
                if std::fs::symlink_metadata(gopath::os_path(&p)).is_err_and(|e| e.raw_os_error() == Some(2)) {
                    return fail(format!(
                        "{given} does not exist. check_code only checks files that exist and writes nothing: write it with write_file first"
                    ));
                }
            }
            return fail(e);
        }
    };
    if let Err(e) = secret_guard(&given, path.as_bytes(), "check").and_then(|_| private_guard(&given, path.as_bytes(), "check")) {
        return fail(e);
    }
    let info = match std::fs::metadata(&path) {
        Ok(i) => i,
        Err(e) => return fail(format!("cannot access {given}: {}", crate::goerr::path("stat", path.as_bytes(), &e))),
    };
    let mut file = path.clone();
    if info.is_dir() {
        file = code_file_in(&path);
        if file.is_empty() {
            return fail(format!(
                "{given} has no Go, Python, Rust, shell or JavaScript file in it. check_code only checks files that exist and writes \
                 nothing: write the code with write_file first, or give the package folder"
            ));
        }
    }
    let sandbox = have_bwrap() && getenv("SARINA_SANDBOX_TESTS") != "off";
    let (note, mut md) =
        verify_run(&file, tc, sandbox, false, sandbox, "there is no sandbox (bubblewrap) here, and check_code runs tests only in one");
    if note.is_empty() {
        return fail(format!("check_code checks Go, Python, Rust, shell and JavaScript files; {given} is none of them"));
    }
    let rel = lossy(&rel_path(&tc.root, path.as_bytes()));
    md.insert("action".into(), s("check_code"));
    md.insert("path".into(), s(&rel));
    let note = note.replacen(
        "\nFix this before you answer; do not say the code works.",
        "\nThis is how it fails now; do not say the code works.",
        1,
    );
    Output { content: format!("check_code {rel}: {}", note.strip_prefix('\n').unwrap_or(&note)), metadata: md, error: None }
}

/// check_code with the Go build's definition.
pub fn check_code_tool() -> crate::tools::registry::Tool {
    crate::tools::defs::tool("check_code", std::sync::Arc::new(check_code))
}
