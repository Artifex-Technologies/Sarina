//! testdata/parity/agentrun.json: the agent loop against a scripted model (the same answers, from the same kind of
//! loopback server, in the Go test and here): every request body the model server got, the events, the result and the
//! session file after each prompt, the notes and the workspace's files at the end. The session id is written as SID and
//! the workspace as $W.
//!
//! One test function: it sets PATH, SHELL and SARINA_LESSONS_FILE, which the whole process shares.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use sarina::agent::run::{Agent, Options, PlanDecision};
use sarina::agent::Event;
use sarina::config::{default_runtime, BudgetConfig, ContextConfig, ModelConfig, OutputSchema, Permissions};
use sarina::goany::Any;
use sarina::goctx::Ctx;
use sarina::gojson;
use sarina::gotime::Time;
use sarina::llm::client::Client;
use sarina::llm::types::ToolSpec;
use sarina::tools::registry::{Builder, Class, Output, Permission, Tool};
use serde_json::{json, Value};

fn opt_i(v: &Value) -> Option<i64> {
    v.as_i64()
}

fn budget(v: &Value) -> BudgetConfig {
    BudgetConfig {
        max_total_tokens: opt_i(&v["max_total_tokens"]),
        max_input_tokens: opt_i(&v["max_input_tokens"]),
        max_output_tokens: opt_i(&v["max_output_tokens"]),
        max_reasoning_tokens: opt_i(&v["max_reasoning_tokens"]),
        max_total_cost_usd: v["max_total_cost_usd"].as_f64(),
        max_tool_calls: opt_i(&v["max_tool_calls"]),
        max_delegated_tasks: opt_i(&v["max_delegated_tasks"]),
        max_model_calls: opt_i(&v["max_model_calls"]),
        max_session_turns: opt_i(&v["max_session_turns"]),
    }
}

fn context(v: &Value) -> Option<ContextConfig> {
    if v.is_null() {
        return None;
    }
    let n = |k: &str| v[k].as_i64().unwrap_or(0);
    let b = |k: &str| v[k].as_bool().unwrap_or(false);
    Some(ContextConfig {
        disable_notes: b("disable_notes"),
        notes_every_turns: n("notes_every_turns"),
        notes_window_percent: n("notes_window_percent"),
        disable_clearing: b("disable_clearing"),
        clear_window_percent: n("clear_window_percent"),
        clear_keep_recent: n("clear_keep_recent"),
        clear_min_tokens: n("clear_min_tokens"),
        disable_llm_compaction: b("disable_llm_compaction"),
        disable_discovery: b("disable_context_discovery"),
    })
}

fn events_json(evs: &[Event]) -> String {
    if evs.is_empty() {
        return "null".into();
    }
    let parts: Vec<String> = evs.iter().map(|e| gojson::any_json(&Any::Object(e.clone()), true).unwrap()).collect();
    format!("[{}]", parts.join(","))
}

fn probe_tool(out: String, cancel: bool) -> Tool {
    let params = gojson::unmarshal_any(br#"{"type":"object","properties":{}}"#).0.into_object().ok();
    Tool {
        spec: ToolSpec { name: "probe".into(), description: "test tool probe".into(), parameters: params },
        handler: Arc::new(move |_, tc| {
            if cancel {
                tc.ctx.cancel();
            }
            Output { content: out.clone(), metadata: BTreeMap::new(), error: None }
        }),
        requires: Permission::None,
        class: Class::Unset,
        stream: None,
    }
}

fn files_of(ws: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![ws.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let p = e.path().to_str().unwrap().to_owned();
            let rel = p[ws.len() + 1..].to_owned();
            if e.file_type().unwrap().is_dir() {
                out.push(format!("{rel}/"));
                stack.push(p);
                continue;
            }
            let b = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
            let shown = if p.ends_with(".json") && !b.contains("parent_session_id") { "(the session file above)".to_owned() } else { b };
            out.push(format!("{rel}: {shown}"));
        }
    }
    out.sort();
    out
}

