//! testdata/parity/shellparse.json: Go's shellparse.Parse on the shell corpus, compared byte for byte.

mod common;

use sarina::shellparse::{parse, Word};
use serde_json::{json, Map, Value};

fn word(w: &Word) -> Value {
    let mut m = Map::new();
    m.insert("t".into(), common::text(&w.text));
    if w.quoted {
        m.insert("q".into(), json!(true));
    }
    if w.proc_sub {
        m.insert("p".into(), json!(true));
    }
    Value::Object(m)
}

fn words(ws: &[Word]) -> Option<Value> {
    (!ws.is_empty()).then(|| Value::Array(ws.iter().map(word).collect()))
}

pub fn parsed(src: &[u8]) -> Value {
    match parse(src) {
        Err(e) => json!({ "err": e }),
        Ok(cmds) => {
            let mut out = Vec::new();
            for c in &cmds {
                let mut m = Map::new();
                if let Some(v) = words(&c.assigns) {
                    m.insert("assigns".into(), v);
                }
                if let Some(v) = words(&c.words) {
                    m.insert("words".into(), v);
                }
                if !c.redirects.is_empty() {
                    let rs = c
                        .redirects
                        .iter()
                        .map(|r| {
                            let mut rm = Map::new();
                            rm.insert("op".into(), json!(r.op));
                            rm.insert("target".into(), word(&r.target));
                            if r.writes() {
                                rm.insert("writes".into(), json!(true));
                            }
                            Value::Object(rm)
                        })
                        .collect();
                    m.insert("redirects".into(), Value::Array(rs));
                }
                if !c.heredocs.is_empty() {
                    m.insert("heredocs".into(), Value::Array(c.heredocs.iter().map(|h| common::text(h)).collect()));
                }
                if c.pipe_id != 0 {
                    m.insert("pipe".into(), json!(c.pipe_id));
                }
                if !c.func_name.is_empty() {
                    m.insert("func".into(), common::text(&c.func_name));
                }
                if c.data {
                    m.insert("data".into(), json!(true));
                }
                out.push(Value::Object(m));
            }
            json!({ "commands": out })
        }
    }
}

#[test]
fn shellparse_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("shellparse") {
        let src = common::bytes(&c["in"]);
        check.eq(format!("Parse({:?})", String::from_utf8_lossy(&src)), &parsed(&src), &c["out"]);
    }
    check.done("shellparse");
}
