//! testdata/parity/discover.json: the context-window discovery: serverRoot on base URLs of every shape, sameModel, and
//! ContextWindow against scripted servers (the same answers as Go's test): the probes in order, what each answer gives,
//! the 4 MiB bound, a transport failure ending the round, the cache and its TTL, a cancelled caller not cached.

mod common;

use sarina::config::ModelConfig;
use sarina::goctx::Ctx;
use sarina::llm::discover::{same_model, server_root, Discoverer};
use sarina::llm::discover::{DEFAULT_DISCOVERY_TIMEOUT, DEFAULT_DISCOVERY_TTL, MAX_PLAUSIBLE_WINDOW, MIN_PLAUSIBLE_WINDOW};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn discover_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("discover") {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "root" => common::text(server_root(&String::from_utf8(common::bytes(input)).unwrap()).as_bytes()),
            "same" => json!(same_model(input[0].as_str().unwrap(), input[1].as_str().unwrap())),
            "defaults" => json!([
                DEFAULT_DISCOVERY_TIMEOUT.as_nanos() as u64,
                DEFAULT_DISCOVERY_TTL.as_nanos() as u64,
                MIN_PLAUSIBLE_WINDOW,
                MAX_PLAUSIBLE_WINDOW
            ]),
            "window" => {
                let srv = common::raw_seq(common::steps(&input["script"]), None);
                let start = Instant::now();
                let now = Arc::new(Mutex::new(start));
                let n2 = now.clone();
                let mut d = Discoverer::default();
                d.timeout = Duration::from_millis(input["timeoutMs"].as_u64().unwrap());
                d.now = Some(Box::new(move || *n2.lock().unwrap()));
                let cfg = ModelConfig {
                    model: input["model"].as_str().unwrap().into(),
                    base_url: format!("http://{}{}", srv.addr, input["path"].as_str().unwrap()),
                    api_key: "k".into(),
                    ..ModelConfig::default()
                };
                let mut results = Vec::new();
                for (i, m) in input["calls"].as_array().unwrap().iter().enumerate() {
                    *now.lock().unwrap() = start + Duration::from_secs(60 * m.as_u64().unwrap());
                    let ctx = Ctx::background();
                    let cancel_ms = input["cancelMs"].as_u64().unwrap();
                    if i == 0 && cancel_ms > 0 {
                        let c2 = ctx.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(cancel_ms));
                            c2.cancel();
                        });
                    }
                    let (n, src) = d.context_window(&ctx, &cfg);
                    std::thread::sleep(Duration::from_millis(50));
                    results.push(json!([n, src, srv.seen.lock().unwrap().len()]));
                }
                let seen = srv.seen.lock().unwrap().clone();
                json!({"results": results, "requests": if seen.is_empty() { Value::Null } else { Value::Array(seen) }})
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {}", c["fn"], input.get("name").unwrap_or(input)), &got, &c["out"]);
    }
    check.done("discover");
}
