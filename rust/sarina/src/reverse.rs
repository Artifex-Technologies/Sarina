//! Go: `go/reverse`: the reverse-engineering tools with SARINA_REVERSE=on and labyrinth-rea installed. `reverse_engineer`
//! runs REA (with Ghidra) through labyrinth-rea inside its sandbox (no network, no home, files read-only): only static,
//! read-only commands, only on files in the analysis folder (~/Documents/Samples) or the malware scan's quarantine.
//! `stage_binary` copies an installed program there, read-only; `draft_yara_rule` hands Heimdall a rule draft that does
//! nothing until the owner tests and approves it.
//!
//! Byte for byte Go's (testdata/parity/reverse.json; rust/parity/cli-differential.py's reverse cases): the network map
//! with Go's regular expressions on Go's bytes (a byte that is not UTF-8 matches as U+FFFD does in Go), `net.ParseIP` and
//! `SplitHostPort`, `regexp.Split`, the cut at 14,000 characters. The analysis's output is read to 16 MiB and its error
//! output to 64 KiB, a child left holding them is waited for 2 s (Go: the same bounds, `runBounded`).

use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gopath;
use crate::gotext::{self, lossy, quote};
use crate::tools::registry::{Context, Output, Tool};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// The REA commands she may use (all static), and what each answers (Go's `Commands`); sorted, as Go's `names` sorts.
pub static COMMANDS: &[(&str, &str)] = &[
    ("analyze", "overview: format, architecture, entry points, imports, notable strings"),
    ("decompile", "pseudocode of a function (give its name or address in target)"),
    ("function", "details of one function (target)"),
    ("functions", "list function names (from the symbol table; a stripped program has none)"),
    ("inspect-binary-layout", "sections, segments, headers"),
    ("inspect-firmware-regions", "regions of a firmware image"),
    ("inspect-go-binary", "Go build info and package list"),
    ("inspect-pe-resources", "Windows resources (manifest, version, icons)"),
    ("inspect-rust-binary", "Rust crates and metadata"),
    ("inspect-signature", "code signature and certificates"),
    ("inspect-with-objdump", "objdump disassembly of the file (or a target range)"),
    ("network-map", "web addresses, host names and IP addresses inside the program, with where each IP belongs (offline)"),
    ("search", "search strings and symbols (target = the text)"),
    ("search-bytes", "search a byte pattern (target = hex)"),
    ("xrefs", "who calls or references an address or name (target)"),
];

fn names() -> Vec<&'static str> {
    COMMANDS.iter().map(|(n, _)| *n).collect()
}

fn env_bytes(k: &str) -> Vec<u8> {
    std::env::var_os(k).map(std::os::unix::ffi::OsStringExt::into_vec).unwrap_or_default()
}

/// Go's `SamplesDir`: LABYRINTH_REA_SAMPLES, else ~/Documents/Samples (the sandbox sees it as /samples).
pub fn samples_dir() -> Vec<u8> {
    let d = env_bytes("LABYRINTH_REA_SAMPLES");
    if !d.is_empty() {
        return d;
    }
    gopath::join(&[&env_bytes("HOME"), b"Documents", b"Samples"])
}

/// Go's `QuarantineDir`: the malware scan's quarantine (the sandbox sees it as /quarantine).
pub fn quarantine_dir() -> Vec<u8> {
    gopath::join(&[&env_bytes("HOME"), b".local", b"share", b"labyrinth", b"quarantine"])
}

/// Go's `SandboxPath`: a path on the computer as the sandbox sees it, or why it is refused.
pub fn sandbox_path(p: &str) -> Result<Vec<u8>, String> {
    let mut abs = gopath::abs(p.as_bytes()).map_err(|e| crate::goerr::text(&e))?;
    if let Ok(r) = gopath::eval_symlinks(&abs) {
        abs = r;
    }
    for (host, inside) in [(samples_dir(), &b"/samples"[..]), (quarantine_dir(), &b"/quarantine"[..])] {
        let host = gopath::eval_symlinks(&host).unwrap_or(host);
        if let Ok(rel) = gopath::rel(&host, &abs) {
            if rel != b".." && !rel.starts_with(b"../") {
                return Ok(gopath::join(&[inside, &rel]));
            }
        }
    }
    Err(format!(
        "{p} is not in the analysis folder: only files in {} (or the quarantine) can be examined; ask the owner to put a copy there",
        lossy(&samples_dir())
    ))
}

