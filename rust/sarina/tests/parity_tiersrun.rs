//! testdata/parity/tiersrun.json: the parts of compute tiers that run something: the claude CLI hand-off against a
//! stand-in in bash's builtins, the wake runner, and the Router's whole life in scenarios against scripted model servers
//! and a scripted Heimdall. Never the real model.
//!
//! One test function: it sets PATH and the Heimdall variable, which the whole process shares.

mod common;

use sarina::config::{ModelConfig, Permissions};
use sarina::goany::Any;
use sarina::goctx::Ctx;
use sarina::gojson;
use sarina::gotext::float_v;
use sarina::gotime::Time;
use sarina::heimdall::{load_grants_at, Grants};
use sarina::llm::client::Error;
use sarina::llm::openai::{Request, StreamEvent};
use sarina::llm::types::Message;
use sarina::tiers::cli::CliClient;
use sarina::tiers::config::{parse, Tier, KIND_CLAUDE_CLI};
use sarina::tiers::resource::{Sample, Sampler};
use sarina::tiers::router::{Options, Router};
use sarina::tiers::wake::new_wake;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

const LOCAL_KEY: &str = "local-key-0123456789";
const CLAUDE_KEY: &str = "sk-ant-api03-TESTKEY-do-not-leak-0123456789";

const CLAUDE_STAND_IN: &str = r#"#!BASH
{ printf '[%s]\n' "$@"; export -p; } > .standin-call
case "$STANDIN_MODE" in
  ok) printf '{"type":"result","is_error":false,"result":"done by claude","session_id":"cli-1","total_cost_usd":0.25}'; printf 'a warning\n' >&2;;
  is_error) printf '{"is_error":true,"result":"nope","session_id":"cli-2"}';;
  is_error_num) printf '{"is_error":1,"result":"x"}';;
  not_error) printf '{"is_error":0,"result":"fine","total_cost_usd":"x","session_id":5}';;
  is_error_str) printf '{"is_error":"","result":"empty string is false"}';;
  badjson) printf 'not json at all';;
  null) printf 'null';;
  array) printf '[1]';;
  trailing) printf '{"result":"x"} {}';;
  exit1) printf '{"result":"x"}'; exit 1;;
  empty) ;;
  secret) printf '{"result":"key local-key-0123456789 here","session_id":"s"}'; printf 'token ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa and local-key-0123456789\n' >&2;;
  bigerr) printf '%05000d' 7 >&2; printf '{"result":"after noise"}';;
  bigout) printf '{"result":"'; printf '%03000d' 1; printf '"}';;
  huge) printf '{"result":"'; printf '%016777216d' 0; printf '"}';;
  sleep) read -t 5 <> <(:);;
  tree) ( read -t 5 <> <(:) ) & read -t 5 <> <(:);;
  bgholds) ( read -t 4 <> <(:) ) & printf '{"result":"quick"}';;
esac
"#;

fn strs(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
}

fn http_ans(status: &str, body: &str) -> Vec<common::Step> {
    vec![common::Step::Send(
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            .into_bytes(),
    )]
}

fn dead_address() -> String {
    let ln = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    ln.local_addr().unwrap().to_string()
}

fn marshal(m: &std::collections::BTreeMap<String, Any>) -> String {
    gojson::marshal_any(&Any::Object(m.clone())).unwrap()
}

struct FixedSampler(Sample);

impl Sampler for FixedSampler {
    fn sample(&self, _: &Ctx) -> Sample {
        self.0
    }
}

fn prefix(s: &str, n: usize) -> String {
    String::from_utf8_lossy(&s.as_bytes()[..s.len().min(n)]).into_owned()
}

