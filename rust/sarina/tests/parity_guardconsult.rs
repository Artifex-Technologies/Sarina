//! testdata/parity/guardconsult.json: the gate's full scan with Heimdall's second opinion against scripted answers, the
//! local checks deciding first, and the default gate, which never asks.

mod common;

use sarina::guard::Gate;
use sarina::heimdall::ENV_VAR;
use serde_json::{json, Value};
use std::time::Duration;

#[test]
fn guard_consult_matches_go() {
    let mut check = common::Check::new();
    let cases = common::cases("guardconsult");
    let inputs = &cases.iter().find(|c| c["fn"] == "inputs").unwrap()["in"];
    let replies: Vec<Vec<u8>> = inputs["replies"].as_array().unwrap().iter().map(common::bytes).collect();
    let texts: Vec<Vec<u8>> = inputs["texts"].as_array().unwrap().iter().map(common::bytes).collect();
    let gates = [
        Gate::memory(),
        Gate { heimdall_kind: "skill_text".into(), session_id: "s-9".into(), ..Gate::default() },
        Gate::default_gate(),
        Gate::default(),
    ];
    for c in &cases {
        let got = match c["fn"].as_str().unwrap() {
            "inputs" => continue,
            "absent" => json!([Gate::memory().scan(&texts[0], &|_| String::new()).blocked]),
            "scan" => {
                let i: Vec<usize> = c["in"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as usize).collect();
                let (g, text) = (&gates[i[2]], &texts[i[1]]);
                let asks = !Gate { allow_leading_bom: g.allow_leading_bom, ..Gate::default() }.scan_local(text).blocked
                    && !g.heimdall_kind.is_empty();
                let (addr, got, _release) = common::raw_server(replies[i[0]].clone(), Duration::ZERO, false);
                let u = format!("http://{addr}/v1/review");
                let v = g.scan(text, &|k| if k == ENV_VAR { u.clone() } else { String::new() });
                let req = if asks { got.recv_timeout(Duration::from_secs(5)).unwrap_or(Value::Null) } else { Value::Null };
                json!([v.blocked, common::text(v.reason.as_bytes()), req])
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {}", c["fn"], c["in"]), &got, &c["out"]);
    }
    check.done("guardconsult");
}