/// Go's `targetRE`: `^[A-Za-z0-9_.:+$@/\- ]{1,200}$`.
fn plain_target(t: &str) -> bool {
    (1..=200).contains(&t.len()) && t.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.:+$@/- ".contains(&c))
}

/// Go's `Args`: labyrinth-rea's arguments for one call, or what is wrong.
pub fn args(command: &str, path: &str, target: &str, max_tokens: i64) -> Result<Vec<Vec<u8>>, String> {
    if !COMMANDS.iter().any(|(n, _)| *n == command) {
        return Err(format!("unknown command {} (offered: {})", quote(command), names().join(", ")));
    }
    let inside = sandbox_path(path)?;
    let v = |xs: &[&str]| xs.iter().map(|x| x.as_bytes().to_vec()).collect::<Vec<_>>();
    if command == "functions" {
        let mut a = v(&["--exec", "nm", "-C", "--defined-only", "-n"]);
        a.push(inside);
        return Ok(a);
    }
    if command == "network-map" {
        let mut a = v(&["--exec", "sh", "-c", r#"strings -a -n 6 "$1"; strings -a -n 6 -e l "$1""#, "strings"]);
        a.push(inside);
        return Ok(a);
    }
    let mut a = vec![command.as_bytes().to_vec(), inside];
    if !target.is_empty() {
        if target.starts_with('-') || !plain_target(target) {
            return Err(format!("target {} is not a plain function name, address or text", quote(target)));
        }
        a.push(target.as_bytes().to_vec());
    }
    let max_tokens = if max_tokens <= 0 || max_tokens > 6000 { 3000 } else { max_tokens };
    a.extend(v(&["--format", "md", "--token-limit", &max_tokens.to_string()]));
    Ok(a)
}

/// The most of an analysis's output that is read, and of its error output (Go's `maxOutput`, `maxErrOutput`).
pub const MAX_OUTPUT: usize = 16 << 20;
pub const MAX_ERR_OUTPUT: usize = 64 << 10;

/// Go's `runBounded`: the first `max` bytes of the program's output and the first 64 KiB of its error output, and Go's
/// `Run` error ("exit status 3", "signal: killed", WaitDelay's, a start error), at `deadline` or when `ctx` ends killed.
fn run_bounded(ctx: &Ctx, deadline: Instant, bin: &str, args: &[Vec<u8>], max: usize) -> (Vec<u8>, Vec<u8>, Option<String>) {
    let (mut out, mut errb) = (Vec::new(), Vec::new());
    // a context already done: Go's Start refuses before starting anything
    if let Some(e) = ctx.err() {
        return (out, errb, Some(e.to_string()));
    }
    let args: Vec<String> = args.iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    let mut sink = |stream: &'static str, b: &[u8]| {
        let (buf, cap) = if stream == "stdout" { (&mut out, max) } else { (&mut errb, MAX_ERR_OUTPUT) };
        let room = cap.saturating_sub(buf.len());
        buf.extend_from_slice(&b[..b.len().min(room)]);
    };
    let env = crate::tools::exec::parent_env();
    let r = crate::tools::exec::run_group(bin, &args, &env, None, deadline, ctx, crate::tools::exec::GroupKill::Plain, &mut sink);
    let err = match r {
        Err(e) => Some(e),
        Ok(end) => match end.status {
            None => Some("signal: killed".to_string()),
            Some(st) => crate::tools::exec::exit_error(st)
                .or_else(|| end.io_incomplete.then(|| "exec: WaitDelay expired before I/O complete".to_string())),
        },
    };
    (out, errb, err)
}

/// Go's `exec.LookPath("labyrinth-rea")` unless LABYRINTH_REA names the program.
fn rea_binary() -> Option<String> {
    let bin = std::env::var("LABYRINTH_REA").unwrap_or_default();
    if !bin.is_empty() {
        return Some(bin);
    }
    crate::tools::exec::look_path_go("labyrinth-rea").ok()
}

/// Go's `Tools`: stage_binary, draft_yara_rule and reverse_engineer when SARINA_REVERSE=on and labyrinth-rea is there
/// (read when the registry is built).
pub fn tools() -> Vec<Tool> {
    if std::env::var("SARINA_REVERSE").unwrap_or_default() != "on" {
        return Vec::new();
    }
    let Some(bin) = rea_binary() else { return Vec::new() };
    vec![stage_tool(), rule_tool(), reverse_tool(bin)]
}

fn sarg(args: &BTreeMap<String, Any>, k: &str) -> String {
    match args.get(k) {
        Some(Any::String(v)) => v.clone(),
        _ => String::new(),
    }
}

fn reverse_tool(bin: String) -> Tool {
    crate::tools::defs::tool(
        "reverse_engineer",
        Arc::new(move |a: &BTreeMap<String, Any>, tc: &Context| {
            let (command, path, target) = (sarg(a, "command"), sarg(a, "path"), sarg(a, "target"));
            let argv = match args(&command, &path, &target, 0) {
                Ok(v) => v,
                Err(e) => return crate::tools::files::fail(e),
            };
            let deadline = Instant::now() + Duration::from_secs(6 * 60);
            let (out, errb, run_err) = run_bounded(&tc.ctx, deadline, &bin, &argv, MAX_OUTPUT);
            let mut text = gotext::trim_space_bytes(&out).to_vec();
            if command == "network-map" && (run_err.is_none() || !text.is_empty()) {
                text = find_addresses(&text).report(&tc.ctx, &gopath::base(path.as_bytes()));
            }
            if let Some(mut cut) = crate::sysinfo::rune_prefix(&text, 14000) {
                cut.extend_from_slice(b"\n[cut]");
                text = cut;
            }
            if let Some(e) = &run_err {
                if text.is_empty() {
                    return crate::tools::files::fail(format!("the analysis did not run: {e}: {}", lossy(gotext::trim_space_bytes(&errb))));
                }
            }
            if command == "analyze" && run_err.is_none() {
                text = condense_overview(&text);
                if let Ok(fargs) = args("functions", &path, "", 0) {
                    let (nm, _, _) = run_bounded(&tc.ctx, deadline, &bin, &fargs, MAX_OUTPUT);
                    text.extend_from_slice(b"\n\n");
                    text.extend_from_slice(own_functions(&nm).as_bytes());
                }
            }
            if command == "decompile" {
                let calls = called_functions(&lossy(&text), &target);
                if !calls.is_empty() {
                    text.extend_from_slice(format!("\n\nThis code calls functions of the same program: {}. If the question depends on what they do (a check, a rule), decompile them too before you answer; do not guess their logic.", calls.join(", ")).as_bytes());
                }
            }
            text.extend_from_slice(b"\n\n[Evidence only: say nothing about the program that is not in the output above. An overview does not show the logic: to explain a check, decompile the function that does it and quote its code. If you have not seen it, say you have not verified it.]");
            let md = BTreeMap::from([
                ("action".to_string(), Any::String("reverse_engineer".into())),
                ("command".to_string(), Any::String(command)),
                ("path".to_string(), Any::String(path)),
            ]);
            Output { content: lossy(&text), metadata: md, error: None }
        }),
    )
}

/// Go's `ruleTool`: a YARA rule draft for a sample Heimdall holds; nothing changes until the owner approves it.
fn rule_tool() -> Tool {
    crate::tools::defs::tool(
        "draft_yara_rule",
        Arc::new(|a: &BTreeMap<String, Any>, _tc: &Context| {
            let s = |k: &str| gotext::trim_space(&sarg(a, k)).to_owned();
            match draft_rule(&gotext::to_lower(&s("sample_sha256")), &s("rule"), &s("note")) {
                Err(e) => crate::tools::files::fail(e),
                Ok((id, next)) => Output {
                    content: format!("Draft {id} is with Heimdall. It is not active: the owner tests it first ({next})."),
                    metadata: BTreeMap::from([
                        ("action".to_string(), Any::String("draft_yara_rule".into())),
                        ("id".to_string(), Any::String(id)),
                    ]),
                    error: None,
                },
            }
        }),
    )
}

/// Go's `heimdall.DraftRule`: `POST /v1/rules/drafts` on Heimdall (the scheme and host of HEIMDALL_REVIEW_URL, else
/// 127.0.0.1:8930); the draft's id and the owner's next step.
pub fn draft_rule(sample_sha256: &str, rule: &str, note: &str) -> Result<(String, String), String> {
    let mut base = "http://127.0.0.1:8930".to_string();
    let getenv = |k: &str| std::env::var(k).unwrap_or_default();
    if let Some(u) = crate::gourl::parse(&crate::heimdall::url(&getenv)) {
        if !u.host.is_empty() {
            base = format!("{}://{}", u.scheme, lossy(&u.host));
        }
    }
    let payload = Any::Object(BTreeMap::from([
        ("note".to_string(), Any::String(note.into())),
        ("rule".to_string(), Any::String(rule.into())),
        ("sample_sha256".to_string(), Any::String(sample_sha256.into())),
    ]));
    // json.Marshal: keys sorted, <, > and & escaped
    let body = crate::gojson::marshal_any(&payload).map_err(|e| e.to_string())?;
    let reply =
        crate::gohttp::post(&format!("{base}/v1/rules/drafts"), "application/json", body.as_bytes(), crate::heimdall::TIMEOUT, 64 * 1024)
            .map_err(|e| format!("Heimdall is not reachable: {e}"))?;
    if reply.status != 200 {
        return Err(format!("Heimdall answered {}: {}", reply.status_text, lossy(gotext::trim_space_bytes(&reply.body))));
    }
    use crate::gostruct::{unmarshal, Field, Ty, Val};
    static DRAFT: Ty = Ty::Struct("struct { ID string; Next string }", "", &[Field("ID", &Ty::Str), Field("Next", &Ty::Str)]);
    let (v, err) = unmarshal(&DRAFT, &reply.body);
    let (id, next) = match v {
        Val::Struct(f) => match (&f[0], &f[1]) {
            (Val::Str(i), Val::Str(n)) => (i.clone(), n.clone()),
            _ => (String::new(), String::new()),
        },
        _ => (String::new(), String::new()),
    };
    if err.is_some() || id.is_empty() {
        return Err("Heimdall's answer could not be read".into());
    }
    Ok((id, next))
}

fn stage_tool() -> Tool {
    crate::tools::defs::tool(
        "stage_binary",
        Arc::new(|a: &BTreeMap<String, Any>, _tc: &Context| {
            let p = sarg(a, "path");
            match stage(&p) {
                Err(e) => crate::tools::files::fail(e),
                Ok(dst) => {
                    let dst = lossy(&dst);
                    Output {
                        content: format!("Copied to {dst} (read-only). Now use reverse_engineer on that path."),
                        metadata: BTreeMap::from([
                            ("action".to_string(), Any::String("stage_binary".into())),
                            ("path".to_string(), Any::String(p)),
                            ("copy".to_string(), Any::String(dst)),
                        ]),
                        error: None,
                    }
                }
            }
        }),
    )
}

/// The most a staged copy may be (Go's `maxStage`).
const MAX_STAGE: u64 = 200 << 20;

fn stage_roots() -> Vec<Vec<u8>> {
    let mut r: Vec<Vec<u8>> =
        ["/usr/bin", "/usr/sbin", "/usr/lib", "/usr/lib64", "/usr/libexec", "/usr/local/bin", "/usr/local/lib", "/opt"]
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();
    let h = env_bytes("HOME");
    if !h.is_empty() {
        r.push(gopath::join(&[&h, b".local", b"bin"]));
        r.push(gopath::join(&[&h, b".local", b"share", b"labyrinth", b"apps"]));
    }
    r
}

/// Go's `Stage`: an installed program copied, read-only, into the analysis folder's "staged" folder as
/// NAME-<first 8 hex of its SHA-256>; the copy's path.
pub fn stage(src: &str) -> Result<Vec<u8>, String> {
    use sha2::Digest;
    use std::io::{Read, Write};
    let abs = gopath::abs(src.as_bytes()).map_err(|e| crate::goerr::text(&e))?;
    let Ok(real) = gopath::eval_symlinks(&abs) else { return Err(format!("{src} does not exist")) };
    let inside = stage_roots().iter().any(|root| {
        let root = gopath::eval_symlinks(root).unwrap_or_else(|_| root.clone());
        gopath::rel(&root, &real).is_ok_and(|rel| rel != b".." && !rel.starts_with(b"../"))
    });
    if !inside {
        return Err(format!("{src} is not an installed program: only files under /usr, /opt and your own program folders can be staged"));
    }
    let md = match std::fs::symlink_metadata(gopath::os_path(&real)) {
        Ok(m) if m.file_type().is_file() => m,
        _ => return Err(format!("{src} is not a plain file")),
    };
    if md.len() > MAX_STAGE {
        return Err(format!("{src} is larger than {} MB", MAX_STAGE >> 20));
    }
    let mut input = std::fs::File::open(gopath::os_path(&real)).map_err(|e| crate::goerr::path("open", &real, &e))?;
    let dir = gopath::join(&[&samples_dir(), b"staged"]);
    crate::goos::mkdir_all_mode(&dir, 0o700)?;
    let (mut tmp, tmp_name) = crate::goos::create_temp(&dir, ".stage-")?;
    struct Remove(Vec<u8>);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(gopath::os_path(&self.0));
        }
    }
    let _remove = Remove(tmp_name.clone());
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 32 << 10];
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(crate::goerr::path("read", &real, &e)),
        };
        tmp.write_all(&buf[..n]).map_err(|e| crate::goerr::path("write", &tmp_name, &e))?;
        h.update(&buf[..n]);
    }
    drop(tmp);
    let sum: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let mut name = gopath::base(&real);
    name.extend_from_slice(format!("-{}", &sum[..8]).as_bytes());
    let dst = gopath::join(&[&dir, &name]);
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(gopath::os_path(&tmp_name), std::fs::Permissions::from_mode(0o400))
            .map_err(|e| crate::goerr::path("chmod", &tmp_name, &e))?;
    }
    std::fs::rename(gopath::os_path(&tmp_name), gopath::os_path(&dst))
        .map_err(|e| format!("rename {} {}: {}", lossy(&tmp_name), lossy(&dst), crate::goerr::text(&e)))?;
    Ok(dst)
}

