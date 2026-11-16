//! Go: `cryptotools/cryptotools.go`: `sarina crypto`, the five tools the model gets with SARINA_CRYPTO=on (with the
//! workspace's guards on the files they read) and the skills' "crypto" prefetch.

use super::{classic, decode, english, hash, identify, modern};
use crate::goany::Any;
use crate::goerr;
use crate::gopath;
use crate::gotext::{self, lossy, quote};
use crate::tools::defs::tool;
use crate::tools::files::{arg_bool, arg_str, fail};
use crate::tools::fsutil::resolve_in_root;
use crate::tools::private::is_private;
use crate::tools::registry::{Context, Output, Tool};
use crate::tools::secret::is_secret_path;
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, LazyLock};

/// `sarina crypto identify TEXT | decode [ENC] TEXT | classic CIPHER [KEY|crack] TEXT | hash [ALGO] TEXT | modern TEXT`.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    if args.len() < 2 {
        env.eprintln(
            "usage: sarina crypto identify TEXT | decode [ENC] TEXT | classic CIPHER [KEY|crack] TEXT | hash [ALGO] TEXT | modern TEXT",
        );
        return 2;
    }
    let from = |i: usize| args[i..].join(" ");
    let res: Result<Vec<u8>, String> = match args[0].as_str() {
        "identify" => Ok(identify::identify_report(from(1).as_bytes(), "string")),
        "decode" => {
            if args.len() > 2 {
                decode::decode_one(&args[1], from(2).as_bytes())
            } else {
                Ok(decode::describe(decode::decode_auto(args[1].as_bytes(), 3).as_ref()))
            }
        }
        "classic" => if args.len() < 3 {
            Err("classic CIPHER [KEY|crack] TEXT".to_string())
        } else if args[2] == "crack" {
            classic::classic(&from(3), &args[1], "", false, true)
        } else if args.len() > 3 {
            classic::classic(&from(3), &args[1], &args[2], true, false)
        } else {
            classic::classic(&args[2], &args[1], "", false, false)
        }
        .map(String::into_bytes),
        "hash" => Ok(if args.len() > 2 {
            hash::hash_report(from(2).as_bytes(), "text", &args[1], "", "")
        } else {
            hash::hash_report(args[1].as_bytes(), "text", "sha256", "", "")
        }
        .into_bytes()),
        "modern" => modern::modern(&from(1), "", now()),
        other => Err(format!("unknown subcommand {}", quote(other))),
    };
    match res {
        Ok(mut out) => {
            out.push(b'\n');
            env.stdout.extend_from_slice(&out);
            env.flush();
            0
        }
        Err(e) => {
            env.eprintln(&format!("sarina crypto: {e}"));
            2
        }
    }
}

/// Go's `time.Now()` as `modern` takes it: (Unix seconds, nanoseconds).
pub fn now() -> (i64, u32) {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() as i64, d.subsec_nanos())
}

// ---- the tools (SARINA_CRYPTO=on) ------------------------------------------------------------------------------------

/// Go's `MaxInput`: the most of a file the tools read.
pub const MAX_INPUT: u64 = 64 << 20;

/// Go's `evidenceNote`, after every answer.
const EVIDENCE_NOTE: &str = "\n[Computed, not guessed. Say what this shows, and only this. A hash cannot be reversed; encrypted data without its key cannot be read; a signature is verified only where this says VERIFIED.]";

/// The workspace a tool reads in: the context's, else the process's folder (Go's `os.Getwd`, its error ignored).
pub(crate) fn root_of(tc: &Context) -> String {
    if tc.root.is_empty() {
        gopath::getwd().map(|b| lossy(&b)).unwrap_or_default()
    } else {
        tc.root.clone()
    }
}

