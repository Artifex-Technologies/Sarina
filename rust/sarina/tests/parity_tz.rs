//! testdata/parity/tz.json: Go's time zone files and lookups (go/internal/parity/tz_parity_test.go): TZif files read as
//! LoadLocationFromTZData reads them, and times looked up in them (the POSIX TZ footer included), as the agent's prompt
//! writes today's date in the local zone.

mod common;
use common::{cases, Check};
use sarina::tz;
use serde_json::{json, Value};

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

/// Go's `Format("2006-01-02 15:04")` of Unix time `sec` at `off` seconds east of UTC (years 0 to 9999; the fixture's
/// times outside them are compared by offset only).
fn ymd_hm(sec: i64, off: i64) -> Option<String> {
    let s = sarina::gotime::Time::from_unix(sec + off, 0).format_rfc3339_utc();
    (s.len() == 20).then(|| format!("{} {}", &s[..10], &s[11..16]))
}

#[test]
fn tz_matches_go() {
    let mut d = Check::new();
    for c in cases("tz") {
        let name = c["in"]["name"].as_str().unwrap().to_owned();
        let loc = tz::from_tzdata(&unhex(c["in"]["data"].as_str().unwrap()));
        d.eq(format!("{name}: loads"), &json!(loc.is_some()), &json!(c["out"]["error"].as_str().unwrap().is_empty()));
        let Some(loc) = loc else { continue };
        for z in c["out"]["zones"].as_array().unwrap() {
            let sec = z["sec"].as_i64().unwrap();
            let (zn, off, dst) = loc.lookup(sec);
            let what = format!("{name} at {sec}");
            d.eq(format!("{what}: name"), &Value::String(zn), &z["name"]);
            d.eq(format!("{what}: offset"), &json!(off), &z["offset"]);
            d.eq(format!("{what}: dst"), &json!(dst), &z["dst"]);
            if let Some(local) = ymd_hm(sec, off) {
                if (0..=9999).contains(&local[..4].parse::<i64>().unwrap_or(-1)) && !z["local"].as_str().unwrap().starts_with('-') {
                    d.eq(format!("{what}: local"), &Value::String(local), &z["local"]);
                }
            }
        }
    }
    d.done("tz");
}