// ---- the network map --------------------------------------------------------------------------------------------------

static URL_RX: LazyLock<regex::Regex> = LazyLock::new(|| {
    crate::goregexp::compile(r#"(?i)\b(?:https?|wss?|ftp|tcp|udp)://[A-Za-z0-9.\-\[\]:]+(?:/[^\s"'<>\\]{0,200})?"#).expect("urlRx")
});
static IPV4_RX: LazyLock<regex::Regex> = LazyLock::new(|| {
    crate::goregexp::compile(r"\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)(?::\d{1,5})?\b").expect("ipv4Rx")
});
static IPV6_RX: LazyLock<regex::Regex> =
    LazyLock::new(|| crate::goregexp::compile(r"(?:[0-9a-fA-F]{0,4}:){2,7}[0-9a-fA-F]{1,4}").expect("ipv6Rx"));
static ONION_RX: LazyLock<regex::Regex> = LazyLock::new(|| crate::goregexp::compile(r"\b[a-z2-7]{16,56}\.onion\b").expect("onionRx"));
static HOST_RX: LazyLock<regex::Regex> = LazyLock::new(|| {
    crate::goregexp::compile(r"(?i)\b(?:[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?\.)+(?:com|net|org|io|info|biz|gov|edu|mil|ru|cn|de|uk|fr|nl|jp|kr|br|in|us|eu|ca|au|it|es|pl|ua|ir|kp|tr|vn|xyz|top|online|site|club|app|dev|cloud|tk|ml|ga|cf|gq|pw|cc|me|tv|ws|su|link|live|icu|shop|store|tech|space|website|fun)\b").expect("hostRx")
});
static CALL_RX: LazyLock<regex::Regex> = LazyLock::new(|| crate::goregexp::compile(r"\b([A-Za-z_][A-Za-z0-9_]{2,})\s*\(").expect("callRE"));

/// Go's `FindAllString(text, n)` on Go's bytes: the regex runs on the text with each bad byte as U+FFFD (Go's regexp reads
/// it as RuneError) and the matches are cut from the bytes.
fn find_all(re: &regex::Regex, s: &str, at: &[usize], text: &[u8], n: usize) -> Vec<Vec<u8>> {
    re.find_iter(s).take(n).map(|m| text[at[m.start()]..at[m.end()]].to_vec()).collect()
}

/// What a program's strings name (Go's `NetMap`).
#[derive(Debug, Default, PartialEq)]
pub struct NetMap {
    pub urls: Vec<Vec<u8>>,
    pub hosts: Vec<Vec<u8>>,
    pub ips: Vec<Vec<u8>>,
    pub onions: Vec<Vec<u8>>,
}

/// Go's `net.SplitHostPort` (the host and port of "host:port" or "[host]:port"), with Go's `AddrError` texts.
pub fn split_host_port(hp: &str) -> Result<(String, String), String> {
    let err = |why: &str| if hp.is_empty() { why.to_owned() } else { format!("address {hp}: {why}") };
    const MISSING_PORT: &str = "missing port in address";
    const TOO_MANY_COLONS: &str = "too many colons in address";
    let Some(i) = hp.rfind(':') else { return Err(err(MISSING_PORT)) };
    let (host, j, k);
    if hp.starts_with('[') {
        let Some(end) = hp.find(']') else { return Err(err("missing ']' in address")) };
        if end + 1 == hp.len() {
            return Err(err(MISSING_PORT));
        } else if end + 1 != i {
            return Err(err(if hp.as_bytes()[end + 1] == b':' { TOO_MANY_COLONS } else { MISSING_PORT }));
        }
        host = &hp[1..end];
        (j, k) = (1, end + 1);
    } else {
        host = &hp[..i];
        if host.contains(':') {
            return Err(err(TOO_MANY_COLONS));
        }
        (j, k) = (0, 0);
    }
    if hp[j..].contains('[') {
        return Err(err("unexpected '[' in address"));
    }
    if hp[k..].contains(']') {
        return Err(err("unexpected ']' in address"));
    }
    Ok((host.to_owned(), hp[i + 1..].to_owned()))
}

/// The host of an address that may carry a port, as the map reads it.
fn host_of(s: &str) -> String {
    split_host_port(s).map(|(h, _)| h).unwrap_or_else(|_| s.to_owned())
}

/// Go's `net.ParseIP` with what the map asks of it: None, or (unspecified, loopback, an IPv4 address or IPv4-mapped).
pub fn parse_ip(s: &str) -> Option<(bool, bool, bool)> {
    match crate::service::parse_ip(s)? {
        std::net::IpAddr::V4(v4) => Some((v4.is_unspecified(), v4.is_loopback(), true)),
        std::net::IpAddr::V6(v6) => Some((v6.is_unspecified(), v6.is_loopback(), false)),
    }
}

/// Go's `FindAddresses`: web addresses, host names, IP addresses (each the parser accepts, not unspecified or loopback)
/// and Tor addresses in a program's strings, each list without repeats, sorted, at most 60 and a count of the rest.
pub fn find_addresses(text: &[u8]) -> NetMap {
    let (s, at) = gotext::lossy_with_offsets(text);
    let set = |xs: Vec<Vec<u8>>, keep: &dyn Fn(&[u8]) -> bool| -> Vec<Vec<u8>> {
        let mut m = std::collections::BTreeSet::new();
        for x in xs {
            let mut end = x.len();
            while end > 0 && b".,;:)".contains(&x[end - 1]) {
                end -= 1;
            }
            let x = x[..end].to_vec();
            if keep(&x) {
                m.insert(x);
            }
        }
        let mut out: Vec<Vec<u8>> = m.into_iter().collect();
        if out.len() > 60 {
            let more = out.len() - 60;
            out.truncate(60);
            out.push(format!("... and {more} more").into_bytes());
        }
        out
    };
    let all = |_: &[u8]| true;
    let mut ips = find_all(&IPV4_RX, &s, &at, text, 2000);
    ips.extend(find_all(&IPV6_RX, &s, &at, text, 2000));
    NetMap {
        urls: set(find_all(&URL_RX, &s, &at, text, 2000), &all),
        onions: set(find_all(&ONION_RX, &s, &at, text, 500), &all),
        ips: set(ips, &|x| {
            let h = host_of(&lossy(x));
            parse_ip(&h).is_some_and(|(unspec, lo, v4)| !unspec && !lo && (v4 || h.len() >= 6))
        }),
        hosts: set(find_all(&HOST_RX, &s, &at, text, 2000), &|x| {
            let l = gotext::to_lower_bytes(x);
            !l.starts_with("www.w3.org") && !l.ends_with(".gnu.org")
        }),
    }
}

/// Go's `origins`: where each IP belongs, from `heimdalld origin` (offline), when it is installed.
fn origins(ctx: &Ctx, ips: &[Vec<u8>]) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut out = BTreeMap::new();
    let plain: Vec<Vec<u8>> =
        ips.iter().map(|s| host_of(&lossy(s)).into_bytes()).filter(|h| crate::service::parse_ip(&lossy(h)).is_some()).collect();
    let Ok(bin) = crate::tools::exec::look_path_go("heimdalld") else { return out };
    if plain.is_empty() {
        return out;
    }
    let mut args = vec![b"origin".to_vec()];
    args.extend(plain);
    let (b, _, _) = run_bounded(ctx, Instant::now() + Duration::from_secs(10), &bin, &args, 1 << 20);
    for line in b.split(|&c| c == b'\n') {
        if let Some(i) = line.windows(2).position(|w| w == b"  ") {
            out.insert(line[..i].to_vec(), gotext::trim_space_bytes(&line[i + 2..]).to_vec());
        }
    }
    out
}