/// Go's `looksLikeDataNotPath`: a "path" of 16 characters or more with no slash that names nothing in the workspace is
/// the data itself.
fn looks_like_data_not_path(root: &str, path: &str) -> bool {
    if path.len() < 16 || path.contains(['/', '\\']) {
        return false;
    }
    matches!(std::fs::symlink_metadata(gopath::os_path(&gopath::join(&[root.as_bytes(), path.as_bytes()]))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// Go's `input`: the text, or the file at path in the workspace (refused when it looks like a key or credentials file,
/// is in a private folder, is not a regular file or is over [`MAX_INPUT`]); (the bytes, what they are).
fn input(args: &BTreeMap<String, Any>, tc: &Context) -> Result<(Vec<u8>, String), String> {
    let (text, path) = (arg_str(args, "text"), arg_str(args, "path"));
    if path.is_empty() && text.is_empty() {
        return Err("give text or a path".into());
    }
    if path.is_empty() {
        return Ok((text.into_bytes(), "text".into()));
    }
    let root = root_of(tc);
    if looks_like_data_not_path(&root, &path) && text.is_empty() {
        return Ok((path.into_bytes(), "text".into()));
    }
    let resolved = resolve_in_root(&root, &path)?;
    if is_secret_path(path.as_bytes()) || is_secret_path(&resolved) {
        return Err(format!("refusing to read {path}: it looks like a credentials or key file; those never enter the conversation"));
    }
    if is_private(&resolved) {
        return Err(format!("refusing to read {path}: the owner keeps that folder private from me"));
    }
    let st =
        std::fs::metadata(gopath::os_path(&resolved)).map_err(|e| format!("cannot read {path}: {}", goerr::path("stat", &resolved, &e)))?;
    if !st.is_file() {
        return Err(format!("{path} is not a regular file"));
    }
    if st.len() > MAX_INPUT {
        return Err(format!("{path} is {} bytes; I read at most {MAX_INPUT}", st.len()));
    }
    let b = std::fs::read(gopath::os_path(&resolved)).map_err(|e| format!("cannot read {path}: {}", goerr::path("open", &resolved, &e)))?;
    Ok((b, format!("file {path}")))
}

/// Go's `inputAllowingCerts`: as [`input`], but a certificate under a key-like name (.pem, .key, ...) is read when it is
/// a PEM file of at most 1 MiB that holds no private key.
fn input_allowing_certs(args: &BTreeMap<String, Any>, tc: &Context) -> Result<(Vec<u8>, String), String> {
    let path = arg_str(args, "path");
    if path.is_empty() || !is_secret_path(path.as_bytes()) {
        return input(args, tc);
    }
    let resolved = resolve_in_root(&root_of(tc), &path)?;
    if is_private(&resolved) {
        return Err(format!("refusing to read {path}: the owner keeps that folder private from me"));
    }
    match std::fs::metadata(gopath::os_path(&resolved)) {
        Ok(st) if st.is_file() && st.len() <= 1 << 20 => {}
        _ => return Err(format!("cannot read {path} as a certificate file")),
    }
    let b = std::fs::read(gopath::os_path(&resolved)).map_err(|e| goerr::path("open", &resolved, &e))?;
    let has = |pat: &[u8]| b.windows(pat.len()).any(|w| w == pat);
    if !has(b"-----BEGIN") || has(b"PRIVATE") {
        return Err(format!("refusing to read {path}: it looks like a key or credentials file, not a certificate"));
    }
    Ok((b, format!("file {path}")))
}

fn answer(text: &[u8], md: BTreeMap<String, Any>) -> Output {
    Output { content: lossy(text) + EVIDENCE_NOTE, metadata: md, error: None }
}

fn action(name: &str) -> BTreeMap<String, Any> {
    BTreeMap::from([("action".to_string(), Any::String(name.into()))])
}

/// Go's `Tools`: crypto_identify, crypto_decode, crypto_classic, crypto_hash and crypto_modern when SARINA_CRYPTO=on
/// (read when the registry is built).
pub fn tools() -> Vec<Tool> {
    if std::env::var("SARINA_CRYPTO").unwrap_or_default() != "on" {
        return Vec::new();
    }
    vec![
        tool(
            "crypto_identify",
            Arc::new(|a, tc| match input(a, tc) {
                Ok((b, src)) => answer(&identify::identify_report(&b, &src), action("crypto_identify")),
                Err(e) => fail(e),
            }),
        ),
        tool(
            "crypto_decode",
            Arc::new(|a, _| {
                let (from, text) = (arg_str(a, "from"), arg_str(a, "text"));
                if from.is_empty() || from == "auto" {
                    return answer(&decode::describe(decode::decode_auto(text.as_bytes(), 3).as_ref()), action("crypto_decode"));
                }
                match decode::decode_one(&from, text.as_bytes()) {
                    Ok(out) => {
                        // decode_one knows only these names
                        let encoding = decode::ENCODINGS.iter().find(|e| **e == from).copied().unwrap_or_default();
                        let is_text = decode::looks_like_text(&out) || english::printable(&lossy(&out)) > 0.95;
                        let c = decode::Chain { steps: vec![decode::Step { encoding, out }], text: is_text };
                        answer(&decode::describe(Some(&c)), action("crypto_decode"))
                    }
                    Err(e) => fail(format!("{from}: {e}")),
                }
            }),
        ),
        tool(
            "crypto_classic",
            Arc::new(|a, _| {
                let cipher = arg_str(a, "cipher");
                match classic::classic(
                    &arg_str(a, "text"),
                    &cipher,
                    &arg_str(a, "key"),
                    arg_bool(a, "decode", false),
                    arg_bool(a, "crack", false),
                ) {
                    Ok(out) => {
                        let mut md = action("crypto_classic");
                        md.insert("cipher".into(), Any::String(cipher));
                        answer(out.as_bytes(), md)
                    }
                    Err(e) => fail(e),
                }
            }),
        ),
        tool(
            "crypto_hash",
            Arc::new(|a, tc| {
                let report =
                    |b: &[u8], src: &str| hash::hash_report(b, src, &arg_str(a, "algo"), &arg_str(a, "key"), &arg_str(a, "expected"));
                if matches!(a.get("text"), Some(Any::String(t)) if t.is_empty()) && arg_str(a, "path").is_empty() {
                    return answer(report(&[], "text (empty)").as_bytes(), action("crypto_hash"));
                }
                match input(a, tc) {
                    Ok((b, src)) => answer(report(&b, &src).as_bytes(), action("crypto_hash")),
                    Err(e) => fail(e),
                }
            }),
        ),
        tool(
            "crypto_modern",
            Arc::new(|a, tc| match input_allowing_certs(a, tc).and_then(|(b, _)| modern::modern(&lossy(&b), &arg_str(a, "key"), now())) {
                Ok(out) => answer(&out, action("crypto_modern")),
                Err(e) => fail(e),
            }),
        ),
    ]
}

// ---- the "crypto" prefetch -------------------------------------------------------------------------------------------

fn re(go: &str) -> regex::Regex {
    crate::goregexp::compile(go).expect("a fixed pattern")
}

static PEM_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"-----BEGIN ([A-Z0-9 ]+)-----"));
static TOKEN_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"[A-Za-z0-9+/=_.$:-]{16,}"));
static QUOTED_RE: LazyLock<regex::Regex> = LazyLock::new(|| re("[\"“]([^\"”\\n]{20,600})[\"”]"));
static AFTER_COLON_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(":\\s*([^\"“\\n]{20,600})"));
static SENTENCE_SPLIT_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"[.!?]\s+"));
static WORDS_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"^[A-Za-z]+$"));

