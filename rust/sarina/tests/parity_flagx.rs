//! testdata/parity/flagx.json: Go's flagx.Parse with a flag set of a bool, a string, an int and a bool that defaults to
//! true: positionals, error, the flag set's output (error and usage), and every flag's value.

mod common;

use sarina::flagx;
use sarina::goflag::FlagSet;
use serde_json::{json, Value};

#[test]
fn flagx_matches_go() {
    let mut check = common::Check::new();
    for c in common::cases("flagx") {
        let args: Vec<String> = match &c["in"] {
            Value::Null => vec![],
            v => v.as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect(),
        };
        let mut fs = FlagSet::new("t");
        fs.bool("w", false, "allow writing");
        fs.string("cwd", ".", "the working folder");
        fs.int("n", 0, "how many\nat most");
        fs.bool("b", true, "");
        let mut out = Vec::new();
        let (pos, err) = match flagx::parse(&mut fs, &args, &mut out) {
            Ok(p) => (p, String::new()),
            Err(e) => (vec![], e),
        };
        let got = json!({
            "positional": if pos.is_empty() { Value::Null } else { json!(pos) },
            "err": err,
            "output": String::from_utf8(out).unwrap(),
            "w": fs.is_true("w"),
            "cwd": fs.str("cwd"),
            "n": fs.num("n"),
            "b": fs.is_true("b"),
        });
        check.eq(format!("Parse({args:?})"), &got, &c["out"]);
    }
    check.done("flagx");
}