impl NetMap {
    /// Go's `Report`.
    pub fn report(&self, ctx: &Ctx, file: &[u8]) -> Vec<u8> {
        let mut b =
            format!("Network map of {} (from its strings only: nothing was run and no name was looked up).\n", lossy(file)).into_bytes();
        if self.urls.len() + self.hosts.len() + self.ips.len() + self.onions.len() == 0 {
            b.extend_from_slice(b"No web addresses, host names or IP addresses were found in its strings. It may build them at run time or keep them encrypted; that cannot be seen this way.\n");
            return b;
        }
        let list = |b: &mut Vec<u8>, title: &str, xs: &[Vec<u8>], note: &dyn Fn(&[u8]) -> Vec<u8>| {
            if xs.is_empty() {
                return;
            }
            b.extend_from_slice(format!("\n{title} ({}):\n", xs.len()).as_bytes());
            for x in xs {
                b.extend_from_slice(b"- ");
                b.extend_from_slice(x);
                let n = note(x);
                if !n.is_empty() {
                    b.extend_from_slice(b"  (");
                    b.extend_from_slice(&n);
                    b.extend_from_slice(b")");
                }
                b.push(b'\n');
            }
        };
        let org = origins(ctx, &self.ips);
        let none = |_: &[u8]| Vec::new();
        list(&mut b, "Web addresses", &self.urls, &none);
        list(&mut b, "Host names", &self.hosts, &none);
        list(&mut b, "IP addresses", &self.ips, &|x| org.get(host_of(&lossy(x)).as_bytes()).cloned().unwrap_or_default());
        list(&mut b, "Tor addresses", &self.onions, &|_| b"a hidden service: unusual for ordinary software".to_vec());
        b.extend_from_slice(b"\nAn address in a program's strings is not proof it is contacted (documentation, certificates and libraries carry addresses too); a dotted number may be a version. To know whether one is used, search for it (search) and decompile the function that refers to it (xrefs, then decompile).\n");
        b
    }
}

