//! testdata/parity/runkitbuild.json: runkit::build from plain settings (allowlists, a policy, a damaged policy, compute
//! tiers, the claude CLI escalation, sub-agents, the prompt options), each agent run once against the same scripted model
//! as Go's: the notices, whether a Router was made, the error, the answer and every request body. The tools are the Go
//! test binary's built-ins (the default tools and memory's).
//!
//! One test function: it sets process variables (the lessons file, the trust folder).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use sarina::runkit::{build, Spec};
use serde_json::{json, Value};

#[test]
fn runkit_build_matches_go() {
    std::env::set_var("SARINA_LESSONS_FILE", "off");
    let trust = std::env::temp_dir().join(format!("sarina-runkit-trust-{}", std::process::id()));
    std::fs::create_dir_all(&trust).unwrap();
    std::env::set_var("SARINA_TRUST_DIR", &trust);
    let top = format!("/tmp/sarina-runkit-{:08x}", std::process::id().wrapping_mul(2654435761));
    let _ = std::fs::remove_dir_all(&top);
    std::fs::create_dir(&top).unwrap();
    let date = regex::Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
    let hex = regex::Regex::new(r"[0-9a-f]{32}").unwrap();
    let mut check = common::Check::new();
    for (n, c) in common::cases("runkitbuild").iter().enumerate() {
        let sc = &c["in"];
        let script: Vec<Vec<common::Step>> = sc["answers"]
            .as_array()
            .map(|a| a.iter().map(|x| vec![common::Step::Send(x.as_str().unwrap().as_bytes().to_vec())]).collect())
            .unwrap_or_default();
        let srv = common::raw_seq(script, None);
        let addr = srv.addr.clone();
        let ws = format!("{top}/w{n:02}");
        std::fs::create_dir_all(&ws).unwrap();
        if let Some(files) = sc["files"].as_object() {
            for (name, body) in files {
                let p = std::path::Path::new(&ws).join(name);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, body.as_str().unwrap().replace("$ADDR", &addr)).unwrap();
            }
        }
        let mut rt = sarina::config::default_runtime(&ws);
        rt.disable_claude_md_discovery = true;
        rt.permissions.allow_file_write = sc["allow_write"].as_bool().unwrap();
        let allow = sc["allow_set"]
            .as_bool()
            .unwrap()
            .then(|| sc["allow"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect()).unwrap_or_default());
        let escalation = sc["escalate"].as_bool().unwrap().then(|| sarina::tiers::config::EscalationOptions {
            enabled: true,
            cli_path: format!("{ws}/no-claude"),
            triggers: vec!["backend_error".into()],
            ..Default::default()
        });
        let env: HashMap<String, String> =
            sc["env"].as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect()).unwrap_or_default();
        let builtins = sarina::cli::chat::builtins()
            .into_iter()
            .filter(|t| !t.spec.name.starts_with("limbo_") && t.spec.name != "sarina_doctor")
            .collect();
        let built = build(Spec {
            model: sarina::config::ModelConfig {
                model: "test-model".into(),
                base_url: format!("http://{addr}/v1"),
                api_key: "parity-key".into(),
                timeout_seconds: 5.0,
                ..Default::default()
            },
            runtime: rt,
            allowlist: allow,
            prompt: sarina::agent::prompt::PromptOptions {
                override_: sc["override"].as_str().unwrap().into(),
                append: sc["append"].as_str().unwrap().into(),
                ..Default::default()
            },
            home: ws.clone(),
            getenv: Some(Arc::new(move |k: &str| env.get(k).cloned().unwrap_or_default())),
            client: None,
            approver: None,
            discover: false,
            use_tiers: sc["use_tiers"].as_bool().unwrap(),
            escalation,
            subagents: sc["subagents"].as_bool().unwrap(),
            builtins: Some(builtins),
            trust: None,
        });
        let clean = |s: &str| {
            let s = s.replace(&addr, "$ADDR").replace(&ws, "$W");
            let s = hex.replace_all(&s, "SID");
            date.replace_all(&s, "DATE").into_owned()
        };
        let mut out = serde_json::Map::new();
        match built {
            Err(e) => {
                out.insert("err".into(), json!(clean(&e)));
            }
            Ok(mut b) => {
                out.insert("err".into(), json!(""));
                let notices: Vec<String> = b.notices.iter().map(|x| clean(x)).collect();
                out.insert("notices".into(), if notices.is_empty() { Value::Null } else { json!(notices) });
                out.insert("router".into(), json!(b.router.is_some()));
                out.insert("tools".into(), json!(b.agent.tool_count()));
                let (res, err) = b.agent.run(&sarina::goctx::Ctx::background(), "hello");
                out.insert("run_err".into(), json!(clean(&err.unwrap_or_default())));
                if let Some(r) = res {
                    out.insert("final".into(), json!(clean(&r.final_output)));
                    out.insert("stop".into(), json!(r.stop_reason));
                }
                let reqs: Vec<String> = common::settle(&srv.seen)
                    .iter()
                    .map(|r| format!("{} {}", clean(r["uri"].as_str().unwrap_or("")), clean(r["body"].as_str().unwrap_or(""))))
                    .collect();
                out.insert("requests".into(), json!(reqs));
            }
        }
        check.eq(sc["name"].as_str().unwrap(), &Value::Object(out), &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&top);
    let _ = std::fs::remove_dir_all(&trust);
    check.done("runkitbuild");
}
