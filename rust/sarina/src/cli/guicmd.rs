//! Go: `gui/command.go`: `sarina gui` (and the `sarina-gui` name): the flags with the environment's defaults (the API
//! key read after parsing, never shown in the usage), loopback only, the settings the console starts with, the launch
//! link with its token, the browser opened unless --no-browser, then the console until SIGINT or SIGTERM (Go's
//! `Shutdown` with 3 s of grace).

use std::sync::Arc;
use std::time::Duration;

use super::servicecmd::{listen, signals};
use super::tiercmd::{escalation_options, register_escalation};
use super::{Env, Getenv, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::config::{default_runtime, model_from_env, DEFAULT_TIMEOUT_SECONDS};
use crate::goflag::{FlagSet, HELP};
use crate::gotext;
use crate::gui::state::{abs_dir, default_state, APPROVAL_AUTO};
use crate::gui::{serve, Config, Server};
use crate::service::is_loopback_host;

/// Go's `openBrowser`: a convenience only (failure ignored); the child's output goes nowhere and it is not waited for.
fn open_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// `sarina gui [flags]`.
pub fn gui(env: &mut Env, args: &[String]) -> i32 {
    let getenv: Getenv = match &env.getenv {
        Some(g) => g.clone(),
        None => Arc::new(|k: &str| Env::default().getenv(k)),
    };
    let d = model_from_env(&*getenv);
    let mut fs = FlagSet::new("gui");
    fs.string("host", "127.0.0.1", "address to bind (loopback only)");
    fs.int("port", 8765, "port to listen on");
    fs.string("cwd", ".", "workspace directory the agent works in");
    fs.string("model", &d.model, "model name (env OPENAI_MODEL)");
    fs.string("base-url", &d.base_url, "OpenAI-compatible base URL (env OPENAI_BASE_URL)");
    fs.string("api-key", "", "API key (env OPENAI_API_KEY); never returned by the API");
    fs.string("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)");
    fs.bool("allow-shell", false, "start with shell commands permitted");
    fs.bool("allow-write", false, "start with file writes permitted");
    fs.bool("auto-approve", false, "do not ask before each permitted shell/write action");
    fs.float("temperature", 0.0, "sampling temperature");
    fs.float("timeout-seconds", DEFAULT_TIMEOUT_SECONDS, "per model call timeout");
    fs.bool("stream", true, "stream the model's reply to the page as it is produced");
    fs.int("max-turns", 12, "model turns per run");
    fs.string("append-system-prompt", "", "text added after the system prompt");
    fs.bool("no-browser", false, "do not open the default browser on start");
    fs.list("add-dir", "extra working directory (repeatable)");
    register_escalation(&mut fs);
    let mut err = Vec::new();
    let parsed = fs.parse(args, &mut err);
    env.stderr.extend_from_slice(&err);
    env.flush();
    if let Err(e) = parsed {
        return if e == HELP { EXIT_OK } else { EXIT_USAGE };
    }
    let api_key = if fs.is_set("api-key") { fs.str("api-key") } else { d.api_key.clone() };
    let host = fs.str("host");
    if !is_loopback_host(&host) {
        env.eprintln(&format!(
            "sarina gui: refusing to listen on {host}. The console has no accounts and may hold shell access, so it only binds to a loopback address."
        ));
        return EXIT_USAGE;
    }
    let cwd = fs.str("cwd");
    let mut st = default_state(&*getenv, &cwd);
    let abs = match abs_dir(&cwd) {
        Ok(a) => a,
        Err(e) => {
            env.eprintln(&format!("sarina gui: --cwd: {}", e.msg));
            return EXIT_USAGE;
        }
    };
    st.model = fs.str("model");
    st.base_url = fs.str("base-url");
    st.api_key = api_key;
    st.allow_shell = fs.is_true("allow-shell");
    st.allow_write = fs.is_true("allow-write");
    if fs.is_true("auto-approve") {
        st.approval_mode = APPROVAL_AUTO.into();
    }
    st.temperature = fs.f64("temperature");
    st.timeout_seconds = fs.f64("timeout-seconds");
    st.stream = fs.is_true("stream");
    st.max_turns = fs.num("max-turns");
    st.append_system_prompt = fs.str("append-system-prompt");
    let session_dir = fs.str("session-dir");
    if !session_dir.is_empty() {
        st.session_dir = crate::gopath::abs(session_dir.as_bytes()).map(|a| gotext::lossy(&a)).unwrap_or_default();
        st.session_dir_explicit = true;
    }
    for dd in fs.values("add-dir") {
        match abs_dir(&dd) {
            Ok(a) => st.extra_dirs.push(a),
            Err(e) => {
                env.eprintln(&format!("sarina gui: --add-dir: {}", e.msg));
                return EXIT_USAGE;
            }
        }
    }
    st.cwd = abs.clone();
    if !st.session_dir_explicit {
        st.session_dir = default_runtime(&abs).session_directory;
    }
    let banner = format!(
        "  workspace={}\n  model={} base_url={}\n  session_dir={}\n  permissions={} approval={}\n",
        st.cwd,
        st.model,
        st.base_url,
        st.session_dir,
        st.permission_mode(),
        st.approval_mode
    );
    let server = Arc::new(Server::new(Config {
        initial: st,
        token: String::new(),
        home: env.home.clone(),
        getenv,
        discover: true,
        approval_timeout: Duration::ZERO,
        escalation: escalation_options(&fs),
    }));
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    signals(stop_tx);
    let ln = match listen(&host, fs.num("port")) {
        Ok(l) => l,
        Err(e) => {
            env.eprintln(&format!("sarina gui: {e}"));
            return EXIT_FAILURE;
        }
    };
    let addr = ln.local_addr().map(|a| a.to_string()).unwrap_or_default();
    let url = format!("http://{addr}/?t={}", server.token);
    env.eprintln(&format!("sarina gui: {url}"));
    env.stderr.extend_from_slice(banner.as_bytes());
    env.eprintln("  The link carries a token for this launch; keep it to yourself.");
    if !fs.is_true("no-browser") {
        open_browser(&url);
    }
    serve(server, ln, stop_rx);
    EXIT_OK
}
