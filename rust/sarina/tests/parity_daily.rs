//! testdata/parity/daily.json: the daily notes as `sarina trends` reads them (go/daily/daily_parity_test.go): Parse and
//! Render, ParseNumbers and Line, ReadDays on a Daily folder made from the fixture's list, Trends.

mod common;
use common::Check;
use sarina::daily::{self, Numbers};
use serde_json::{json, Value};

fn numbers(n: &Numbers) -> Value {
    let disks: Vec<Value> = n.disks.iter().map(|(k, v)| json!([k, v])).collect();
    // Go writes a whole float64 without a fraction
    let load = if n.load.fract() == 0.0 && n.load.abs() < 1e15 { json!(n.load as i64) } else { json!(n.load) };
    json!([n.at, if disks.is_empty() { Value::Null } else { json!(disks) }, n.crashes, n.top, n.top_n, n.failed, n.mem_pct, load])
}

fn list(v: &[String]) -> Value {
    if v.is_empty() {
        Value::Null
    } else {
        json!(v)
    }
}

#[test]
fn daily_matches_go() {
    let mut d = Check::new();
    let base = std::env::temp_dir().join(format!("parity-daily-{}", std::process::id()));
    for c in common::cases("daily") {
        let rows = c["out"].as_array().cloned().unwrap_or_default();
        match c["fn"].as_str().unwrap() {
            "numbers" => {
                for r in &rows {
                    let l = r[0].as_str().unwrap();
                    let got = match daily::parse_numbers(l) {
                        Some(n) => json!([l, true, numbers(&n), n.line()]),
                        None => json!([l, false, numbers(&Numbers::default()), ""]),
                    };
                    d.eq(format!("numbers {l}"), &got, r);
                }
            }
            "parse" => {
                for r in &rows {
                    let n = daily::parse("2026-11-16", r[0].as_str().unwrap());
                    let got = json!([r[0], list(&n.head), n.status_at, list(&n.status), list(&n.numbers), n.render()]);
                    d.eq(format!("parse {:.40}", r[0].to_string()), &got, r);
                }
            }
            "trends" => {
                let vault = base.join("vault");
                for f in c["in"].as_array().unwrap() {
                    let p = vault.join("Daily").join(f["Name"].as_str().unwrap());
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(&p, f["Text"].as_str().unwrap()).unwrap();
                }
                let empty = base.join("empty");
                std::fs::create_dir_all(&empty).unwrap();
                for r in &rows {
                    let (ds, key) = match r[0].as_i64() {
                        Some(n) => (daily::read_days(vault.to_str().unwrap(), n), r[0].clone()),
                        None => (daily::read_days(empty.to_str().unwrap(), 30), r[0].clone()),
                    };
                    let days: Vec<Value> = ds.iter().map(|x| json!([x.date, numbers(&x.numbers)])).collect();
                    let days = if days.is_empty() { Value::Null } else { json!(days) };
                    d.eq(format!("trends {key}"), &json!([key, days, daily::trends(&ds)]), r);
                }
            }
            "trendsdays" => {
                for r in &rows {
                    let ds: Vec<daily::Day> = r[0]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(i, l)| daily::Day {
                            date: format!("2026-11-{:02}", i + 1),
                            numbers: daily::parse_numbers(l.as_str().unwrap()).unwrap(),
                        })
                        .collect();
                    d.eq("trendsdays", &json!([r[0], daily::trends(&ds)]), r);
                }
            }
            other => panic!("unknown case {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    d.done("daily");
}
