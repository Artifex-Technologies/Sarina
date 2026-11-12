//! testdata/parity/registry.json: the permission gate and the registry: what a Builder offers for every allowlist,
//! narrowing, plan mode, permission set and deny rule, and what Execute answers for every tool, permission set, deny
//! rule, approver and argument.

mod common;

use sarina::config::Permissions;
use sarina::goany::Any;
use sarina::gojson;
use sarina::gotext::lossy;
use sarina::llm::types::ToolSpec;
use sarina::tools::registry::{Builder, Class, Context, Deny, Output, Permission, Registry, Tool, ToolError};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

fn untyped(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Array(a) => Any::Array(a.iter().map(untyped).collect()),
        Value::Object(m) if m.contains_key("f") => Any::Number(f64::from_bits(u64::from_str_radix(m["f"].as_str().unwrap(), 16).unwrap())),
        Value::Object(m) if m.contains_key("s") => Any::String(lossy(&common::bytes(&m["s"]))),
        Value::Object(m) => Any::Object(m["o"].as_object().unwrap().iter().map(|(k, x)| (k.clone(), untyped(x))).collect()),
        other => panic!("{other}"),
    }
}

/// The Go test's made-up tools: what a call does depends on its "mode" argument.
fn parity_tools() -> Vec<Tool> {
    let schema = gojson::unmarshal_any(br#"{"type":"object","properties":{"n":{"type":"integer"},"mode":{"type":"string"}}}"#)
        .0
        .into_object()
        .unwrap();
    let handler = Arc::new(|args: &BTreeMap<String, Any>, _: &Context| {
        let mode = match args.get("mode") {
            Some(Any::String(s)) => s.as_str(),
            _ => "",
        };
        let fail = |e: ToolError| Output { error: Some(e), ..Output::default() };
        match mode {
            "err" => Output {
                metadata: BTreeMap::from([("k".to_owned(), Any::String("v".into()))]),
                error: Some(ToolError::Failed("it broke <here>".into())),
                ..Output::default()
            },
            "mderr" => Output {
                content: "lost".into(),
                metadata: BTreeMap::from([("error_kind".to_owned(), Any::String("mine".into())), ("y".to_owned(), Any::Bool(true))]),
                error: Some(ToolError::Failed("failed after all".into())),
            },
            "denied" => fail(ToolError::Denied("not for you".into())),
            "wrapped" => fail(ToolError::Denied("wrapped: deep".into())),
            "panic" => panic!("boom"),
            "md" => Output {
                content: "with metadata".into(),
                metadata: BTreeMap::from([
                    ("error_kind".to_owned(), Any::String("mine".into())),
                    ("z".to_owned(), Any::Number(1.5)),
                    ("streamed".to_owned(), Any::Bool(false)),
                ]),
                error: None,
            },
            _ => Output { content: format!("ran {}", gojson::marshal_any(&Any::Object(args.clone())).unwrap()), ..Output::default() },
        }
    });
    let mk = |name: &str, requires, class, params: bool| Tool {
        spec: ToolSpec { name: name.into(), description: format!("the {name} tool"), parameters: params.then(|| schema.clone()) },
        handler: handler.clone(),
        requires,
        class,
        stream: None,
    };
    vec![
        mk("r", Permission::None, Class::ReadOnly, true),
        mk("w", Permission::Write, Class::Mutating, true),
        mk("s", Permission::Shell, Class::Mutating, true),
        mk("u", Permission::None, Class::Unset, false),
        mk("m", Permission::None, Class::Mutating, true),
        mk("enter_plan_mode", Permission::None, Class::PlanControl, false),
        mk("exit_plan_mode", Permission::None, Class::PlanControl, false),
        mk("ws", Permission::Write, Class::ReadOnly, true),
    ]
}

fn deny_w() -> Deny {
    Arc::new(|name: &str| if name.starts_with('w') { format!("the operator's policy forbids {name}") } else { String::new() })
}

fn strs(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect())
}

#[test]
fn registry_matches_go() {
    // the "panic" calls panic on purpose (Execute turns that into a result): keep the default hook from printing each
    std::panic::set_hook(Box::new(|_| {}));
    let mut check = common::Check::new();
    let perm_sets = [
        Permissions::default(),
        Permissions { allow_file_write: true, ..Permissions::default() },
        Permissions { allow_shell_commands: true, ..Permissions::default() },
        Permissions { allow_file_write: true, allow_shell_commands: true, ..Permissions::default() },
    ];
    for c in common::cases("registry") {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "Build" => {
                let a = input.as_array().unwrap();
                let perms = match a[3].as_u64().unwrap() {
                    0 => None,
                    1 => Some(perm_sets[0].clone()),
                    2 => Some(perm_sets[1].clone()),
                    _ => Some(perm_sets[3].clone()),
                };
                let b = Builder {
                    allowlist: strs(&a[0]),
                    narrow: strs(&a[1]),
                    plan_mode: a[2].as_bool().unwrap(),
                    perms,
                    extra: parity_tools(),
                    deny: a[4].as_bool().unwrap().then(deny_w),
                    ..Builder::default()
                };
                let reg = b.build();
                let names = reg.names();
                let specs: Vec<String> = reg.specs().into_iter().map(|s| s.name).collect();
                json!([names, reg.len(), if specs.is_empty() { Value::Null } else { json!(specs) }, reg.get("w").is_some()])
            }
            "Execute" => {
                let a = input.as_array().unwrap();
                let perms = match a[1].as_str().unwrap() {
                    "--" => perm_sets[0].clone(),
                    "w-" => perm_sets[1].clone(),
                    "-s" => perm_sets[2].clone(),
                    _ => perm_sets[3].clone(),
                };
                let mut reg = Registry::new(parity_tools());
                if a[2].as_bool().unwrap() {
                    reg.set_deny(Some(deny_w()));
                }
                let asked: Arc<Mutex<Value>> = Arc::new(Mutex::new(Value::Null));
                let mode = a[3].as_u64().unwrap();
                let rec = asked.clone();
                let tc = Context {
                    permissions: perms,
                    approver: (mode > 0).then(|| {
                        Arc::new(move |req: &sarina::tools::registry::ApprovalRequest| {
                            *rec.lock().unwrap() =
                                json!([req.tool, gojson::marshal_any(&Any::Object(req.arguments.clone())).unwrap(), req.needs.as_str()]);
                            match mode {
                                2 => (false, String::new()),
                                3 => (false, "not now".to_owned()),
                                _ => (true, String::new()),
                            }
                        }) as sarina::tools::registry::Approver
                    }),
                    ..Context::default()
                };
                let args = untyped(&a[4]).into_object().unwrap();
                let r = reg.execute(a[0].as_str().unwrap(), Some(args), &tc);
                let md = gojson::marshal_any(&Any::Object(r.metadata.clone())).unwrap();
                let asked = asked.lock().unwrap().clone();
                json!([[r.name, r.ok, r.content, md], asked])
            }
            "Permission" => json!(match input.as_i64().unwrap() {
                1 => Permission::Write.as_str(),
                2 => Permission::Shell.as_str(),
                _ => Permission::None.as_str(),
            }),
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    let _ = std::panic::take_hook();
    check.done("registry");
}
