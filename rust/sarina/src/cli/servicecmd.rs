//! Go: `service/command.go`: `sarina service` (and the `sarina-service` name): the flags with the environment's
//! defaults (the token and API key read after parsing, never shown in the usage), the refusal to listen beyond loopback
//! without a token, the banner, then the service until SIGINT or SIGTERM (Go's `Shutdown`: 5 s for the busy
//! connections).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::sync::Arc;

use super::tiercmd::{escalation_options, register_escalation};
use super::{Env, Getenv, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::config::model_from_env;
use crate::goflag::{FlagSet, HELP};
use crate::gotext;
use crate::service::{is_loopback_host, parse_ip, serve, Config, Server};

/// Go's `triBool.resolve`: the flag when given, else the environment's value when it reads as a bool, else `def`.
fn resolve(fs: &FlagSet, name: &str, env: &str, def: bool) -> bool {
    if fs.is_set(name) {
        return fs.str(name) == "true";
    }
    let e = gotext::trim_space(env);
    match crate::goflag::parse_bool(e) {
        Some(v) if !e.is_empty() => v,
        _ => def,
    }
}

/// Go's `net.Listen("tcp", JoinHostPort(host, port))` with its error texts: a port out of range, a name that does not
/// resolve, the kernel's refusal to bind.
pub(crate) fn listen(host: &str, port: i64) -> Result<TcpListener, String> {
    let ps = port.to_string();
    if !(0..=65535).contains(&port) {
        return Err(format!("listen tcp: address {ps}: invalid port"));
    }
    let ip: IpAddr = if host.is_empty() {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else if let Some(ip) = parse_ip(host) {
        ip
    } else if gotext::equal_fold(host, "localhost") {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        use std::net::ToSocketAddrs;
        let addrs: Vec<SocketAddr> = (host, 0u16).to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
        match addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()) {
            Some(a) => a.ip(),
            None => return Err(format!("listen tcp: lookup {host}: no such host")),
        }
    };
    let addr = SocketAddr::new(ip, port as u16);
    TcpListener::bind(addr).map_err(|e| format!("listen tcp {addr}: bind: {}", crate::goerr::text(&e)))
}

/// SIGINT and SIGTERM blocked in this thread (and the threads it starts) and taken by one thread of their own, which
/// tells `stop` (Go's `signal.NotifyContext`).
pub(crate) fn signals(stop: std::sync::mpsc::Sender<()>) {
    // SAFETY: a signal set built and used locally; pthread_sigmask on the calling thread only.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
    let _ = std::thread::Builder::new().name("signals".into()).spawn(move || loop {
        let mut sig: libc::c_int = 0;
        // SAFETY: as above; sigwait fills `sig`.
        let r = unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::sigaddset(&mut set, libc::SIGTERM);
            libc::sigwait(&set, &mut sig)
        };
        if r == 0 {
            let _ = stop.send(());
        }
    });
}

