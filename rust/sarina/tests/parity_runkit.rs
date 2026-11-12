//! testdata/parity/runkit.json: the scrubber (with and without literals) and the transcript and run result as the service
//! and the GUI send them, from a session file's messages.

mod common;

use sarina::runkit::{result_json, scrubber, transcript};
use serde_json::{json, Value};

fn lits(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap().to_owned()).collect())
}

#[test]
fn runkit_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("runkit") {
        let i = &c["in"];
        let l = lits(&i["literals"]);
        let refs: Vec<&str> = l.iter().flatten().map(String::as_str).collect();
        let sc = l.as_ref().map(|_| scrubber(&refs));
        let scf = sc.as_ref().map(|f| &**f as &dyn Fn(&str) -> String);
        match c["fn"].as_str().unwrap() {
            "scrubber" => {
                let f = scrubber(&refs);
                let got: Vec<String> = i["texts"].as_array().unwrap().iter().map(|t| f(t.as_str().unwrap())).collect();
                check.eq(format!("scrubber {}", i["literals"]), &json!(got), &c["out"]);
            }
            "transcript" => {
                let s = sarina::session::parse(i["session"].as_str().unwrap().as_bytes()).unwrap();
                let got = sarina::gojson::marshal_any(&transcript(&s.messages, scf)).unwrap();
                check.eq(format!("transcript {}", i["literals"]), &common::text(got.as_bytes()), &c["out"]);
            }
            "result" => {
                let s = sarina::session::parse(i["session"].as_str().unwrap().as_bytes()).unwrap();
                let res = sarina::agent::run::RunResult {
                    final_output: "the answer, api_key=sk-1234567890ab".into(),
                    turns: 2,
                    tool_calls: 3,
                    transcript: s.messages[1..3].to_vec(),
                    usage: sarina::llm::types::Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_input_tokens: 2,
                        reasoning_tokens: 1,
                        ..Default::default()
                    },
                    total_cost_usd: 0.125,
                    stop_reason: i["stop"].as_str().unwrap().into(),
                    session_id: "sid".into(),
                    ..Default::default()
                };
                let got = result_json(&res, scf).unwrap();
                check.eq(format!("result {} {}", i["stop"], i["literals"]), &common::text(got.as_bytes()), &c["out"]);
            }
            f => panic!("unknown case {f}"),
        }
    }
    check.done("runkit");
}
