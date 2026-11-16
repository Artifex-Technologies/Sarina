//! testdata/parity/modern.json: crypto_modern without crypto/x509 (go/cryptotools/modern_parity_test.go): SSH keys,
//! JWTs, private keys refused, PGP blocks without gpg and with the fixture's stand-in gpg.

mod common;
use common::{cases, Check};
use sarina::cryptotools::modern;
use serde_json::{json, Value};

fn text(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => json!(s),
        Err(_) => json!({ "hex": b.iter().map(|c| format!("{c:02x}")).collect::<String>() }),
    }
}

#[test]
fn modern_matches_go() {
    let mut d = Check::new();
    let base = std::env::temp_dir().join(format!("parity-modern-{}", std::process::id()));
    let (empty, bin) = (base.join("empty"), base.join("bin"));
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    for c in cases("modern") {
        if c["fn"] == "gpg" {
            use std::os::unix::fs::PermissionsExt;
            let p = bin.join("gpg");
            std::fs::write(&p, c["in"].as_str().unwrap()).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        } else {
            std::env::set_var("PATH", &empty);
        }
        for r in c["out"].as_array().unwrap() {
            let (input, key, mode) = (r[0].as_str().unwrap(), r[1].as_str().unwrap(), r[2].as_str().unwrap());
            std::env::set_var("PGP_MODE", mode);
            let got = match modern::modern(input, key, (1_700_000_000, 0)) {
                Some(Ok(o)) => json!([input, key, mode, text(&o), ""]),
                Some(Err(e)) => json!([input, key, mode, "", e]),
                None => json!("needs crypto/x509"),
            };
            d.eq(format!("modern {input:.50?} key {key:?} {mode}"), &got, r);
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    d.done("modern");
}
