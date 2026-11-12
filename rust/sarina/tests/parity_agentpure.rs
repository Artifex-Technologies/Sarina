//! testdata/parity/agentpure.json: the agent's pure helpers: token counting and the window, the budgets, the system prompt
//! with lessons, guidance and the git snapshot (against a stand-in git in bash), curated memory's block, clearing tool
//! outputs, snipping and compacting without a model, session notes' coverage and format, the transcript a summarising
//! model reads and its summary cleaned. Session files are compared by their sha256.
//!
//! One test function: it sets PATH, SHELL and SARINA_LESSONS_FILE, which the whole process shares.

mod common;

use std::collections::HashMap;

use sarina::agent::compact::*;
use sarina::agent::limits::*;
use sarina::agent::prompt::*;
use sarina::agent::Event;
use sarina::config::{BudgetConfig, ContextSettings, OutputSchema, Permissions, Pricing, RuntimeConfig};
use sarina::goany::Any;
use sarina::gojson;
use sarina::llm::types::{ToolSpec, Usage};
use sarina::session::{self, OrderedMap, Session};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Go's json.Marshal of a []string (HTML escaped).
fn go_strs(v: &[String]) -> String {
    let mut out = String::from("[");
    for (i, s) in v.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        gojson::write_str(&mut out, s, true);
    }
    out + "]"
}

fn events_json(evs: &[Option<Event>]) -> String {
    if evs.is_empty() {
        return "null".into();
    }
    let parts: Vec<String> = evs
        .iter()
        .map(|e| match e {
            Some(e) => gojson::any_json(&Any::Object(e.clone()), true).unwrap(),
            None => "null".into(),
        })
        .collect();
    format!("[{}]", parts.join(","))
}