// ---- the overview, the functions, the calls ---------------------------------------------------------------------------

const LIBC_NAMES: &[&str] = &[
    "printf", "puts", "putchar", "fprintf", "sprintf", "snprintf", "strcmp", "strncmp", "strlen", "strcpy", "strncpy", "strcat", "memcpy",
    "memset", "memcmp", "malloc", "calloc", "realloc", "free", "exit", "abort", "atoi", "strtol", "fopen", "fclose", "fread", "fwrite",
    "fgets", "scanf", "sscanf", "getenv", "return", "if", "while", "for", "switch", "sizeof",
];

/// Go's `calledFunctions`: the program's own functions a piece of decompiled code calls (at most six).
pub fn called_functions(code: &str, this: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in CALL_RX.captures_iter(code) {
        let n = &c[1];
        if LIBC_NAMES.contains(&n) || n == this || out.iter().any(|o| o == n) || n.starts_with("__") {
            continue;
        }
        if n.starts_with('_') || n.to_uppercase() == n {
            continue;
        }
        out.push(n.to_owned());
        if out.len() == 6 {
            break;
        }
    }
    out
}

const OVERVIEW_KEEP: &[&str] = &[
    "subject.name",
    "subject.format",
    "subject.architecture",
    "subject.digest",
    "normalized_result.segment_count",
    "normalized_result.procedure_count",
    "normalized_result.string_count",
    "confidence",
];

