//! The side-by-side tests' way into single functions (never installed; built with --features testhook).
//!
//! One JSON request per line on stdin, one JSON answer per line on stdout:
//!   {"fn": NAME, "args": [...], "environ": {...}, "cwd": DIR, "now": "YYYY-MM-DDTHH:MM:SS" or null}
//! "environ" becomes this process's environment (os.environ for the function), "now" fixes the clock. The answer is
//! {"ok": RESULT} or {"crash": WHAT}. Streamers live between calls by number.

use sarina_voice::ask::{self, Ctx, Streamer};
use sarina_voice::checkup;
use sarina_voice::json::{self, Value};
use sarina_voice::py::{self, Env, LocalTime, PyResult};
use std::collections::HashMap;
use std::io::{BufRead, Write};

fn strs(v: &[String]) -> Value {
    Value::Arr(v.iter().map(|x| json::s(x)).collect())
}

fn arg_str(a: &[Value], i: usize) -> String {
    a.get(i).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn arg_env(a: &[Value], i: usize) -> Env {
    let mut e = Env::default();
    if let Some(Value::Obj(m)) = a.get(i) {
        for (k, v) in m {
            e.set(k, v.as_str().unwrap_or(""));
        }
    }
    e
}

fn arg_list(a: &[Value], i: usize) -> Vec<String> {
    match a.get(i) {
        Some(Value::Arr(x)) => x.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect(),
        _ => vec![],
    }
}

fn now_arg(a: &[Value], i: usize) -> Option<LocalTime> {
    a.get(i).and_then(|v| v.as_str()).and_then(py::parse_iso_datetime)
}

fn call(f: &str, a: &[Value], streamers: &mut HashMap<i64, Streamer>, next: &mut i64) -> PyResult<Value> {
    let ctx = Ctx::new(Env::from_process());
    Ok(match f {
        "spoken" => json::s(&ask::spoken(&arg_str(a, 0))),
        "laugh" => Value::Bool(ask::laugh(&arg_str(a, 0))),
        "drop_offers" => json::s(&ask::drop_offers(&arg_str(a, 0))),
        "flatten" => json::s(&ask::flatten(&arg_str(a, 0))),
        "shorten" => json::s(&ask::shorten(&arg_str(a, 0), 75)),
        "clean_sentence" => json::s(&ask::clean_sentence(&arg_str(a, 0))),
        "keyword_topics" => strs(&ask::keyword_topics(&arg_str(a, 0))),
        "words_of" => {
            let mut w: Vec<String> = ask::words_of(&arg_str(a, 0)).into_iter().collect();
            w.sort();
            strs(&w)
        }
        "plain" => json::s(&ask::plain(
            &arg_str(a, 0),
            a.get(1).and_then(|v| if let Value::Int(n) = v { n.parse().ok() } else { None }).unwrap_or(0),
        )),
        "recall" => strs(&ask::recall(&arg_str(a, 0), &arg_str(a, 1), &arg_env(a, 2))?),
        "learned" => {
            let today = a.get(1).and_then(|v| v.as_str()).and_then(py::date_fromiso).unwrap_or_else(|| LocalTime::now().days());
            strs(&ask::learned(&ctx, &arg_str(a, 0), today, 6)?)
        }
        "relevant" => strs(&ask::relevant(&ctx, &arg_str(a, 0), &arg_list(a, 1), &arg_env(a, 2))?),
        "companion" => json::s(&ask::companion(&ctx, &arg_str(a, 0), &arg_env(a, 1), now_arg(a, 2))?),
        "told_jokes" => strs(&ask::told_jokes(&arg_str(a, 0))?),
        "a_repeat" => json::s(&ask::a_repeat(&arg_str(a, 0), &arg_list(a, 1))),
        "is_echo" => Value::Bool(ask::is_echo(&arg_str(a, 0), &arg_str(a, 1))),
        "about_the_computer" => Value::Bool(ask::about_the_computer(&arg_list(a, 0), &arg_str(a, 1))),
        "log_talk" => {
            ask::log_talk(&arg_str(a, 0), &arg_str(a, 1), &arg_env(a, 2))?;
            Value::Null
        }
        "log_joke" => {
            ask::log_joke(&arg_str(a, 0), &arg_str(a, 1), &arg_env(a, 2))?;
            Value::Null
        }
        "save_turn" => {
            ask::save_turn(&arg_str(a, 0), &arg_str(a, 1), &arg_str(a, 2));
            Value::Null
        }
        "last_turn" => match ask::last_turn(&arg_str(a, 0))? {
            Some((q, ans)) => Value::Arr(vec![json::s(&q), json::s(&ans)]),
            None => Value::Null,
        },
        "note_laugh" => {
            let prev = match a.first() {
                Some(Value::Arr(x)) if x.len() == 2 => {
                    Some((x[0].as_str().unwrap_or("").to_string(), x[1].as_str().unwrap_or("").to_string()))
                }
                _ => None,
            };
            ask::note_laugh(prev, &arg_env(a, 1))?;
            Value::Null
        }
        "route" => match ask::route(&ctx, &arg_str(a, 0), &arg_env(a, 1))? {
            Some(t) => strs(&t),
            None => Value::Null,
        },
        "command" => {
            // command(question, cwd, write, shell, voice, resume, rules)
            let b = |i: usize| a.get(i).map(|v| v.truthy()).unwrap_or(false);
            let resume = a.get(5).and_then(|v| v.as_str()).map(|s| s.to_string());
            strs(&ask::command(&ctx, &arg_str(a, 0), &arg_str(a, 1), b(2), b(3), b(4), resume.as_deref(), &arg_str(a, 6))?)
        }
        "streamer_new" => {
            *next += 1;
            streamers.insert(*next, Streamer::new(&arg_str(a, 0), a.get(1).map(|v| v.truthy()).unwrap_or(false)));
            Value::Int(next.to_string())
        }
        "streamer_feed" | "streamer_finish" | "streamer_text" => {
            let id: i64 = match a.first() {
                Some(Value::Int(n)) => n.parse().unwrap_or(0),
                _ => 0,
            };
            let st = streamers.get_mut(&id).expect("no such streamer");
            match f {
                "streamer_feed" => strs(&st.feed(&arg_str(a, 1), a.get(2).map(|v| v.truthy()).unwrap_or(false))),
                "streamer_finish" => strs(&st.finish()),
                _ => json::s(&st.text()),
            }
        }
        // the check-up
        "checkup_note" => {
            checkup::note(&arg_str(a, 0), &arg_str(a, 1))?;
            Value::Null
        }
        "security_keys" => {
            let keys = checkup::security_keys(a.first().unwrap_or(&Value::Null))?;
            Value::Obj(keys)
        }
        "security_review" => {
            let c = checkup::Checkup::new(Env::from_process());
            json::s(&checkup::security_review(&c, &arg_env(a, 0), &arg_str(a, 1), &arg_list(a, 2))?)
        }
        "held_check" => {
            let mut c = checkup::Checkup::new(Env::from_process());
            if let Some(r) = a.get(2).and_then(|v| v.as_str()) {
                c.requests = r.to_string();
            }
            let held = match a.first() {
                Some(Value::Arr(x)) => Some(x.clone()),
                _ => None,
            };
            let broken = match a.get(1) {
                Some(Value::Arr(x)) => Some(
                    x.iter()
                        .map(|p| match p {
                            Value::Arr(t) if t.len() == 2 => {
                                (t[0].as_str().unwrap_or("").to_string(), t[1].as_str().unwrap_or("").to_string())
                            }
                            _ => (String::new(), String::new()),
                        })
                        .collect(),
                ),
                _ => None,
            };
            strs(&checkup::held_check(&c, held, broken)?)
        }
        "float_repr" => json::s(&py::float_repr(match a.first() {
            Some(Value::Float(f)) => *f,
            Some(Value::Int(n)) => n.parse().unwrap_or(0.0),
            _ => 0.0,
        })),
        "json_roundtrip" => match json::loads(&arg_str(a, 0)) {
            Ok(v) => json::s(&json::dumps(&v)),
            Err(json::DecodeError) => Value::Null,
        },
        "splitlines" => strs(&py::splitlines(&arg_str(a, 0)).into_iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        "split_ws" => strs(&py::split_ws(&arg_str(a, 0)).into_iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        "normpath" => json::s(&py::normpath(&arg_str(a, 0))),
        "realpath" => json::s(&py::realpath(&arg_str(a, 0))?),
        _ => return Ok(Value::Obj(vec![("unknown".into(), json::s(f))])),
    })
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut streamers: HashMap<i64, Streamer> = HashMap::new();
    let mut next = 0i64;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = json::loads(&line) else { break };
        // this call's os.environ, working folder and clock
        if let Some(Value::Obj(m)) = req.get("environ") {
            for (k, _) in std::env::vars_os() {
                unsafe { std::env::remove_var(k) };
            }
            for (k, v) in m {
                unsafe { std::env::set_var(k, v.as_str().unwrap_or("")) };
            }
        }
        match req.get("now").and_then(|v| v.as_str()) {
            Some(n) => unsafe { std::env::set_var("SARINA_VOICE_TEST_NOW", n) },
            None => unsafe { std::env::remove_var("SARINA_VOICE_TEST_NOW") },
        }
        if let Some(d) = req.get("cwd").and_then(|v| v.as_str()) {
            let _ = std::env::set_current_dir(d);
        }
        let f = req.get("fn").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let args = match req.get("args") {
            Some(Value::Arr(a)) => a.clone(),
            _ => vec![],
        };
        let reply = match call(&f, &args, &mut streamers, &mut next) {
            Ok(v) => json::obj(vec![("ok", v)]),
            Err(c) => json::obj(vec![("crash", json::s(&c.0))]),
        };
        let _ = writeln!(out, "{}", json::dumps(&reply));
        let _ = out.flush();
    }
}