#[test]
fn tiers_run_matches_go() {
    let cases = common::cases("tiersrun");
    let mut check = common::Check::new();
    let path_env = std::env::var("PATH").unwrap_or_default();
    let bash = path_env.split(':').map(|d| format!("{d}/bash")).find(|p| std::path::Path::new(p).exists()).expect("bash");
    let bash = String::from_utf8(sarina::gopath::eval_symlinks(bash.as_bytes()).unwrap()).unwrap();
    let tmp = std::env::temp_dir().join(format!("sarina-parity-tiersrun-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let top = String::from_utf8(sarina::gopath::eval_symlinks(tmp.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let bin = format!("{top}/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cli = format!("{bin}/claude");
    std::fs::write(&cli, CLAUDE_STAND_IN.replacen("BASH", &bash, 1)).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&bash, format!("{bin}/sh")).unwrap();
    std::env::set_var("PATH", &bin);
    std::env::set_var("SAFE_VAR", "visible");
    std::env::set_var("MY_TOKEN", "hidden");
    let base_hide = |s: &str| s.replace(&bash, "$BASH").replace(&top, "$BASE");
    let cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
    let environ: Vec<String> = [
        format!("PATH={bin}"),
        "HOME=/h".into(),
        "OPENAI_API_KEY=x".into(),
        format!("ANTHROPIC_TEST_KEY={CLAUDE_KEY}"),
        "GITHUB_TOKEN=ghp_x".into(),
        "MY_SECRET_THING=hush".into(),
        "SAFE_VAR=visible".into(),
        format!("WRAPPED_VALUE=prefix-{LOCAL_KEY}-suffix"),
        "CUSTOM_KEYNAME_VAR=1".into(),
        "LANG=C".into(),
    ]
    .to_vec();
    let msg = |r: &str, c: &str| Message { role: r.into(), content: c.into(), ..Message::default() };
    let msgs = vec![
        msg("system", "be brief"),
        msg("user", "refactor the parser"),
        msg("assistant", "working on it"),
        msg("tool", "secret local-key-0123456789 in output"),
    ];
    let wake_srv = common::raw_seq(vec![http_ans("200 OK", ""), http_ans("404 Not Found", ""), vec![]], None);
    let wake_dead = dead_address();
    let mut cli_n = 0;
    for c in &cases {
        let (fname, input, want) = (c["fn"].as_str().unwrap(), &c["in"], &c["out"]);
        match fname {
            "cli" => {
                let i = cli_n;
                cli_n += 1;
                let mode = input[0].as_str().unwrap();
                let mut work = format!("{top}/cli/{i}");
                std::fs::create_dir_all(&work).unwrap();
                if !input[2].as_str().unwrap().is_empty() {
                    work = input[2].as_str().unwrap().to_string();
                }
                let path = input[1].as_str().unwrap();
                let tier = Tier {
                    kind: KIND_CLAUDE_CLI.into(),
                    cli_path: if path.is_empty() { cli.clone() } else { path.to_string() },
                    timeout_seconds: input[4].as_f64().unwrap(),
                    extra_dirs: strs(&input[7]),
                    accept_edits: input[8].as_bool().unwrap(),
                    extra_args: strs(&input[9]),
                    api_key_env: input[10].as_str().unwrap().into(),
                    ..Tier::default()
                };
                let mut env = environ.clone();
                env.push(format!("STANDIN_MODE={mode}"));
                let mut c = CliClient {
                    tier,
                    workdir: work.clone(),
                    environ: Some(env),
                    secrets: vec![LOCAL_KEY.into(), CLAUDE_KEY.into(), "short".into()],
                    last: Default::default(),
                };
                let ctx = Ctx::background();
                let after = input[5].as_u64().unwrap();
                if after > 0 {
                    let c2 = ctx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(after));
                        c2.cancel();
                    });
                }
                let reason = input[3].as_str().unwrap();
                let mut events: Vec<Value> = Vec::new();
                let mut on = |e: StreamEvent| events.push(json!([e.kind, e.delta, e.finish_reason]));
                let stream = input[6].as_bool().unwrap();
                let start = Instant::now();
                let r = c.complete(
                    &ctx,
                    &Request { messages: msgs.clone(), ..Request::default() },
                    (!reason.is_empty()).then_some(reason),
                    if stream { Some(&mut on) } else { None },
                );
                let took = start.elapsed();
                ctx.cancel();
                let (tout, err) = match &r {
                    Ok(t) => (json!([t.content, t.finish_reason, t.has_finish_reason, marshal(t.raw.as_ref().unwrap())]), String::new()),
                    Err(e) => (Value::Null, e.clone()),
                };
                let call = std::fs::read(format!("{work}/.standin-call")).unwrap_or_default();
                let _ = std::fs::remove_file(format!("{work}/.standin-call"));
                let l = &c.last;
                let got = json!([
                    tout,
                    base_hide(&err),
                    r.is_err(),
                    [
                        l.ok,
                        base_hide(&l.err),
                        l.session_id,
                        float_v(l.cost_usd),
                        l.stdout.len(),
                        prefix(&l.stdout, 200),
                        prefix(&l.stderr, 200),
                        l.stderr.len()
                    ],
                    if events.is_empty() { Value::Null } else { Value::Array(events) },
                    base_hide(&String::from_utf8_lossy(&call)),
                    took < Duration::from_secs(4),
                    took < Duration::from_millis(1800)
                ]);
                check.eq(format!("cli {i} {input}"), &got, want);
            }
            "wake" => {
                let perms = Permissions {
                    allow_shell_commands: input[0].as_bool().unwrap(),
                    allow_destructive_shell_commands: input[1].as_bool().unwrap(),
                    ..Permissions::default()
                };
                let w = input[2].as_str().unwrap().replace("$W", &wake_srv.addr).replace("$DEAD", &wake_dead).replace("$BASE", &top);
                let run = new_wake(perms, top.clone());
                let r = run(&Ctx::background(), &Tier { name: "desk".into(), wake: w, ..Tier::default() });
                let out = String::from_utf8_lossy(&std::fs::read(format!("{top}/wake.out")).unwrap_or_default()).into_owned();
                let hide_w = |s: &str| base_hide(&s.replace(&wake_srv.addr, "$W").replace(&wake_dead, "$DEAD").replace(&cwd, "$CWD"));
                let got = json!([hide_w(&r.err().unwrap_or_default()), base_hide(&out).replace(&cwd, "$CWD")]);
                check.eq(format!("wake {input}"), &got, want);
            }
            "wakeRequests" => {
                let reqs: Vec<Value> = common::settle(&wake_srv.seen).iter().map(|r| json!([r["method"], r["uri"]])).collect();
                check.eq("wake requests", &Value::Array(reqs), want);
            }
            "router" => {
                let got = run_router(input, &top, &cli, &bash);
                let name = input["name"].as_str().unwrap();
                // compare piece by piece so a difference names its place
                for (k, part) in ["log", "served", "heimdall", "calls"].iter().enumerate() {
                    if part == &"log" {
                        let (g, w) = (got[0].as_array().cloned().unwrap_or_default(), want[0].as_array().cloned().unwrap_or_default());
                        for j in 0..g.len().max(w.len()) {
                            check.eq(format!("router {name} log {j}"), g.get(j).unwrap_or(&Value::Null), w.get(j).unwrap_or(&Value::Null));
                        }
                    } else {
                        check.eq(format!("router {name} {part}"), &got[k], &want[k]);
                    }
                }
            }
            other => panic!("unknown fn {other}"),
        }
    }
    drop(wake_srv);
    let _ = std::fs::remove_dir_all(&tmp);
    check.done("tiersrun");
}

