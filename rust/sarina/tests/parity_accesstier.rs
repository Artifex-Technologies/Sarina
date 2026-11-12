//! testdata/parity/accesstier.json: AccessTierFromEnv against Heimdall's grants file (LoadGrants, TierCap) in every state
//! the gate knows, and a time.Time read from JSON (the grants' "until").

mod common;

use sarina::config::access_tier_from_env_with;
use sarina::gostruct::{unmarshal, Ty, Val};
use sarina::heimdall::load_grants_at;
use serde_json::json;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn chmod(p: &std::path::Path, mode: u32) {
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
}

#[test]
fn access_tier_matches_go() {
    let mut check = common::Check::new();
    let base = std::env::temp_dir().join(format!("sarina-accesstier-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let me = std::fs::metadata(&base).unwrap().uid();
    let cases = common::cases("accesstier");
    let env: Vec<String> = cases.iter().find(|c| c["fn"] == "env").unwrap()["in"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| String::from_utf8(common::bytes(v)).unwrap())
        .collect();
    for (n, c) in cases.iter().enumerate() {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "env" => continue,
            "grants" => {
                let dir = base.join(format!("g{n}")).join("heimdall");
                std::fs::create_dir_all(&dir).unwrap();
                let mut p = dir.join("grants.json");
                let mut owner = me;
                if !input["body"].is_null() {
                    std::fs::write(&p, common::bytes(&input["body"])).unwrap();
                    chmod(&p, input["mode"].as_u64().unwrap() as u32);
                }
                if let Some(m) = input["dirMode"].as_u64() {
                    chmod(&dir, m as u32);
                }
                match input["special"].as_str() {
                    Some("link") => {
                        let real = dir.join("grants.json.real");
                        std::fs::rename(&p, &real).unwrap();
                        std::os::unix::fs::symlink(&real, &p).unwrap();
                    }
                    Some("other-owner") => owner = me + 1,
                    Some("folder") => std::fs::create_dir(&p).unwrap(),
                    Some("under-file") => {
                        std::fs::write(dir.join("file"), b"").unwrap();
                        p = dir.join("file").join("grants.json");
                    }
                    _ => {}
                }
                let pb = p.to_str().unwrap().as_bytes().to_vec();
                let g = load_grants_at(&pb, owner);
                let caps: Vec<i64> = (0..=4).map(|own| g.tier_cap(own)).collect();
                let tiers: Vec<i64> = env
                    .iter()
                    .map(|v| {
                        access_tier_from_env_with(&|k| if k == "SARINA_ACCESS_TIER" { v.clone() } else { String::new() }, || {
                            load_grants_at(&pb, owner)
                        })
                    })
                    .collect();
                chmod(&p, 0o644);
                chmod(&dir, 0o755);
                let problem = g.problem.replace(dir.to_str().unwrap(), "{DIR}");
                json!({"on": g.on, "problem": problem, "cap": caps, "tiers": tiers})
            }
            "time" => {
                let doc = format!("\"{}\"", input.as_str().unwrap());
                static T: Ty = Ty::Time;
                match unmarshal(&T, doc.as_bytes()) {
                    (_, Some(e)) => json!(e.0),
                    (Val::Time(t), None) => json!([t.unix().to_string(), t.nanos(), t.offset()]),
                    _ => unreachable!(),
                }
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&base);
    check.done("accesstier");
}
