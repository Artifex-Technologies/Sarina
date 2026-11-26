//! Go: `chymaera`: the Argus commands (`chymaera-console`, `chymaera-analyze`, `chymaera-device-info`,
//! `chymaera-merge-report`). Everything here is read-only: nothing writes to a device or flashes. Proved by the CLI
//! differential's chymaera cases (rust/parity), against a stand-in console on loopback in the case's own network (never
//! the real one on 44700), a stand-in helper and a stand-in git.

pub mod analysis;
pub mod console;
pub mod firmware;

/// Go's `TokenEnv`.
const TOKEN_ENV: &str = "CHYMAERA_CONSOLE_TOKEN";
/// Go's `SemanticHelperReason`.
const SEMANTIC_HELPER_REASON: &str = "semantic grouping needs the embedding helper";

fn parse(fs: &mut crate::goflag::FlagSet, args: &[String], env: &mut crate::cli::Env) -> Result<(), i32> {
    let r = fs.parse(args, &mut env.stderr);
    env.flush();
    match r {
        Ok(_) => Ok(()),
        Err(e) if e == crate::goflag::HELP => Err(0),
        Err(_) => Err(2),
    }
}

/// Go's `addConnFlags`.
fn conn_flags(fs: &mut crate::goflag::FlagSet) {
    fs.string("host", console::DEFAULT_HOST, "console host (loopback)");
    fs.int("port", console::DEFAULT_PORT, "console port");
    // no default value shown: the usage printed for -h or a mistyped flag would show the token
    fs.string("token", "", "console token (default $CHYMAERA_CONSOLE_TOKEN, else the one Argus App wrote to $XDG_RUNTIME_DIR/argus/token)");
}

/// Go's `connFlags.client`: the token given, else $CHYMAERA_CONSOLE_TOKEN, else Argus App's file.
fn client(fs: &crate::goflag::FlagSet, env: &crate::cli::Env) -> console::Client {
    let mut c = console::Client::new();
    let mut token = fs.str("token");
    if !fs.is_set("token") {
        token = env.getenv(TOKEN_ENV);
        if token.is_empty() {
            token = console::default_token(&env.getenv_fn());
        }
    }
    c.host = fs.str("host");
    c.port = fs.num("port");
    c.token = token;
    c.timeout = console::DEFAULT_TIMEOUT;
    c
}

/// Go's `consoleCommand`: `sarina chymaera-console`.
pub fn console_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("chymaera-console");
    conn_flags(&mut fs);
    fs.int("captures", 10, "number of recent captures to list");
    if let Err(code) = parse(&mut fs, args, env) {
        return code;
    }
    let mut c = client(&fs, env);
    let r = c.connect().and_then(|()| c.status()).and_then(|status| Ok((status, c.capture_list(fs.num("captures"))?)));
    c.close();
    match r {
        Ok((status, list)) => {
            env.println(&console::render_status(&status, &list));
            0
        }
        Err(e) => {
            env.println(&e);
            1
        }
    }
}

/// Go's `analyzeCommand`: `sarina chymaera-analyze`.
pub fn analyze_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("chymaera-analyze");
    conn_flags(&mut fs);
    fs.int("limit", 100, "number of recent captures to analyse");
    fs.string("model-dir", "", "embedding model directory (semantic grouping; needs the ML helper, sarina-helpers)");
    fs.float("similarity-threshold", 0.75, "semantic grouping threshold");
    if let Err(code) = parse(&mut fs, args, env) {
        return code;
    }
    let mut c = client(&fs, env);
    let captures = match c.connect().and_then(|()| c.capture_list(fs.num("limit"))) {
        Ok(x) => x,
        Err(e) => {
            c.close();
            env.println(&e);
            return 1;
        }
    };
    let mut reason = "no embedding model given (pass --model-dir; semantic grouping needs the embedding helper)".to_string();
    let mut groups = None;
    let model_dir = fs.str("model-dir");
    let mut srv = None;
    if !model_dir.is_empty() {
        reason.clear();
        let r = crate::helpers::Server::start(&model_dir, &env.getenv_fn()).and_then(|s| {
            let g = s.group(&captures, fs.f64("similarity-threshold"));
            srv = Some(s);
            g
        });
        match r {
            Ok(g) => groups = g,
            Err(e) => {
                reason = format!("{SEMANTIC_HELPER_REASON} ({}): {e}", crate::helpers::INSTALL_HINT);
                env.eprintln(&format!("semantic grouping unavailable: {reason}"));
            }
        }
    }
    let mut result = analysis::analyze(&captures, &reason);
    result.semantic_groups = groups;
    env.println(&result.as_text());
    // Go's deferred Close calls: the helper's, then the connection's
    if let Some(s) = srv {
        let _ = s.close();
    }
    c.close();
    0
}

/// Go's `deviceInfoCommand`: `sarina chymaera-device-info [--file F] [--expected-commit C]` (standard input without a
/// file); exit 1 when there are warnings.
pub fn device_info_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("chymaera-device-info");
    fs.string("file", "", "file holding captured device_info text (default stdin)");
    fs.string("expected-commit", "", "commit the device is expected to run");
    if let Err(code) = parse(&mut fs, args, env) {
        return code;
    }
    let file = fs.str("file");
    let data = if !file.is_empty() {
        std::fs::read(&file).map_err(|e| {
            let op = if std::fs::metadata(&file).is_ok_and(|m| m.is_dir()) { "read" } else { "open" };
            crate::goerr::path(op, file.as_bytes(), &e)
        })
    } else {
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut b)
            .map(|_| b)
            .map_err(|e| crate::goerr::path("read", b"/dev/stdin", &e))
    };
    let data = match data {
        Ok(d) => d,
        Err(e) => {
            env.println(&e);
            return 1;
        }
    };
    let report = firmware::check_device_info(firmware::parse_device_info(&crate::gotext::lossy(&data)), &fs.str("expected-commit"));
    env.println(&report.as_text());
    if report.ok() {
        0
    } else {
        1
    }
}

/// Go's `mergeReportCommand`: `sarina chymaera-merge-report [--cwd D] [--upstream-ref R]`.
pub fn merge_report_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("chymaera-merge-report");
    fs.string("cwd", ".", "firmware checkout");
    fs.string("upstream-ref", "upstream/dev", "upstream ref to compare against");
    if let Err(code) = parse(&mut fs, args, env) {
        return code;
    }
    match firmware::merge_impact(&fs.str("cwd"), &fs.str("upstream-ref")) {
        Ok(r) => {
            env.println(&r.as_text());
            0
        }
        Err(e) => {
            env.println(&e);
            1
        }
    }
}