/// Go's `sectionRE.Split(md, -1)` for `(?m)^## `.
fn split_sections(md: &[u8]) -> Vec<&[u8]> {
    if md.is_empty() {
        return vec![md];
    }
    let mut matches = Vec::new();
    let mut i = 0;
    while i + 3 <= md.len() {
        if (i == 0 || md[i - 1] == b'\n') && &md[i..i + 3] == b"## " {
            matches.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let (mut out, mut beg, mut end) = (Vec::new(), 0, 0);
    for (s, e) in matches {
        end = s;
        if e != 0 {
            out.push(&md[beg..end]);
        }
        beg = e;
    }
    if end != md.len() {
        out.push(&md[beg..]);
    }
    out
}

/// Go's `condenseOverview`: only the sections of an analyze overview worth the model's context.
pub fn condense_overview(md: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    for sec in split_sections(md) {
        let (head, body) = match sec.iter().position(|&c| c == b'\n') {
            Some(i) => (&sec[..i], &sec[i + 1..]),
            None => (sec, &b""[..]),
        };
        let head = gotext::trim_space_bytes(head);
        if OVERVIEW_KEEP.iter().any(|k| k.as_bytes() == head) {
            b.extend_from_slice(b"## ");
            b.extend_from_slice(head);
            b.extend_from_slice(b"\n\n");
            b.extend_from_slice(gotext::trim_space_bytes(body));
            b.extend_from_slice(b"\n\n");
        }
    }
    if b.is_empty() {
        return md.to_vec();
    }
    gotext::trim_space_bytes(&b).to_vec()
}

/// Go's `ownFunctions`: the program's own code from nm's output, and what to do next.
pub fn own_functions(nm: &[u8]) -> String {
    let mut own: Vec<String> = Vec::new();
    for l in nm.split(|&c| c == b'\n') {
        let f = gotext::fields_bytes(l);
        if f.len() < 3 || (f[1] != b"T" && f[1] != b"t") {
            continue;
        }
        let name = lossy(&f[2..].join(&b' '));
        if name.starts_with('_') || matches!(name.as_str(), "deregister_tm_clones" | "register_tm_clones" | "frame_dummy") {
            continue;
        }
        let addr = f[0].iter().position(|&c| c != b'0').map_or(&b""[..], |i| &f[0][i..]);
        own.push(format!("{name} ({})", lossy(addr)));
    }
    if own.is_empty() {
        return "## functions\n\nNo function names (the program is stripped). Next: decompile by address, starting with the entry point."
            .into();
    }
    if own.len() > 40 {
        let more = own.len() - 40;
        own.truncate(40);
        own.push(format!("and {more} more (command functions lists them all)"));
    }
    format!(
        "## functions (the program's own)\n\n{}\n\nNext: decompile the function that does what you were asked about (target: its name), before you explain it.",
        own.join(", ")
    )
}

/// Go's `cutRunes`: `string([]rune(s)[:n])` when s has more than n characters.
pub fn cut_runes(s: &[u8], n: usize) -> Option<Vec<u8>> {
    crate::sysinfo::rune_prefix(s, n)
}
