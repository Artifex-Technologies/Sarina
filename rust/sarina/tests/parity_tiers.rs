//! testdata/parity/tiers.json: the pure parts of compute tiers: Parse, Load, LooksLikeKey, FromEscalation, the secret
//! redactor, the capture check, Sanitize, Pressure, the samplers' parsers, the claude CLI's prompt, command line and
//! environment, the wake command's environment, offMachine, estimateTokens.

mod common;

use sarina::config::{EscalationConfig, ResourceThresholds};
use sarina::goany::Any;
use sarina::gotext::float_v;
use sarina::llm::openai::Request;
use sarina::llm::types::{FunctionCall, Message, ToolSpec, WireToolCall};
use sarina::tiers::cli::{build_prompt, secret_env_name, CliClient};
use sarina::tiers::config::*;
use sarina::tiers::guard::{describe_guard, looks_like_capture, redact_secrets, sanitize};
use sarina::tiers::resource::{parse_meminfo, parse_nvidia, Sample};
use sarina::tiers::router::{estimate_tokens, off_machine};
use sarina::tiers::wake::filtered_env;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn tier_out(t: &Tier) -> Value {
    json!({"name": t.name, "kind": t.kind, "base_url": t.base_url, "model": t.model, "api_key_env": t.api_key_env,
        "allow": t.allow_data.to_json(), "allow_str": t.allow_data.string(), "max_context": t.max_context, "wake": t.wake,
        "wake_wait": float_v(t.wake_wait), "enabled": t.enabled, "auto": t.auto, "cli_path": t.cli_path,
        "timeout": float_v(t.timeout_seconds), "max_esc": t.max_escalations, "triggers": t.triggers, "extra_args": t.extra_args,
        "extra_dirs": t.extra_dirs, "accept_edits": t.accept_edits})
}

fn cfg_out(c: &Config, err: Option<&str>) -> Value {
    let tiers: Vec<Value> = c.tiers.iter().map(tier_out).collect();
    let r = &c.resource;
    json!({"err": err.unwrap_or(""), "tiers": if tiers.is_empty() { Value::Null } else { Value::Array(tiers) },
        "warnings": c.warnings, "path": c.path, "multi": c.multi(),
        "resource": [float_v(r.max_memory_percent), float_v(r.max_cpu_percent), float_v(r.max_gpu_memory_percent)]})
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect()).unwrap_or_default()
}

fn opt_strs(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
}

fn local_tier(url: &str) -> Tier {
    Tier {
        name: "local".into(),
        kind: KIND_OPENAI_COMPAT.into(),
        base_url: url.into(),
        model: "local-model".into(),
        allow_data: AllowData::all(),
        enabled: true,
        auto: true,
        ..Tier::default()
    }
}

fn desktop_tier(url: &str) -> Tier {
    Tier {
        name: "desktop".into(),
        kind: KIND_OPENAI_COMPAT.into(),
        base_url: url.into(),
        model: "big-model".into(),
        allow_data: AllowData::of(&[CLASS_NO_SECRETS]),
        enabled: true,
        auto: true,
        ..Tier::default()
    }
}

fn cli_tier(path: &str) -> Tier {
    Tier {
        name: "claude-cli".into(),
        kind: KIND_CLAUDE_CLI.into(),
        cli_path: path.into(),
        timeout_seconds: 30.0,
        max_escalations: 1,
        triggers: Some(vec![TRIGGER_BACKEND_ERROR.into(), TRIGGER_RESOURCE_EXHAUSTED.into()]),
        allow_data: AllowData::of(&[CLASS_NO_SECRETS, CLASS_NO_CAPTURES]),
        enabled: true,
        auto: true,
        ..Tier::default()
    }
}

fn msg(role: &str, content: &str) -> Message {
    Message { role: role.into(), content: content.into(), ..Message::default() }
}

fn call(id: &str, kind: &str, name: &str, args: &str) -> WireToolCall {
    WireToolCall { id: id.into(), kind: kind.into(), function: FunctionCall { name: name.into(), arguments: args.into() } }
}

fn obj(v: Value) -> BTreeMap<String, Any> {
    sarina::gojson::unmarshal_any(v.to_string().as_bytes()).0.into_object().unwrap()
}

