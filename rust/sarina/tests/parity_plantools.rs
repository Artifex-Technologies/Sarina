//! testdata/parity/plantools.json: enter_plan_mode and exit_plan_mode through the registry with plan mode unknown, on and
//! off, and plans at, under and over the limit.

mod common;

use sarina::goany::Any;
use sarina::gojson;
use sarina::tools::plan::plan_tools;
use sarina::tools::registry::{Context, Registry};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[test]
fn plan_tools_match_go() {
    let cases = common::cases("plantools");
    let mut check = common::Check::new();
    let plans = cases.iter().find(|c| c["fn"] == "plans").unwrap()["in"].as_array().unwrap().clone();
    let reg = Registry::new(plan_tools());
    for c in cases.iter().filter(|c| c["fn"] == "mode") {
        let mode = c["in"].as_str().unwrap();
        let tc = Context {
            plan_active: (mode != "unknown").then(|| {
                let on = mode == "on";
                Arc::new(move || on) as sarina::tools::plan::PlanActive
            }),
            ..Context::default()
        };
        let row = |r: sarina::tools::registry::ToolResult| json!([r.ok, r.content, gojson::marshal_any(&Any::Object(r.metadata)).unwrap()]);
        let mut out = vec![row(reg.execute("enter_plan_mode", Some(BTreeMap::from([("x".into(), Any::Number(1.0))])), &tc))];
        for p in &plans {
            let args = match p {
                Value::Null => BTreeMap::new(),
                p => BTreeMap::from([("plan".to_owned(), gojson::unmarshal_any(p.to_string().as_bytes()).0)]),
            };
            out.push(row(reg.execute("exit_plan_mode", Some(args), &tc)));
        }
        check.eq(format!("plan mode {mode}"), &Value::Array(out), &c["out"]);
    }
    check.done("plantools");
}