#[test]
fn agent_runs_match_go() {
    std::env::set_var("SARINA_LESSONS_FILE", "off");
    std::env::set_var("SHELL", "/bin/bash");
    // the workspace's path is in the system prompt, so its length is part of every token estimate: the Go test's shape,
    // /tmp/sarina-agentrun-XXXXXXXX/wNN
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let tmp = std::path::PathBuf::from(format!("/tmp/sarina-agentrun-{:08x}", stamp ^ std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let bin = std::env::temp_dir().join(format!("sarina-parity-agentrun-bin-{}", std::process::id()));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("PATH", &bin);
    for k in [
        "SARINA_CONTEXT_WINDOW",
        "SARINA_REQUEST_MAX_TOKENS",
        "SARINA_SESSION_NOTES",
        "SARINA_CLEAR_TOOL_OUTPUTS",
        "SARINA_LLM_COMPACTION",
        "SARINA_CONTEXT_DISCOVERY",
        "HEIMDALL_REVIEW_URL",
        "SENTRY_REVIEW_URL",
        "SARINA_SESSION_FILE_MODE",
    ] {
        std::env::set_var(k, "");
    }
    let mut check = common::Check::new();
    let ids = regex::Regex::new("[0-9a-f]{32}").unwrap();
    for (n, c) in common::cases("agentrun").iter().enumerate() {
        let sc = &c["in"]["scen"];
        let name = sc["name"].as_str().unwrap();
        let srv = common::raw_seq(common::steps(&c["in"]["script"]), None);
        let ws_dir = tmp.join(format!("w{n:02}"));
        std::fs::create_dir_all(&ws_dir).unwrap();
        let ws = String::from_utf8(sarina::gopath::eval_symlinks(ws_dir.to_str().unwrap().as_bytes()).unwrap()).unwrap();
        if let Some(files) = sc["files"].as_object() {
            for (f, body) in files {
                std::fs::write(format!("{ws}/{f}"), body.as_str().unwrap()).unwrap();
            }
        }
        let mut rt = default_runtime(&ws);
        rt.disable_claude_md_discovery = true;
        rt.stream_model_responses = sc["stream"].as_bool().unwrap();
        rt.budget_config = budget(&sc["budget"]);
        rt.permissions = Permissions { allow_file_write: sc["write"].as_bool().unwrap(), ..Default::default() };
        rt.context_management = context(&sc["context"]);
        if let Some(m) = sc["max_turns"].as_i64().filter(|m| *m > 0) {
            rt.max_turns = m;
        }
        if let Some(t) = sc["snip"].as_i64().filter(|t| *t > 0) {
            rt.auto_snip_threshold_tokens = Some(t);
        }
        if let Some(p) = sc["preserve"].as_i64().filter(|p| *p > 0) {
            rt.compact_preserve_messages = p;
        }
        if let Some(t) = sc["compact"].as_i64().filter(|t| *t > 0) {
            rt.auto_compact_threshold_tokens = Some(t);
        }
        if sc["schema"].as_bool().unwrap() {
            let schema = gojson::unmarshal_any(br#"{"type":"object"}"#).0.into_object().unwrap();
            rt.output_schema = Some(OutputSchema { name: "answer".into(), schema, strict: true });
        }
        let run_ctx = Ctx::background();
        let model = ModelConfig {
            model: "test-model".into(),
            base_url: format!("http://{}", srv.addr),
            api_key: "test-api-key-do-not-persist-4242".into(),
            timeout_seconds: 5.0,
            ..Default::default()
        };
        let mut client = Client::new(model.clone());
        client.retry_delay = Duration::from_millis(1);
        let all = sarina::tools::files::file_tools();
        let pick = |n: &str| all.iter().find(|t| t.spec.name == n).unwrap().clone();
        let mut extra = vec![pick("read_file"), pick("write_file"), pick("edit_file"), pick("list_dir")];
        extra.extend(sarina::tools::plan::plan_tools());
        extra.push(probe_tool(sc["probe"].as_str().unwrap_or("").to_owned(), sc["cancel"].as_bool().unwrap()));
        let opts = Options {
            model,
            runtime: rt.clone(),
            client: Arc::new(client),
            tools: Builder { extra, ..Builder::default() },
            prompt: Default::default(),
            context_window: sc["window"].as_i64().unwrap_or(0),
            now: Arc::new(|| Time::from_unix(1_791_115_200, 0)),
            home: ws.clone(),
            on_event: None,
            getenv: Arc::new(|_: &str| String::new()),
            discoverer: None,
            scanner: None,
            tiers: None,
            session_env: BTreeMap::new(),
            parent_link: None,
            approver: None,
            confirm: None,
            subagents: if sc["agents"].as_bool().unwrap() {
                Some(Arc::new(sarina::agents::Catalog { profiles: sarina::agents::builtins(), problems: vec![] }))
            } else {
                None
            },
        };
        let mut a = Agent::new(opts).unwrap();
        if sc["plan"].as_bool().unwrap() {
            a.enter_plan(None);
        }
        let decide = sc["decide"].as_str().unwrap_or("").to_owned();
        if !decide.is_empty() {
            a.set_plan_decider(Some(Arc::new(move |_: &str| PlanDecision { action: decide.clone(), feedback: String::new() })));
        }
        let mut sid = String::new();
        let mut turns = Vec::new();
        for (pi, p) in sc["prompts"].as_array().unwrap().iter().enumerate() {
            let p = p.as_str().unwrap();
            let (res, err) = if pi == 0 {
                a.run(&run_ctx, p)
            } else {
                let b = std::fs::read(sarina::session::path(&rt.session_directory, &sid)).unwrap();
                a.resume(&run_ctx, p, sarina::session::parse(&b).unwrap())
            };
            let res = res.unwrap_or_else(|| panic!("{name}: {err:?}"));
            sid = res.session_id.clone();
            let clean = |s: &str| ids.replace_all(&s.replace(&srv.addr, "$ADDR").replace(&ws, "$W"), "SID").into_owned();
            let file = std::fs::read(&res.session_path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let notes = std::fs::read(a.notes_path(&sid)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            turns.push(json!({
                "final": clean(&res.final_output), "turns": res.turns, "tool_calls": res.tool_calls, "stop": res.stop_reason,
                "usage": res.usage.to_json(), "cost": res.total_cost_usd, "plan": res.plan, "err": err.unwrap_or_default(),
                "events": clean(&events_json(&res.events)), "session": clean(&file), "path": clean(&res.session_path),
                "notes": clean(&notes), "scratch": clean(&res.scratchpad_directory),
            }));
        }
        let clean = |s: &str| ids.replace_all(&s.replace(&srv.addr, "$ADDR").replace(&ws, "$W"), "SID").into_owned();
        let reqs: Vec<String> = common::settle(&srv.seen).iter().map(|r| clean(r["body"].as_str().unwrap_or(""))).collect();
        let want = &c["out"];
        let want_turns = want["turns"].as_array().unwrap();
        check.eq(format!("{name}: prompts"), &json!(turns.len()), &json!(want_turns.len()));
        for (k, (got, w)) in turns.iter().zip(want_turns).enumerate() {
            for key in ["final", "turns", "tool_calls", "stop", "usage", "plan", "err", "events", "session", "path", "notes", "scratch"] {
                let (g, x) = (got[key].as_str().unwrap_or(""), w[key].as_str().unwrap_or(""));
                if g != x {
                    let at = g.bytes().zip(x.bytes()).take_while(|(a, b)| a == b).count();
                    let from = g[..at].char_indices().rev().nth(60).map_or(0, |(i, _)| i);
                    eprintln!("{name} #{k} {key} differs at {at}:\n  rust: {:.300}\n  go:   {:.300}", &g[from..], &x[from..]);
                }
                check.eq(format!("{name} #{k} {key}"), &got[key], &w[key]);
            }
            check.eq(format!("{name} #{k} cost"), &json!(got["cost"].as_f64()), &json!(w["cost"].as_f64()));
        }
        let want_reqs: Vec<String> =
            want["requests"].as_array().map(|a| a.iter().map(|v| v.as_str().unwrap().to_owned()).collect()).unwrap_or_default();
        check.eq(format!("{name}: request count"), &json!(reqs.len()), &json!(want_reqs.len()));
        for (k, (got, w)) in reqs.iter().zip(&want_reqs).enumerate() {
            check.eq(format!("{name}: request {k}"), &json!(got), &json!(w));
        }
        let files: Vec<String> = files_of(&ws).iter().map(|f| clean(f)).collect();
        let mut files = files;
        files.sort();
        check.eq(format!("{name}: files"), &json!(files), &want["files"]);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&bin);
    check.done("agentrun");
}
