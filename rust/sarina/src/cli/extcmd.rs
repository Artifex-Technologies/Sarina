//! Go: `cli/extcmd.go`: `sarina mcp list` (the MCP servers defined, nothing started) and `sarina agents list` (the
//! sub-agent profiles), over the loaders already ported (`policy`, `mcp::config`, `agents`). Proved by the CLI
//! differential's ext cases.

use super::{Env, EXIT_OK, EXIT_USAGE};

/// The owner's signed list (Go's `trust.Default()`): why a file may not be used, "" when it may.
fn trust() -> std::sync::Arc<dyn Fn(&str) -> String + Send + Sync> {
    let c = std::sync::Arc::new(crate::trust::Checker::new(
        crate::trust::default_dir(&|k: &str| std::env::var(k).unwrap_or_default()).as_bytes(),
    ));
    std::sync::Arc::new(move |p: &str| c.refuse(p.as_bytes()))
}

/// The flags after "list": the workspace (Go's `filepath.Abs` of `--cwd`, "" when that fails), or the exit status.
fn list_flags(name: &'static str, with_trust: bool, env: &mut Env, args: &[String]) -> Result<(String, bool), i32> {
    let mut fs = crate::goflag::FlagSet::new(name);
    fs.string("cwd", ".", "workspace directory");
    if with_trust {
        fs.bool("trust-workspace-mcp", false, "also read MCP definition files in the workspace");
    }
    let r = fs.parse(args, &mut env.stderr);
    env.flush();
    match r {
        Ok(_) => {}
        Err(e) if e == crate::goflag::HELP => return Err(EXIT_OK),
        Err(_) => return Err(EXIT_USAGE),
    }
    let ws = crate::gopath::abs(fs.str("cwd").as_bytes()).map(|b| crate::gotext::lossy(&b)).unwrap_or_default();
    Ok((ws, with_trust && fs.is_true("trust-workspace-mcp")))
}

fn policy(env: &Env, ws: &str, trust: &(dyn Fn(&str) -> String + Send + Sync)) -> crate::policy::Policy {
    crate::policy::load(&crate::policy::Options {
        home: env.home_dir(),
        getenv: &env.getenv_fn(),
        workspace: ws.to_string(),
        trust: Some(trust),
    })
}

/// Go's `mcpCommand`: `sarina mcp list [--cwd DIR] [--trust-workspace-mcp]`.
pub fn mcp(env: &mut Env, args: &[String]) -> i32 {
    if args.first().is_none_or(|a| a != "list") {
        env.eprintln("usage: sarina mcp list [--cwd DIR] [--trust-workspace-mcp]");
        return EXIT_USAGE;
    }
    let (ws, trust_workspace) = match list_flags("mcp list", true, env, &args[1..]) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let t = trust();
    if !policy(env, &ws, &*t).trusted {
        env.println("MCP servers are disabled: the operator policy sets trusted=false");
        return EXIT_OK;
    }
    // the command's environment (None: this process's, which the loader reads itself)
    let getenv: Option<crate::mcp::config::Getenv> = env.getenv.clone().map(|g| -> crate::mcp::config::Getenv { g });
    let cfg = crate::mcp::config::load(&crate::mcp::config::LoadOptions {
        home: env.home_dir(),
        getenv,
        workspace: ws,
        trust_workspace,
        trust: Some(t),
    });
    for n in &cfg.notices {
        env.println(&format!("NOTICE: {n}"));
    }
    if cfg.servers.is_empty() {
        env.println("No MCP servers configured. Define them in ~/.sarina/mcp.json or a file named by SARINA_MCP_FILE.");
    }
    for s in &cfg.servers {
        let ro = if s.read_only { "   read-only" } else { "" };
        env.println(&format!("{}\n    {} {}\n    source: {} ({}){ro}", s.name, s.command, s.args.join(" "), s.source, s.file));
    }
    for p in &cfg.problems {
        env.println(&format!("PROBLEM: {p}"));
    }
    EXIT_OK
}

/// Go's `agentsCommand`: `sarina agents list [--cwd DIR]`.
pub fn agents(env: &mut Env, args: &[String]) -> i32 {
    if args.first().is_none_or(|a| a != "list") {
        env.eprintln("usage: sarina agents list [--cwd DIR]");
        return EXIT_USAGE;
    }
    let (ws, _) = match list_flags("agents list", false, env, &args[1..]) {
        Ok(x) => x,
        Err(code) => return code,
    };
    let t = trust();
    let pol = policy(env, &ws, &*t);
    let cat = crate::agents::load(&crate::agents::Options {
        home: env.home_dir(),
        getenv: &env.getenv_fn(),
        trusted: pol.trusted,
        trust: Some(&*t),
    });
    for p in &cat.profiles {
        let tools = match &p.tools {
            Some(t) => t.join(", "),
            None => "everything a read-only helper may use".into(),
        };
        env.println(&format!("{}\n    {}\n    source: {}   tools: {tools}   shell: {}", p.name, p.description, p.source, p.shell));
    }
    for p in &cat.problems {
        env.println(&format!("NOT LOADED: {p}"));
    }
    EXIT_OK
}
