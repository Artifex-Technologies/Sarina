//! testdata/parity/policy.json: Load (operator files, SARINA_POLICY_FILE, the tighten-only workspace file, the trust
//! check), Denied, DenyRules and ApplyBudget on made-up folders. Go reads keys in map order, so the fixture holds every
//! answer Go gave in 60 runs; the Rust answer must be one of them.

mod common;

use sarina::config::BudgetConfig;
use sarina::policy::{load, Options, ENV_FILE, WORKSPACE_FILE};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const MAX: usize = 256 << 10;

fn policy_file(path: &Path, raw: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let Ok(spec) = std::str::from_utf8(raw) else {
        std::fs::write(path, raw).unwrap();
        return;
    };
    if spec == "@dir" {
        std::fs::create_dir_all(path).unwrap();
    } else if spec == "@big" {
        std::fs::write(path, " ".repeat(MAX + 1)).unwrap();
    } else if let Some(body) = spec.strip_prefix("@fill:") {
        std::fs::write(path, format!("{body}{}", " ".repeat(MAX - body.len()))).unwrap();
    } else if let Some(body) = spec.strip_prefix("@unreadable:") {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    } else {
        std::fs::write(path, spec).unwrap();
    }
}

fn budget_out(b: &BudgetConfig) -> Value {
    let i = |x: Option<i64>| x.map_or(Value::Null, |v| json!(v.to_string()));
    json!([
        i(b.max_total_tokens),
        i(b.max_input_tokens),
        i(b.max_output_tokens),
        i(b.max_reasoning_tokens),
        b.max_total_cost_usd.map_or(Value::Null, |v| json!(sarina::gotext::float_v(v))),
        i(b.max_tool_calls),
        i(b.max_delegated_tasks),
        i(b.max_model_calls),
        i(b.max_session_turns),
    ])
}

fn sorted_notices(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(n) = v["notices"].as_array_mut() {
        n.sort_by_key(|x| x.to_string());
    }
    v
}

#[test]
fn policy_matches_go() {
    let mut check = common::Check::new();
    let base = std::env::temp_dir().join(format!("sarina-policy-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let cases = common::cases("policy");
    let tools: Vec<String> = cases.iter().find(|c| c["fn"] == "tools").unwrap()["in"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| String::from_utf8(common::bytes(v)).unwrap())
        .collect();
    let starts = [
        BudgetConfig::default(),
        BudgetConfig {
            max_total_tokens: Some(100),
            max_input_tokens: Some(100),
            max_output_tokens: Some(100),
            max_reasoning_tokens: Some(100),
            max_total_cost_usd: Some(100.0),
            max_tool_calls: Some(100),
            max_delegated_tasks: Some(100),
            max_model_calls: Some(100),
            max_session_turns: Some(100),
        },
        BudgetConfig {
            max_total_tokens: Some(1),
            max_tool_calls: Some(1),
            max_total_cost_usd: Some(0.25),
            max_session_turns: Some(1 << 40),
            ..BudgetConfig::default()
        },
    ];
    let wd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    let mut by_normalising = 0;
    for (n, c) in cases.iter().enumerate() {
        if c["fn"] != "load" {
            continue;
        }
        let input = &c["in"];
        let root = base.join(format!("c{n}"));
        std::fs::create_dir_all(&root).unwrap();
        let root_s = root.to_str().unwrap().to_string();
        let home = root.join("home");
        if !input["home"].is_null() {
            policy_file(&home.join(".sarina/policy.json"), &common::bytes(&input["home"]));
        }
        let ws = match input["wsDir"].as_str() {
            Some("-") => String::new(),
            Some(d) => String::from_utf8(sarina::gopath::join(&[root_s.as_bytes(), d.as_bytes()])).unwrap(),
            None => format!("{root_s}/ws"),
        };
        if !input["ws"].is_null() && !ws.is_empty() {
            policy_file(&Path::new(&ws).join(WORKSPACE_FILE), &common::bytes(&input["ws"]));
        }
        if let Some(files) = input["files"].as_object() {
            for (name, spec) in files {
                policy_file(&root.join(name), spec.as_str().unwrap().as_bytes());
            }
        }
        let extras = input["extras"].as_str().unwrap_or("").replace("{ROOT}", &root_s);
        let refuse: Vec<String> = input["refuse"]
            .as_array()
            .map(|a| a.iter().map(|p| p.as_str().unwrap().replace("{ROOT}", &root_s)).collect())
            .unwrap_or_default();
        let getenv = |k: &str| if k == ENV_FILE { extras.clone() } else { String::new() };
        let trust = |p: &str| if refuse.iter().any(|r| r == p) { "not approved (made up)".to_string() } else { String::new() };
        let opts = Options {
            home: if input["noHome"] == true { String::new() } else { home.to_str().unwrap().to_string() },
            getenv: &getenv,
            workspace: ws.clone(),
            trust: if input["refuse"].is_null() { None } else { Some(&trust) },
        };
        let p = load(&opts);
        let clean = |x: &str| x.replace(&root_s, "{ROOT}").replace(&wd, "{CWD}");
        let all = |xs: &[String]| xs.iter().map(|x| clean(x)).collect::<Vec<_>>();
        let denied: Vec<String> = tools.iter().map(|t| clean(&p.denied(t))).collect();
        let budgets: Vec<Value> = starts
            .iter()
            .map(|s| {
                let mut b = s.clone();
                p.apply_budget(&mut b);
                budget_out(&b)
            })
            .collect();
        let got = json!({"trusted": p.trusted, "safeEnv": all(&p.safe_env), "sources": all(&p.sources), "notices": all(&p.notices),
            "fatal": all(&p.fatal), "rules": p.deny_rules(), "denied": denied, "budgets": budgets});
        let outs = c["out"].as_array().unwrap();
        let want = if let Some(w) = outs.iter().find(|w| **w == got) {
            w.clone()
        } else if let Some(w) = outs.iter().find(|w| sorted_notices(w) == sorted_notices(&got)) {
            by_normalising += 1;
            w.clone()
        } else {
            outs[0].clone()
        };
        check.eq(format!("load {input}"), &got, &want);
        if !ws.is_empty() {
            let _ = std::fs::set_permissions(Path::new(&ws).join(WORKSPACE_FILE), std::fs::Permissions::from_mode(0o644));
        }
        if let Some(files) = input["files"].as_object() {
            for name in files.keys() {
                let _ = std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o644));
            }
        }
    }
    eprintln!("policy: {by_normalising} answers equal only with the notices in another order (Go's map order)");
    let _ = std::fs::remove_dir_all(&base);
    check.done("policy");
}