fn run_router(sc: &Value, top: &str, cli: &str, bash: &str) -> Value {
    let name = sc["name"].as_str().unwrap();
    let servers: Vec<common::SeqServer> =
        sc["servers"].as_array().map(|a| a.iter().map(|s| common::raw_seq(common::steps(s), None)).collect()).unwrap_or_default();
    let addrs: Vec<String> = servers.iter().map(|s| s.addr.clone()).collect();
    let dead = dead_address();
    let hsrv = common::raw_seq(common::steps(&sc["heimdall"]), None);
    std::env::set_var("HEIMDALL_REVIEW_URL", format!("http://{}/v1/review", hsrv.addr));
    let mut reps: Vec<(String, String)> = addrs.iter().enumerate().map(|(i, a)| (a.clone(), format!("$A{i}"))).collect();
    reps.extend([
        (dead.clone(), "$DEAD".into()),
        (hsrv.addr.clone(), "$H".into()),
        (bash.into(), "$BASH".into()),
        (top.into(), "$BASE".into()),
    ]);
    let reps = Rc::new(reps);
    let hide = move |s: &str| reps.iter().fold(s.to_string(), |s, (a, b)| s.replace(a.as_str(), b));
    let mut doc = sc["doc"].as_str().unwrap().to_string();
    for (i, a) in addrs.iter().enumerate() {
        doc = doc.replace(&format!("$S{i}"), &format!("http://{a}/v1")).replace(&format!("$A{i}"), &format!("http://{a}"));
    }
    doc = doc.replace("$DEAD", &format!("http://{dead}/v1")).replace("$CLI", cli);
    let cfg = parse(doc.as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let grants_doc = sc["grants"].as_str().unwrap().to_string();
    let grants: Grants = if grants_doc.is_empty() {
        Grants::default()
    } else {
        let dir = format!("{top}/grants-{name}");
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = format!("{dir}/grants.json");
        std::fs::write(&p, &grants_doc).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        // SAFETY: getuid(2) has no preconditions.
        let uid = unsafe { libc::getuid() };
        load_grants_at(p.as_bytes(), uid)
    };
    let log: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    let work = format!("{top}/router-{name}");
    std::fs::create_dir_all(&work).unwrap();
    let smp = &sc["sample"];
    let sample = Sample {
        mem_percent: smp[0].as_f64().unwrap(),
        mem_ok: smp[1].as_bool().unwrap(),
        cpu_percent: smp[2].as_f64().unwrap(),
        cpu_ok: smp[3].as_bool().unwrap(),
        gpu_percent: smp[4].as_f64().unwrap(),
        gpu_ok: smp[5].as_bool().unwrap(),
    };
    let clock = Rc::new(Cell::new(Time::parse_rfc3339("2026-10-05T09:00:00Z").unwrap().unix()));
    let clock2 = clock.clone();
    let mode = sc["mode"].as_str().unwrap().to_string();
    let mut base = ModelConfig {
        model: "local-model".into(),
        base_url: cfg.tiers[0].base_url.clone(),
        api_key: LOCAL_KEY.into(),
        timeout_seconds: 5.0,
        ..ModelConfig::default()
    };
    if sc["timeout"].as_f64().unwrap() > 0.0 {
        base.timeout_seconds = sc["timeout"].as_f64().unwrap();
    }
    let mut opt = Options {
        config: cfg,
        base,
        getenv: Box::new(|k| if k == "ANTHROPIC_TEST_KEY" { CLAUDE_KEY.into() } else { String::new() }),
        sampler: Some(Box::new(FixedSampler(sample))),
        now: Some(Box::new(move || {
            let c = clock2.get();
            clock2.set(c + 1);
            Time::from_unix(c, 0)
        })),
        health_timeout: Duration::from_secs(1),
        wake_poll: Duration::from_millis(5),
        workdir: work.clone(),
        environ: Some(vec![
            "PATH=/nowhere".into(),
            format!("STANDIN_MODE={mode}"),
            format!("OPENAI_API_KEY={LOCAL_KEY}"),
            format!("ANTHROPIC_TEST_KEY={CLAUDE_KEY}"),
            "LANG=C".into(),
        ]),
        grants: Some(Box::new(move || grants.clone())),
        ..Options::default()
    };
    if let Some(answers) = sc["confirm"].as_array() {
        let answers: RefCell<Vec<bool>> = RefCell::new(answers.iter().map(|a| a.as_bool().unwrap()).collect());
        let log = log.clone();
        opt.confirm = Some(Box::new(move |p: &str| {
            log.borrow_mut().push(json!(["confirm", p]));
            let mut a = answers.borrow_mut();
            if a.is_empty() {
                false
            } else {
                a.remove(0)
            }
        }));
    }
    if !sc["nowake"].as_bool().unwrap() {
        let answers: RefCell<Vec<String>> = RefCell::new(strs(&sc["wake"]).unwrap_or_default());
        let log = log.clone();
        opt.wake = Some(Box::new(move |_: &Ctx, t: &Tier| {
            log.borrow_mut().push(json!(["wake", t.name, t.wake]));
            let mut a = answers.borrow_mut();
            if a.is_empty() {
                return Ok(());
            }
            let first = a.remove(0);
            if first == "ok" {
                Ok(())
            } else {
                Err(first)
            }
        }));
    }
    let r = Router::new(opt).unwrap();
    let mut prev: Option<Any> = None;
    for op in sc["ops"].as_array().unwrap() {
        let op = op.as_str().unwrap();
        let (opname, arg) = op.split_once(':').unwrap_or((op, ""));
        match opname {
            "begin" => {
                let h = hide.clone();
                let l = log.clone();
                r.begin_turn(Some(Box::new(move |e| l.borrow_mut().push(json!(["event", h(&marshal(&e))])))));
            }
            "complete" | "stream" | "cancelled" => {
                let ctx = Ctx::background();
                if opname == "cancelled" {
                    ctx.cancel();
                }
                let mut req = Request {
                    messages: vec![
                        Message { role: "system".into(), content: "be brief".into(), ..Message::default() },
                        Message { role: "user".into(), content: arg.into(), ..Message::default() },
                    ],
                    ..Request::default()
                };
                let l2 = log.clone();
                let mut on = move |e: StreamEvent| l2.borrow_mut().push(json!(["stream", e.kind, e.delta, e.finish_reason]));
                let res = if opname == "stream" {
                    req.stream = true;
                    r.complete(&ctx, &req, Some(&mut on))
                } else {
                    r.complete(&ctx, &req, None)
                };
                let (tout, err, kind, status) = match &res {
                    Ok(t) => (json!([t.content, t.finish_reason, t.model]), String::new(), "", 0),
                    Err(Error::Backend(b)) => (Value::Null, b.message.clone(), "backend", b.status),
                    Err(Error::Cli(s)) => (Value::Null, s.clone(), "cli", 0),
                    Err(Error::Ctx(s)) => (Value::Null, s.to_string(), "other", 0),
                };
                log.borrow_mut().push(json!([prefix(op, 40), tout, hide(&err), kind, status]));
            }
            "advance" => clock.set(clock.get() + arg.parse::<i64>().unwrap()),
            "up" => {
                let (n, e) = match r.request_up() {
                    Ok(n) => (n, String::new()),
                    Err(e) => (String::new(), e),
                };
                log.borrow_mut().push(json!(["up", n, e]));
            }
            "down" => log.borrow_mut().push(json!(["down", r.request_down()])),
            "current" => log.borrow_mut().push(json!(["current", r.current()])),
            "pressure" => {
                let (s, p) = r.pressure(&Ctx::background());
                let th = r.thresholds();
                log.borrow_mut().push(json!([
                    "pressure",
                    float_v(s.mem_percent),
                    s.mem_ok,
                    p,
                    float_v(th.max_memory_percent),
                    float_v(th.max_cpu_percent)
                ]));
            }
            "record" | "record0" | "recordprev" => {
                let p = match opname {
                    "record" => prev.clone(),
                    "recordprev" => Some(gojson::unmarshal_any(arg.as_bytes()).0),
                    _ => None,
                };
                let rec = r.record(p.as_ref());
                let b = match &rec {
                    Some(m) => marshal(m),
                    None => "null".into(),
                };
                if rec.is_some() {
                    prev = Some(gojson::unmarshal_any(b.as_bytes()).0);
                }
                log.borrow_mut().push(json!([opname, hide(&b)]));
            }
            "status" => {
                for s in r.status(&Ctx::background()) {
                    log.borrow_mut().push(json!([
                        "status",
                        s.name,
                        s.kind,
                        hide(&s.base_url),
                        s.model,
                        s.api_key_env,
                        s.key_set,
                        s.allow_data,
                        s.max_context,
                        s.wake,
                        s.enabled,
                        s.auto,
                        s.healthy,
                        hide(&s.why),
                        s.current,
                        hide(&s.cli_path),
                        s.cli_found
                    ]));
                }
            }
            other => panic!("unknown op {other}"),
        }
    }
    let served: Vec<Value> = servers
        .iter()
        .map(|s| {
            let reqs: Vec<Value> = common::settle(&s.seen)
                .iter()
                .map(|q| {
                    // Go's fmt.Sprint of a missing value is "<nil>" (a connection whose request could not be read)
                    let f = |k: &str| match &q[k] {
                        Value::Null => "<nil>".to_string(),
                        v => String::from_utf8_lossy(&common::bytes(v)).into_owned(),
                    };
                    json!([q["method"], q["uri"], hide(&f("auth")), hide(&f("apiKey")), hide(&f("body"))])
                })
                .collect();
            if reqs.is_empty() {
                Value::Null
            } else {
                Value::Array(reqs)
            }
        })
        .collect();
    let hreqs: Vec<Value> = common::settle(&hsrv.seen).iter().map(|q| json!([q["method"], q["uri"], q["body"]])).collect();
    let calls = std::fs::read(format!("{work}/.standin-call"))
        .ok()
        .map(|b| Value::from(hide(&String::from_utf8_lossy(&b)).split('\n').map(|s| s.to_string()).collect::<Vec<_>>()));
    let log = log.borrow().clone();
    json!([
        if log.is_empty() { Value::Null } else { Value::Array(log) },
        if served.is_empty() { Value::Null } else { Value::Array(served) },
        if hreqs.is_empty() { Value::Null } else { Value::Array(hreqs) },
        calls.unwrap_or(Value::Null)
    ])
}
