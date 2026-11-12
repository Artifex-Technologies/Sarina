//! testdata/parity/guard.json: the gate's local checks on the Go tests' inputs, seeds changed in case, spacing, neighbours
//! and length, every format character, and generated lines.

mod common;

use sarina::guard::Gate;
use serde_json::json;

#[test]
fn guard_matches_go() {
    let mut check = common::Check::new();
    let (plain, bom) = (Gate::default(), Gate { allow_leading_bom: true, ..Gate::default() });
    for c in common::cases("guard") {
        let text = common::bytes(&c["in"]);
        let (a, b) = (plain.scan_local(&text), bom.scan_local(&text));
        let got = json!([a.blocked, common::text(a.reason.as_bytes()), b.blocked, common::text(b.reason.as_bytes())]);
        check.eq(format!("scan {}", c["in"]), &got, &c["out"]);
    }
    check.done("guard");
}
