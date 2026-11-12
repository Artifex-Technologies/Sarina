//! Go: `cli/tiercmd.go`'s `sarina tiers status`: the tiers from ~/.sarina/tiers.json (or `SARINA_TIERS_FILE`), with the
//! escalation flags of the chat command, each enabled tier probed for health (a models request, never conversation
//! data) and one resource sample against the thresholds.

use super::{Env, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::goctx::Ctx;
use crate::goflag::{FlagSet, HELP};
use crate::tiers::config::{from_escalation, load, EscalationOptions, KIND_CLAUDE_CLI};
use crate::tiers::router::{Options, Router, TierStatus};

/// Go's `runkit.EscalationFlags.Register`.
pub fn register_escalation(fs: &mut FlagSet) {
    fs.bool(
        "escalate-to-claude-code",
        false,
        "add the claude_cli tier: hand a run to the local `claude` CLI when the local backend fails or is under resource pressure",
    );
    fs.string("claude-code-cli-path", "", "path of the claude executable (env CLAUDE_CODE_CLI_PATH; default `claude`)");
    fs.list("escalation-trigger", "backend_error or resource_exhausted: which conditions may hand off (repeatable; default both)");
    fs.bool("disable-resource-check", false, "never move up a tier because of memory, CPU or GPU pressure");
}

/// Go's `EscalationFlags.Options`: None unless escalation was asked for.
pub fn escalation_options(fs: &FlagSet) -> Option<EscalationOptions> {
    if !fs.is_true("escalate-to-claude-code") {
        return None;
    }
    Some(EscalationOptions {
        enabled: true,
        cli_path: fs.str("claude-code-cli-path"),
        triggers: fs.values("escalation-trigger"),
        disable_resource_check: fs.is_true("disable-resource-check"),
        ..EscalationOptions::default()
    })
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

/// Go's `renderTierStatus`.
pub fn render_tier_status(rows: &[TierStatus], path: &str, resource: &str) -> String {
    let mut b = String::new();
    if path.is_empty() {
        b.push_str("Config: none (default: the local backend only)\n");
    } else {
        b.push_str(&format!("Config: {path}\n"));
    }
    for (i, s) in rows.iter().enumerate() {
        let mark = if s.current { "*" } else { " " };
        let state = if !s.enabled {
            "disabled".to_owned()
        } else if !s.healthy {
            format!("unavailable: {}", s.why)
        } else {
            "healthy".to_owned()
        };
        b.push_str(&format!("{mark} {}. {} [{}] {state}\n", i + 1, s.name, s.kind));
        if s.kind == KIND_CLAUDE_CLI {
            let found = if s.cli_found { "found" } else { "not found" };
            b.push_str(&format!("     claude executable {} ({found})\n", s.cli_path));
        }
        if !s.base_url.is_empty() || !s.model.is_empty() {
            b.push_str(&format!("     endpoint {}  model {}\n", or_dash(&s.base_url), or_dash(&s.model)));
        }
        b.push_str(&format!("     allow_data {}", s.allow_data));
        if s.max_context > 0 {
            b.push_str(&format!("  max_context {}", s.max_context));
        }
        if !s.api_key_env.is_empty() {
            let set = if s.key_set { "set" } else { "not set" };
            b.push_str(&format!("  key from ${} ({set})", s.api_key_env));
        }
        if s.wake {
            b.push_str("  wake configured");
        }
        if !s.auto {
            b.push_str("  asks before first use");
        }
        b.push('\n');
    }
    if !resource.is_empty() {
        b.push_str(resource);
        b.push('\n');
    }
    b.trim_end_matches('\n').to_owned()
}

/// Go's `resourceLine`.
pub fn resource_line(r: &Router) -> String {
    let (s, pressure) = r.pressure(&Ctx::background());
    let th = r.thresholds();
    let part = |ok: bool, v: f64, limit: f64| {
        if !ok {
            "unknown".to_owned()
        } else if limit <= 0.0 {
            format!("{v:.0}% (no limit)")
        } else {
            format!("{v:.0}% (limit {limit:.0}%)")
        }
    };
    let mut line = format!(
        "Resources: memory {}, GPU memory {}",
        part(s.mem_ok, s.mem_percent, th.max_memory_percent),
        part(s.gpu_ok, s.gpu_percent, th.max_gpu_memory_percent)
    );
    if !pressure.is_empty() {
        line.push_str("\nPRESSURE: ");
        line.push_str(&pressure);
    }
    line
}

/// `sarina tiers [status] [escalation flags]`.
pub fn tiers(env: &mut Env, args: &[String]) -> i32 {
    let (sub, args) = match args.split_first() {
        Some((s, rest)) => (s.as_str(), rest),
        None => ("status", args),
    };
    if sub != "status" {
        env.eprintln("usage: sarina tiers status [--escalate-to-claude-code] [--claude-code-cli-path P] [--escalation-trigger T] [--disable-resource-check]");
        return EXIT_USAGE;
    }
    let mut fs = FlagSet::new("tiers status");
    register_escalation(&mut fs);
    if let Err(e) = fs.parse(args, &mut env.stderr) {
        return if e == HELP { EXIT_OK } else { EXIT_USAGE };
    }
    let getenv = env.getenv_owned();
    let (mut cfg, err) = load(&env.home, Some(&*getenv));
    if let Some(e) = err {
        env.eprintln(&format!("sarina: {e}"));
        return EXIT_FAILURE;
    }
    if let Some(eo) = escalation_options(&fs) {
        cfg = from_escalation(&cfg, &crate::config::EscalationConfig::default(), &eo, Some(&*getenv));
    }
    for w in cfg.warnings.clone().unwrap_or_default() {
        env.eprintln(&format!("[tier] warning: {w}"));
    }
    let base = crate::config::model_from_env(&*getenv);
    let path = cfg.path.clone();
    let r = match Router::new(Options { config: cfg, base, getenv, ..Options::default() }) {
        Ok(r) => r,
        Err(e) => {
            env.eprintln(&format!("sarina: {e}"));
            return EXIT_FAILURE;
        }
    };
    let rows = r.status(&Ctx::background());
    let resource = resource_line(&r);
    env.println(&render_tier_status(&rows, &path, &resource));
    EXIT_OK
}