/// Go's `lookedAt`: the string itself, or for a long one what it is, its length and its start.
fn looked_at(s: &str, what: &str) -> String {
    let n = s.chars().count();
    if n <= 80 {
        return if what == "the quoted text" { format!("the quoted text {s}") } else { s.to_string() };
    }
    let start: String = s.chars().take(12).collect();
    format!("{what} ({n} characters, starting {}; take it from the question in full, not from here)", quote(&(start + "…")))
}

/// Go's `lettersMostly`: 15 letters or more, and at most one other character (beyond spaces and sentence marks) per ten.
fn letters_mostly(s: &str) -> bool {
    let (mut letters, mut other) = (0, 0);
    for c in s.chars() {
        if gotext::is_letter(c) {
            letters += 1;
        } else if !matches!(c, ' ' | ',' | '.' | '\'' | '!' | '?') {
            other += 1;
        }
    }
    letters >= 15 && other * 10 <= letters
}

/// Go's `Prefetch`: crypto_identify (or crypto_modern for a PEM block) on what a skill's task holds: a PEM block, up to
/// three strings that look like data, quoted text or text after a colon that reads like a cipher.
pub fn prefetch(task: &str) -> Vec<u8> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    if let Some(m) = PEM_RE.find(task) {
        // Go means to cut the block at its END line, but its search for the dashes after "-----END" starts on them and
        // only goes on when it finds them past the start: the block is always the rest of the task
        let block = &task[m.start()..];
        parts.push(match modern::modern(block, "", now()) {
            Ok(out) => out,
            Err(_) => identify::identify_report(block.as_bytes(), "block in the question"),
        });
    }
    for m in TOKEN_RE.find_iter(task) {
        let tok = m.as_str().trim_matches(['.', ',', ':', ';']);
        if parts.len() >= 3 || seen.contains(tok) || tok.len() < 16 {
            continue;
        }
        if WORDS_RE.is_match(tok) && english::word_ratio(tok) > 0.0 {
            continue;
        }
        if tok.matches('/').count() > 1 && !decode::looks("base64", tok.as_bytes()) {
            continue; // a path, not data
        }
        seen.insert(tok);
        let (cs, _) = identify::identify(tok.as_bytes());
        if cs.first().is_none_or(|c| c.score < 50) {
            continue;
        }
        let mut p = format!("Looked at {}:\n", looked_at(tok, "a string in the question")).into_bytes();
        p.extend(identify::identify_report(tok.as_bytes(), "string"));
        parts.push(p);
    }
    let mut candidates: Vec<&str> = QUOTED_RE.captures_iter(task).filter_map(|c| c.get(1)).map(|m| m.as_str()).collect();
    for c in AFTER_COLON_RE.captures_iter(task) {
        if let Some(m) = c.get(1) {
            candidates.extend(SENTENCE_SPLIT_RE.split(m.as_str()));
        }
    }
    for q in candidates {
        let q = gotext::trim_space(q);
        if parts.len() >= 3 || seen.contains(q) || !letters_mostly(q) || english::word_ratio(q) >= 0.2 {
            continue;
        }
        seen.insert(q);
        let (cs, _) = identify::identify(q.as_bytes());
        if cs.first().is_some_and(|c| c.score >= 40) {
            let mut p = format!("Looked at {}:\n", looked_at(q, "the quoted text")).into_bytes();
            p.extend(identify::identify_report(q.as_bytes(), "quoted text"));
            parts.push(p);
        }
    }
    if parts.is_empty() {
        return Vec::new();
    }
    let mut out =
        b"Already computed for you (crypto_identify on what the question contains; call the crypto tools for anything else):\n\n".to_vec();
    out.extend(parts.join(&b"\n"[..]));
    out
}
