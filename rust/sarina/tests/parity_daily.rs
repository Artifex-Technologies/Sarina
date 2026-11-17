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
            "render" => {
                let at = ("2026-11-16".to_string(), "09:05".to_string());
                let statuses = [
                    daily::Status { at: at.clone(), ..daily::Status::default() },
                    daily::Status {
                        at: at.clone(),
                        cores: 8,
                        load: 4.05,
                        mem_used: 6_500_000_000,
                        mem_total: 16_000_000_000,
                        swap_used: 1,
                        disks: vec![
                            daily::Disk { mount: "/".into(), pct: 90, free: 50_500_000_000 },
                            daily::Disk { mount: "/home".into(), pct: 40, free: 499_999_999 },
                        ],
                        crashes: [("b".to_string(), 2), ("a".to_string(), 2), ("c".to_string(), 5)].into_iter().collect(),
                        kernel: 3,
                        security: "Heimdall: x".into(),
                        ..daily::Status::default()
                    },
                    daily::Status {
                        at: at.clone(),
                        cores: 4,
                        load: 4.5,
                        mem_used: 1,
                        mem_total: 3,
                        failed: vec!["x.service".into(), "y (your session)".into()],
                        ..daily::Status::default()
                    },
                    daily::Status { at: at.clone(), cores: 4, load: 2.0, ..daily::Status::default() },
                ];
                let text =
                    common::cases("daily").into_iter().find(|c| c["fn"] == "parse").unwrap()["out"][2][0].as_str().unwrap().to_string();
                for (i, st) in statuses.iter().enumerate() {
                    let (section, n) = st.render();
                    let mut note = daily::parse("2026-11-16", &text);
                    if i == 1 {
                        for _ in 0..50 {
                            note.numbers.push("- 09:00 | disk / 40% | crashes 0 | failed 0 | memory 30% | load 0.5".into());
                        }
                    }
                    note.set_status(st);
                    d.eq(format!("render {i}"), &json!([i, section, numbers(&n), note.render(), note.print(false)]), &rows[i]);
                }
            }
            other => panic!("unknown case {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    d.done("daily");
}