/// `sarina service [flags]`.
pub fn service(env: &mut Env, args: &[String]) -> i32 {
    let getenv: Getenv = match &env.getenv {
        Some(g) => g.clone(),
        None => Arc::new(|k: &str| Env::default().getenv(k)),
    };
    let d = model_from_env(&*getenv);
    let mut fs = FlagSet::new("service");
    fs.string("host", "127.0.0.1", "address to bind (must stay loopback unless a token is set)");
    fs.int("port", 8899, "port to listen on");
    fs.string("token", "", "bearer token required on every endpoint but /healthz (env SARINA_SERVICE_TOKEN); empty = no authentication");
    fs.string("model", &d.model, "default model (env OPENAI_MODEL)");
    fs.string("base-url", &d.base_url, "default OpenAI-compatible endpoint (env OPENAI_BASE_URL)");
    fs.string("api-key", "", "default API key (env OPENAI_API_KEY)");
    fs.string("session-dir", ".port_sessions/agent", "where agent sessions persist");
    fs.string("scratchpad-root", ".port_sessions/scratchpad", "scratchpad root");
    fs.int("max-sessions", 20, "pool bound (least recently used sessions are evicted)");
    fs.tribool("permit-shell", "let callers set allow_shell (default: only when a token is set; env SARINA_SERVICE_PERMIT_SHELL)");
    fs.tribool("permit-write", "let callers set allow_write (default: only when a token is set; env SARINA_SERVICE_PERMIT_WRITE)");
    fs.tribool(
        "permit-backend-override",
        "let callers set base_url/api_key (default: only when a token is set; env SARINA_SERVICE_PERMIT_BACKEND)",
    );
    fs.list("cwd-root", "restrict session cwd to this directory (repeatable; env SARINA_SERVICE_CWD_ROOTS, path list); none = anywhere");
    register_escalation(&mut fs);
    let mut err = Vec::new();
    let parsed = fs.parse(args, &mut err);
    env.stderr.extend_from_slice(&err);
    env.flush();
    if let Err(e) = parsed {
        return if e == HELP { EXIT_OK } else { EXIT_USAGE };
    }
    let token = if fs.is_set("token") { fs.str("token") } else { getenv("SARINA_SERVICE_TOKEN") };
    let api_key = if fs.is_set("api-key") { fs.str("api-key") } else { d.api_key.clone() };
    let mut roots = fs.values("cwd-root");
    for r in getenv("SARINA_SERVICE_CWD_ROOTS").split(':') {
        if !r.is_empty() {
            roots.push(r.to_owned());
        }
    }
    let host = fs.str("host");
    if !is_loopback_host(&host) && token.is_empty() {
        env.eprintln(&format!("sarina service: refusing to listen on {host} without a token. Set --token (or SARINA_SERVICE_TOKEN)."));
        return EXIT_USAGE;
    }
    let trusted = !token.is_empty();
    let abs_dir = |p: &str| crate::gopath::abs(p.as_bytes()).map(|a| gotext::lossy(&a)).unwrap_or_else(|_| p.to_owned());
    let cfg = Config {
        token: token.clone(),
        model: fs.str("model"),
        base_url: fs.str("base-url"),
        api_key,
        session_dir: abs_dir(&fs.str("session-dir")),
        scratchpad_root: abs_dir(&fs.str("scratchpad-root")),
        max_sessions: fs.num("max-sessions"),
        permit_shell: resolve(&fs, "permit-shell", &getenv("SARINA_SERVICE_PERMIT_SHELL"), trusted),
        permit_write: resolve(&fs, "permit-write", &getenv("SARINA_SERVICE_PERMIT_WRITE"), trusted),
        permit_backend_override: resolve(&fs, "permit-backend-override", &getenv("SARINA_SERVICE_PERMIT_BACKEND"), trusted),
        cwd_roots: roots,
        home: env.home.clone(),
        getenv: getenv.clone(),
        discover: true,
        escalation: escalation_options(&fs),
    };
    let server = Arc::new(Server::new(cfg.clone()));
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    signals(stop_tx);
    let ln = match listen(&host, fs.num("port")) {
        Ok(l) => l,
        Err(e) => {
            env.eprintln(&format!("sarina service: {e}"));
            return EXIT_FAILURE;
        }
    };
    let addr = ln.local_addr().map(|a| a.to_string()).unwrap_or_default();
    banner(env, &cfg, &host, &addr);
    serve(server, ln, stop_rx);
    EXIT_OK
}

/// Go's `banner`.
fn banner(env: &mut Env, cfg: &Config, host: &str, addr: &str) {
    if cfg.token.is_empty() {
        env.eprintln("WARNING: no token is configured. Unauthenticated requests are accepted.");
        env.eprintln(
            "         Set SARINA_SERVICE_TOKEN or --token. Without a token callers cannot be granted shell, write or backend overrides.",
        );
    } else if !is_loopback_host(host) {
        env.eprintln(&format!(
            "WARNING: listening on non-loopback address {host}. There is no TLS; put this behind something that provides it."
        ));
    }
    let auth = if cfg.token.is_empty() { "OFF" } else { "on" };
    let yn = |b: bool| if b { "yes" } else { "no" };
    let roots = if cfg.cwd_roots.is_empty() { "anywhere".to_owned() } else { cfg.cwd_roots.join(", ") };
    let mut b = format!("sarina service listening on http://{addr}  auth={auth}\n");
    b.push_str(&format!("  model={} base_url={}\n  session_dir={} pool={}\n", cfg.model, cfg.base_url, cfg.session_dir, cfg.max_sessions));
    if cfg.escalation.is_some() {
        b.push_str("  claude_cli escalation tier: on (operator flag)\n");
    }
    b.push_str(&format!(
        "  callers may set: shell={} write={} backend={}; cwd roots: {roots}\n",
        yn(cfg.permit_shell),
        yn(cfg.permit_write),
        yn(cfg.permit_backend_override)
    ));
    env.stderr.extend_from_slice(b.as_bytes());
    env.flush();
}
