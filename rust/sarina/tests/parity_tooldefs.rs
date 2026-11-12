//! testdata/parity/tooldefs.json: every tool the Go build offers (with the optional sets off and on): its permission,
//! its plan-mode class and its definition exactly as the model receives it.

mod common;

use sarina::tools::defs::definition;
use sarina::tools::registry::Class;
use serde_json::json;

#[test]
fn tool_definitions_match_go() {
    let mut check = common::Check::new();
    for c in common::cases("tooldefs") {
        for t in c["out"].as_array().unwrap() {
            let name = t[0].as_str().unwrap();
            let Some(d) = definition(name) else {
                check.eq(name, &json!("missing"), &t[3]);
                continue;
            };
            let class = match d.class {
                Class::Unset => 0,
                Class::ReadOnly => 1,
                Class::Mutating => 2,
                Class::PlanControl => 3,
            };
            let got = json!([d.spec.name, d.requires.as_str(), class, d.spec.to_json().unwrap()]);
            check.eq(format!("{} {name}", c["in"]), &got, t);
        }
    }
    check.done("tooldefs");
}