fn sanitize_requests() -> Vec<Request> {
    let tool = ToolSpec {
        name: "read_file".into(),
        description: "Read a file <safely> & quickly".into(),
        parameters: Some(obj(json!({"type": "object"}))),
    };
    let tool_b =
        ToolSpec { name: "b".into(), description: String::new(), parameters: Some(obj(json!({"properties": {"x": {"type": "string"}}}))) };
    let mut asst = msg("assistant", "");
    asst.tool_calls = vec![
        call("c1", "function", "bash", r#"{"command":"curl -H 'Authorization: Bearer abcdefghijklmnop1234'"}"#),
        call("c2", "function", "noop", "{}"),
    ];
    let mut tool_msg = msg("tool", "password=hunter22");
    tool_msg.tool_call_id = "c1".into();
    let mut a3 = msg("assistant", "");
    a3.tool_calls = vec![call("c3", "", "write_file", "{\"content\":\"RAW_Data: -1 2\\n\"}")];
    let mut a4 = msg("assistant", "");
    a4.tool_calls = vec![call("c4", "", "x", "RAW_Data: 5")];
    vec![
        Request { messages: vec![msg("system", "be brief"), msg("user", "hello")], ..Request::default() },
        Request {
            messages: vec![msg("user", "my key is sk-ant-api03-ABCDEFGHIJKLMNOP and local-key-0123456789"), asst, tool_msg],
            tools: vec![tool.clone()],
            ..Request::default()
        },
        Request { messages: vec![msg("user", "dump:\nFiletype: Flipper NFC device\nUID: 04 A1 B2 C3 ")], ..Request::default() },
        Request { messages: vec![msg("user", "hi"), a3, a4], ..Request::default() },
        Request {
            messages: vec![msg("user", &"é".repeat(100)), msg("assistant", "my-custom-literal-secret-9999")],
            tools: vec![tool, tool_b],
            ..Request::default()
        },
        Request::default(),
    ]
}

fn prompt_messages() -> Vec<Vec<Message>> {
    let long = format!("{}é{}", "ab".repeat(250), "c".repeat(10));
    let e401 = "é".repeat(401);
    vec![
        vec![],
        vec![msg("system", "sys"), msg("user", "fix the parser")],
        vec![
            msg("user", "first task"),
            msg("assistant", "  working  "),
            msg("tool", "out"),
            msg("user", "<system-reminder>ctx</system-reminder>"),
            msg("user", "   "),
        ],
        vec![
            msg("user", "t1"),
            msg("assistant", "a1"),
            msg("user", "t2"),
            msg("assistant", ""),
            msg("tool", &long),
            msg("user", "t3"),
            msg("assistant", "a3"),
            msg("system", "s"),
            msg("user", "t4"),
            msg("assistant", "a4"),
            msg("tool", &e401),
            msg("user", "  t5 \n"),
        ],
        vec![msg("assistant", "only assistant")],
        vec![msg("user", " <system-reminder> spaced")],
    ]
}

fn msgs_out(ms: &[Message]) -> Value {
    if ms.is_empty() {
        return Value::Null;
    }
    Value::Array(
        ms.iter()
            .map(|m| {
                let calls: Vec<Value> = m.tool_calls.iter().map(|c| json!([c.id, c.function.name, c.function.arguments])).collect();
                json!([m.role, m.content, if calls.is_empty() { Value::Null } else { Value::Array(calls) }, m.tool_call_id])
            })
            .collect(),
    )
}

fn lossy(v: &Value) -> String {
    String::from_utf8_lossy(&common::bytes(v)).into_owned()
}

#[test]
fn tiers_match_go() {
    let cases = common::cases("tiers");
    let mut check = common::Check::new();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-tiers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(sarina::gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let hide = |s: &str| s.replace(&top, "$BASE");
    let known =
        ["my-custom-literal-secret-9999".to_string(), "short".into(), "local-key-0123456789".into(), "eight888".into(), "seven77".into()];
    let knowns: [&[String]; 2] = [&[], &known];
    let reqs = sanitize_requests();
    let allows = [
        AllowData::all(),
        AllowData::default(),
        AllowData::of(&[CLASS_NO_SECRETS]),
        AllowData::of(&[CLASS_NO_CAPTURES]),
        AllowData::of(&[CLASS_NO_SECRETS, CLASS_NO_CAPTURES]),
        AllowData { all: true, classes: vec![CLASS_NO_CAPTURES.into()] },
    ];
    let ths = [
        Thresholds::default(),
        Thresholds { max_memory_percent: 90.0, max_cpu_percent: 0.0, max_gpu_memory_percent: 90.0 },
        Thresholds { max_memory_percent: 0.0, max_cpu_percent: 50.0, max_gpu_memory_percent: 0.0 },
        Thresholds { max_memory_percent: 0.5, max_cpu_percent: 2.5, max_gpu_memory_percent: 99.5 },
    ];
    let sm = |m: f64, mo: bool, c: f64, co: bool, g: f64, go: bool| Sample {
        mem_percent: m,
        mem_ok: mo,
        cpu_percent: c,
        cpu_ok: co,
        gpu_percent: g,
        gpu_ok: go,
    };
    let samples = [
        Sample::default(),
        sm(99.0, false, 0.0, false, 0.0, false),
        sm(89.5, true, 0.0, false, 90.0, true),
        sm(90.0, true, 100.0, true, 0.0, false),
        sm(0.0, false, 50.0, true, 100.0, true),
        sm(2.5, true, 2.5, true, 0.0, false),
        sm(0.0, false, 0.0, false, 99.5, true),
        sm(0.5, true, 0.0, false, 0.0, false),
        sm(1e21, true, 0.0, false, 0.0, false),
        sm(-3.0, true, 3.5, true, 0.0, false),
    ];
    let prompts = prompt_messages();
    let environ: Vec<String> = cases.iter().find(|c| c["fn"] == "filteredEnv").map(|c| strs(&c["in"])).unwrap();
    for c in &cases {
        let (fname, input, want) = (c["fn"].as_str().unwrap(), &c["in"], &c["out"]);
        let got = match fname {
            "parse" => {
                let d = input.as_str().unwrap();
                match parse(d.as_bytes()) {
                    Ok(cfg) => cfg_out(&cfg, None),
                    Err(e) => cfg_out(&Config::default(), Some(&e)),
                }
            }
            "load" => {
                let v = input.as_str().unwrap();
                let home = format!("{top}/{v}");
                let p = format!("{home}/.sarina/tiers.json");
                std::fs::create_dir_all(format!("{home}/.sarina")).unwrap();
                let mut env: BTreeMap<String, String> = BTreeMap::new();
                use std::os::unix::fs::PermissionsExt;
                match v {
                    "valid" => std::fs::write(&p, r#"{"tiers":[{"name":"local"},{"name":"desktop","base_url":"http://10.0.0.5:8080/v1","model":"big","max_context":32768,"wake":"wakeonlan aa:bb"}]}"#).unwrap(),
                    "invalid" => std::fs::write(&p, r#"{"tiers":[{"kind":"x"}]}"#).unwrap(),
                    "dir" => std::fs::create_dir_all(&p).unwrap(),
                    "unreadable" => {
                        std::fs::write(&p, "{}").unwrap();
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o0)).unwrap();
                    }
                    "empty" => std::fs::write(&p, "").unwrap(),
                    "env" => {
                        let other = format!("{home}/other.json");
                        std::fs::write(&other, r#"{"tiers":[{"name":"a"},{"name":"b","base_url":"http://x/v1"}]}"#).unwrap();
                        env.insert("SARINA_TIERS_FILE".into(), format!(" {other} "));
                    }
                    "env-blank" => {
                        std::fs::write(&p, r#"{"tiers":[{"name":"home"}]}"#).unwrap();
                        env.insert("SARINA_TIERS_FILE".into(), "  ".into());
                    }
                    "env-missing" => {
                        std::fs::write(&p, r#"{"tiers":[{"name":"home"}]}"#).unwrap();
                        env.insert("SARINA_TIERS_FILE".into(), format!("{home}/nope.json"));
                    }
                    _ => {}
                }
                let getenv = |k: &str| env.get(k).cloned().unwrap_or_default();
                let (cfg, err) = load(&home, Some(&getenv));
                let mut out = cfg_out(&cfg, err.as_deref());
                out["err"] = json!(hide(out["err"].as_str().unwrap()));
                out["path"] = json!(hide(out["path"].as_str().unwrap()));
                json!([out, hide(&path(&home, Some(&getenv))), hide(&path(&home, None))])
            }
            "looksLikeKey" => json!(looks_like_key(&lossy(input))),
            "fromEscalation" => {
                let base = match input["base"].as_str().unwrap() {
                    "default" => default(),
                    "two" => Config {
                        tiers: vec![local_tier("http://l"), desktop_tier("http://d")],
                        resource: Thresholds { max_memory_percent: 80.0, ..Thresholds::default() },
                        warnings: Some(vec!["w0".into()]),
                        path: String::new(),
                    },
                    _ => Config { tiers: vec![local_tier("http://l"), cli_tier("/x/claude")], ..Config::default() },
                };
                let rt = &input["rt"];
                let esc = EscalationConfig {
                    enabled: input["enabled"].as_bool().unwrap(),
                    cli_path: input["cli_path"].as_str().unwrap().into(),
                    trigger_stop_reasons: strs(&input["triggers"]),
                    max_escalations: input["max"].as_i64().unwrap(),
                    timeout_seconds: input["timeout"].as_f64().unwrap(),
                    resource_thresholds: ResourceThresholds {
                        max_memory_percent: rt[0].as_f64(),
                        max_cpu_percent: rt[1].as_f64(),
                        max_gpu_memory_percent: rt[2].as_f64(),
                        min_disk_free_mb: None,
                    },
                    extra_cli_args: strs(&input["extra"]),
                };
                let o = EscalationOptions {
                    enabled: input["o_enabled"].as_bool().unwrap(),
                    cli_path: input["o_cli"].as_str().unwrap().into(),
                    triggers: strs(&input["o_triggers"]),
                    disable_resource_check: input["o_disable"].as_bool().unwrap(),
                    accept_edits: input["o_accept"].as_bool().unwrap(),
                    extra_dirs: opt_strs(&input["o_dirs"]),
                };
                let env = input["env"].clone();
                let getenv = |k: &str| env[k].as_str().unwrap_or("").to_string();
                let g: Option<&dyn Fn(&str) -> String> = if env.is_null() { None } else { Some(&getenv) };
                cfg_out(&from_escalation(&base, &esc, &o, g), None)
            }
            "redact" => {
                let (out, n) = redact_secrets(&lossy(&input[0]), knowns[input[1].as_u64().unwrap() as usize]);
                check.eq(format!("redact {input}"), &json!([out, n]), &json!([lossy(&want[0]), want[1]]));
                continue;
            }
            "capture" => json!(looks_like_capture(&lossy(input))),
            "sanitize" => {
                let (ri, ai) = (input[0].as_u64().unwrap() as usize, input[1].as_u64().unwrap() as usize);
                let req = &reqs[ri];
                match sanitize(req, &allows[ai], &known) {
                    Ok((out, n)) => json!([msgs_out(&out.messages), n, "", describe_guard(n)]),
                    Err(e) => json!([msgs_out(&req.messages), 0, e, ""]),
                }
            }
            "pressure" => {
                let (ti, si) = (input[0].as_u64().unwrap() as usize, input[1].as_u64().unwrap() as usize);
                json!(ths[ti].pressure(&samples[si]))
            }
            "nvidia" => {
                let r = parse_nvidia(input.as_str().unwrap());
                json!([float_v(r.unwrap_or(0.0)), r.is_some()])
            }
            "meminfo" => {
                let r = parse_meminfo(input.as_str().unwrap());
                json!([float_v(r.unwrap_or(0.0)), r.is_some()])
            }
            "prompt" => {
                let (i, reason) = (input[0].as_u64().unwrap() as usize, input[1].as_str().unwrap());
                json!(build_prompt(&prompts[i], reason))
            }
            "command" => {
                let t = Tier {
                    extra_dirs: opt_strs(&input[0]),
                    accept_edits: input[1].as_bool().unwrap(),
                    extra_args: opt_strs(&input[2]),
                    ..Tier::default()
                };
                json!(t.command("do it\nnow", "/w d"))
            }
            "childEnv" => {
                let c = CliClient {
                    tier: Tier::default(),
                    workdir: String::new(),
                    environ: Some(environ.clone()),
                    secrets: known.to_vec(),
                    last: Default::default(),
                };
                let out = c.child_env(&strs(input));
                if out.is_empty() {
                    Value::Null
                } else {
                    json!(out)
                }
            }
            "secretEnvName" => json!(secret_env_name(input.as_str().unwrap())),
            "filteredEnv" => {
                let out = filtered_env(&strs(input));
                if out.is_empty() {
                    Value::Null
                } else {
                    json!(out)
                }
            }
            "offMachine" => {
                let t = Tier { kind: input[0].as_str().unwrap().into(), base_url: input[1].as_str().unwrap().into(), ..Tier::default() };
                json!(off_machine(&t))
            }
            "estimateTokens" => json!(estimate_tokens(&reqs[input.as_u64().unwrap() as usize])),
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{fname} {input}"), &got, want);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("tiers");
}