fn digest(s: &Session) -> String {
    Sha256::digest(s.marshal().unwrap().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn parse(v: &Value) -> Session {
    session::parse(v.as_str().unwrap().as_bytes()).unwrap()
}

fn specs() -> Vec<ToolSpec> {
    let params = gojson::unmarshal_any(br#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#).0;
    vec![
        ToolSpec { name: "read_file".into(), description: "Read a file <x> & more".into(), parameters: params.into_object().ok() },
        ToolSpec { name: "bare".into(), description: "no params".into(), parameters: None },
    ]
}

fn write(p: &str, body: &[u8]) {
    std::fs::create_dir_all(std::path::Path::new(p).parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn chmod(p: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn agent_pure_helpers_match_go() {
    let cases = common::cases("agentpure");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-agentpure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = tmp.to_str().unwrap().to_owned();
    let clean = |s: &str| s.replace(&top, "$T");
    let by = |f: &str| -> Vec<&Value> { cases.iter().filter(|c| c["fn"] == f).collect() };
    let mut sessions: HashMap<i64, Value> = HashMap::new();
    for c in by("session") {
        sessions.insert(c["in"].as_i64().unwrap(), c["out"].clone());
    }
    let s1 = parse(&by("messageTokens")[0]["in"]);
    let clear_base = by("clearSession")[0]["out"].clone();
    let cov_base = by("coverageSession")[0]["out"].clone();

    // files for the lessons, the guidance and memory
    let lessons = format!("{top}/lessons.md");
    let long = format!("{}{}", "ab".repeat(140), "é".repeat(19));
    let mut body = format!(
        "# Lessons\n- Read before you write.\n-not a rule\n  - Indented rule\x07 with a bell\n- \u{200b}zero width\n- \n- {long}\n"
    );
    for i in 0..25 {
        body += &format!("- rule {i}\n");
    }
    write(&lessons, body.as_bytes());
    write(&format!("{top}/open.md"), b"- x\n");
    chmod(&format!("{top}/open.md"), 0o666);
    write(&format!("{top}/group.md"), b"- g\n");
    chmod(&format!("{top}/group.md"), 0o620);
    write(&format!("{top}/big.md"), "- y\n".repeat(20000).as_bytes());
    let home = format!("{top}/home");
    let cwd = format!("{top}/proj/sub");
    let extra = format!("{top}/extra");
    for (p, c) in [
        (format!("{home}/.claude/CLAUDE.md"), b"global".to_vec()),
        (format!("{top}/proj/CLAUDE.md"), b"proj guidance".to_vec()),
        (format!("{top}/proj/.claude/CLAUDE.md"), b"  \n ".to_vec()),
        (format!("{top}/proj/CLAUDE.local.md"), b"local\r\n".to_vec()),
        (format!("{top}/proj/.claude/rules/b.md"), b"rule b".to_vec()),
        (format!("{top}/proj/.claude/rules/a.md"), b"rule a".to_vec()),
        (format!("{top}/proj/.claude/rules/c.txt"), b"not md".to_vec()),
        (format!("{cwd}/.claude/CLAUDE.md"), "é".repeat(40010).into_bytes()),
        (format!("{extra}/CLAUDE.md"), b"bad \xff\xfe bytes".to_vec()),
        (format!("{extra}/.claude/rules/dir.md/x"), b"in a folder".to_vec()),
    ] {
        write(&p, &c);
    }
    std::os::unix::fs::symlink("../CLAUDE.md", format!("{cwd}/CLAUDE.md")).unwrap();
    std::fs::create_dir(format!("{cwd}/CLAUDE.local.md")).unwrap();
    let mws = format!("{top}/mem/ws");
    let mhome = format!("{top}/mem/home");
    let agent_mem: Vec<String> = (0..7).map(|i| format!("Project rule {i}: use cargo for builds number {i}")).collect();
    write(&format!("{mws}/.port_sessions/memory/AGENT_MEMORY.md"), agent_mem.join("\n§\n").as_bytes());
    write(
        &format!("{mhome}/.sarina/memory/USER.md"),
        "The owner likes short answers\n§\nPrefers Rust over Go\n§\nUses Hyprland".as_bytes(),
    );
    std::env::set_var("SHELL", "/usr/bin/zsh");

    let u = Usage {
        input_tokens: 123456,
        output_tokens: 7890,
        cache_creation_input_tokens: 11,
        cache_read_input_tokens: 222,
        reasoning_tokens: 5,
    };
    let pr = Pricing { input_per_million: 3.0, output_per_million: 15.0, cache_creation_per_million: 3.75, cache_read_per_million: 0.3 };
    let budgets = [
        BudgetConfig::default(),
        BudgetConfig { max_output_tokens: Some(500), ..Default::default() },
        BudgetConfig { max_output_tokens: Some(-3), max_input_tokens: Some(100), ..Default::default() },
        BudgetConfig { max_input_tokens: Some(100000), ..Default::default() },
    ];
    let budgets2 = [
        BudgetConfig::default(),
        BudgetConfig { max_total_tokens: Some(100), ..Default::default() },
        BudgetConfig { max_input_tokens: Some(1000000), max_output_tokens: Some(7000), ..Default::default() },
        BudgetConfig { max_reasoning_tokens: Some(4), ..Default::default() },
        BudgetConfig { max_total_cost_usd: Some(0.0001), ..Default::default() },
        BudgetConfig { max_tool_calls: Some(2), ..Default::default() },
        BudgetConfig { max_delegated_tasks: Some(0), ..Default::default() },
        BudgetConfig { max_model_calls: Some(3), ..Default::default() },
        BudgetConfig { max_session_turns: Some(9), ..Default::default() },
        BudgetConfig { max_session_turns: Some(10), max_tool_calls: Some(3), max_total_tokens: Some(1 << 30), ..Default::default() },
        BudgetConfig { max_total_cost_usd: Some(cost(&pr, &u)), ..Default::default() },
    ];
    let prompt_opts = [
        PromptOptions::default(),
        PromptOptions { system: "Custom body.".into(), ..Default::default() },
        PromptOptions { append: "Appended.".into(), ..Default::default() },
        PromptOptions { override_: "Only this.".into(), ..Default::default() },
        PromptOptions { override_: "  ".into(), system: " ".into(), append: " ".into() },
    ];
    let tool_sets: [Vec<String>; 3] = [vec![], vec!["bash".into()], vec!["read_file".into(), "bash".into()]];
    let perm_sets = [
        Permissions::default(),
        Permissions { allow_file_write: true, ..Default::default() },
        Permissions { allow_file_write: true, allow_shell_commands: true, ..Default::default() },
    ];
    let env = EnvInfo { cwd: "/w/proj".into(), is_git: true, extra_dirs: vec!["/a".into(), "/b".into()], model: "qwen3:8b".into() };
    let clear_settings =
        [(true, 0.5, 2, 50), (true, 0.01, 0, 0), (true, 0.9, 1, 10), (false, 0.01, 0, 0), (true, 0.01, -4, 100000), (true, 0.01, 0, 225)];
    let reduce_opts = [
        ReduceOpts { target: 0, preserve: 4, turn: 3, ..Default::default() },
        ReduceOpts { target: 500, preserve: 0, turn: 3, ..Default::default() },
        ReduceOpts { target: 3000, allow_compaction: true, preserve: 4, turn: 5, ..Default::default() },
        ReduceOpts { target: 0, allow_compaction: true, preserve: 50, turn: 5, notes: "NOTES <kept>".into(), ..Default::default() },
        ReduceOpts { target: 1 << 30, allow_compaction: true, preserve: 4, turn: 5, ..Default::default() },
        ReduceOpts { target: 0, allow_compaction: true, reactive: true, preserve: 2, turn: 6, ..Default::default() },
        ReduceOpts { target: 200, allow_compaction: true, reactive: true, preserve: 0, turn: 6, ..Default::default() },
        ReduceOpts { target: 1500, allow_compaction: true, preserve: 3, turn: 4, ..Default::default() },
    ];

    for c in &cases {
        let (f, input, want) = (c["fn"].as_str().unwrap(), &c["in"], &c["out"]);
        let got: Value = match f {
            "session" | "clearSession" | "coverageSession" => continue,
            "countTokens" => json!(count_tokens(input.as_str().unwrap())),
            "inferWindow" => json!(infer_context_window(input.as_str().unwrap())),
            "snapshot" => {
                let rt = RuntimeConfig {
                    budget_config: budgets[input["budget"].as_u64().unwrap() as usize].clone(),
                    output_schema: if input["schema"].as_bool().unwrap() {
                        Some(OutputSchema { name: "x".into(), ..Default::default() })
                    } else {
                        None
                    },
                    ..Default::default()
                };
                let sp = if input["specs"].as_u64().unwrap() > 0 { specs() } else { vec![] };
                json!(take_snapshot(input["window"].as_i64().unwrap(), &s1.messages, &sp, &rt).to_json())
            }
            "messageTokens" => json!([message_tokens(&s1.messages), tool_spec_tokens(&specs())]),
            "cost" => {
                let g: f64 = want.as_str().unwrap().parse().unwrap();
                check.eq("cost", &json!(cost(&pr, &u).to_bits()), &json!(g.to_bits()));
                continue;
            }
            "checkBudget" => {
                let ctr = Counters { usage: u, cost: cost(&pr, &u), tool_calls: 3, delegated: 1, model_calls: 4, session_turns: 10 };
                json!(check_budget(&budgets2[input.as_u64().unwrap() as usize], &ctr))
            }
            "windowSource" => json!(describe_window_source(input.as_str().unwrap())),
            "lessons" => {
                std::env::set_var("SARINA_LESSONS_FILE", input.as_str().unwrap().replace("$T", &top));
                json!(lessons_part())
            }
            "readLessonsDefault" => json!(read_lessons(&lessons, false).is_empty()),
            "promptParts" => {
                std::env::set_var("SARINA_LESSONS_FILE", &lessons);
                std::env::set_var("SHELL", "/usr/bin/zsh");
                let ix: Vec<usize> = input.as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
                json!(go_strs(&build_system_prompt_parts(&prompt_opts[ix[0]], &env, &tool_sets[ix[1]], &perm_sets[ix[2]])))
            }
            "environment" => {
                std::env::set_var("SHELL", "");
                json!(environment_section(&EnvInfo { cwd: "/x".into(), ..Default::default() }))
            }
            "userContext" => {
                let mut uc = OrderedMap::default();
                uc.set("currentDate", "Today's date is 2026-11-12.");
                uc.set("empty", "  ");
                uc.set("claudeMd", "guidance <b>");
                json!(render_user_context(&uc))
            }
            "systemMessage" => {
                let mut sc = OrderedMap::default();
                sc.set("k", "v");
                sc.set("blank", "");
                json!([render_system_message(&["a".into(), "b".into()], &sc), render_system_message(&[], &OrderedMap::default())])
            }
            "guidance" => json!([
                clean(&discover_guidance(&cwd, &[extra.clone(), cwd.clone()], &home)),
                clean(&discover_guidance(&extra, &[], "")),
                clean(&discover_guidance(&format!("{top}/nothing"), &[], &format!("{top}/nohome"))),
            ]),
            "gitSnapshot" => {
                let name = input["name"].as_str().unwrap();
                let dir = format!("{top}/git/{}", name.replace(' ', "-"));
                std::fs::create_dir_all(format!("{dir}/bin")).unwrap();
                std::fs::create_dir_all(format!("{dir}/ws")).unwrap();
                if name != "not a repo" {
                    std::fs::create_dir(format!("{dir}/ws/.git")).unwrap();
                }
                write(&format!("{dir}/bin/git"), input["script"].as_str().unwrap().as_bytes());
                chmod(&format!("{dir}/bin/git"), 0o755);
                std::env::set_var("PATH", format!("{dir}/bin"));
                let ws = format!("{dir}/ws");
                json!([is_git_repo(&ws), clean(&git_snapshot(&ws))])
            }
            "curatedMemory" => json!(curated_memory(&mws, &mhome, input.as_str().unwrap())),
            "clear" => {
                let ix: Vec<i64> = input.as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
                let (on, frac, keep, min) = clear_settings[ix[0] as usize];
                let st = ContextSettings {
                    notes: false,
                    notes_every_turns: 0,
                    notes_window_fraction: 0.0,
                    clear: on,
                    clear_window_fraction: frac,
                    clear_keep_recent: keep,
                    clear_min_tokens: min,
                    llm_compaction: false,
                    discovery: false,
                };
                let mut s = parse(&clear_base);
                match clear_tool_outputs(&mut s, ix[1], 30, &st, 7) {
                    None => Value::Null,
                    Some(ev) => {
                        let ev2 = clear_tool_outputs(&mut s, ix[1], 30, &st, 8);
                        json!([events_json(&[Some(ev)]), digest(&s), ev2.is_some(), events_json(&[ev2])])
                    }
                }
            }
            "outcome" => json!(outcome_lines(input.as_str().unwrap())),
            "prefix" => {
                let s = parse(&sessions[&input.as_i64().unwrap()]);
                let m = &s.messages;
                let p = protected_prefix(m);
                json!([p, pair_safe_cut(m, p, 4), pair_safe_cut(m, 1, 0), pair_safe_cut(m, 0, 1000), pair_safe_cut(m, 3, 5)])
            }
            "reduce" => {
                let ix: Vec<i64> = input.as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
                let mut s = parse(&sessions[&ix[0]]);
                let (changed, evs) = reduce(&mut s, &reduce_opts[ix[1] as usize]);
                let evs: Vec<Option<Event>> = evs.into_iter().map(Some).collect();
                json!([changed, events_json(&evs), digest(&s), s.messages.len()])
            }
            "coverage" => {
                let (c, ok) = notes_coverage(&parse(&sessions[&input.as_i64().unwrap()]));
                json!([c, ok])
            }
            "groups" => {
                let g = group_messages(&parse(&sessions[&input.as_i64().unwrap()]).messages);
                let parts: Vec<String> = g.iter().map(|ms| session::message::messages_json(ms).unwrap()).collect();
                json!(gojson::escape_html(&format!("[{}]", parts.join(","))).len())
            }
            "render" => {
                let s = parse(&sessions[&input.as_i64().unwrap()]);
                json!([render_transcript(&s.messages, 1500), render_transcript(&s.messages[..s.messages.len().min(3)], 5)])
            }
            "tail" => {
                let s = parse(&sessions[&input.as_i64().unwrap()]);
                let t: Vec<usize> = [0, 100, 2000, 20000, 1 << 30].iter().map(|&mx| tail_within(&s.messages, 1500, mx).len()).collect();
                json!(t)
            }
            "groupsEmpty" => json!(if group_messages(&[]).is_empty() { "null" } else { "?" }),
            "setCoverage" => {
                let mut s = parse(&cov_base);
                set_notes_coverage(&mut s, input.as_i64().unwrap());
                let (c, ok) = notes_coverage(&s);
                json!([gojson::any_json(&Any::Object(s.plugin_state.clone()), true).unwrap(), c, ok])
            }
            "remap" => {
                let p: Vec<i64> = input.as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
                json!(remap_coverage(p[0], p[1], p[2]))
            }
            "collapse" => json!([collapse(input.as_str().unwrap(), 5), collapse(input.as_str().unwrap(), 160)]),
            "truncate" => json!([truncate_chars(input.as_str().unwrap(), 3), truncate_chars(input.as_str().unwrap(), 10)]),
            "section" => json!(section_index(input.as_str().unwrap())),
            "normalise" => match normalise_notes(input.as_str().unwrap()) {
                Some(t) => json!([t, true]),
                None => json!(["", false]),
            },
            "sideMax" => {
                let w = input.as_i64().unwrap();
                json!([side_max_tokens(w, 2048), side_max_tokens(w, 3000)])
            }
            "cleanSummary" => json!(clean_summary(input.as_str().unwrap())),
            "tooLong" => {
                let s = input.as_str().unwrap();
                json!([reply_too_long(s), err_too_long(s), is_prompt_too_long(s)])
            }
            "foldedBody" => json!(folded_body(7, "text", "closing")),
            "reminder" => json!([reminder(PLAN_MODE_NOTICE), reminder("")]),
            _ => panic!("unknown case {f}"),
        };
        check.eq(format!("{f} {input}"), &got, want);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("agentpure");
}
