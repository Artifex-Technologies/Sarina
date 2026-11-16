//! testdata/parity/reverse.json: the pure parts of the reverse-engineering tool (go/reverse/reverse_parity_test.go): the
//! network map of a program's strings with net.ParseIP and SplitHostPort, the condensed overview, the program's own
//! functions, the functions a decompiled function calls, the cut at 14,000 characters.

mod common;
use common::{bytes, cases, Check};
use sarina::reverse;
use serde_json::{json, Value};

fn text(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => json!(s),
        Err(_) => json!({ "hex": b.iter().map(|c| format!("{c:02x}")).collect::<String>() }),
    }
}

/// Go's `parity.Texts`: always a list.
fn texts(xs: &[Vec<u8>]) -> Value {
    Value::Array(xs.iter().map(|x| text(x)).collect())
}

#[test]
fn reverse_matches_go() {
    // no heimdalld: the map without origins, as the fixture was written
    let empty = std::env::temp_dir().join(format!("parity-reverse-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    std::env::set_var("PATH", &empty);
    let mut d = Check::new();
    let ctx = sarina::goctx::Ctx::background();
    for c in cases("reverse") {
        for r in c["out"].as_array().unwrap() {
            match c["fn"].as_str().unwrap() {
                "ip" => {
                    let s = r[0].as_str().unwrap();
                    let split = |s: &str| match reverse::split_host_port(s) {
                        Ok((h, p)) => vec![json!(h), json!(p), json!("")],
                        Err(e) => vec![json!(""), json!(""), json!(e)],
                    };
                    let mut got = vec![json!(s)];
                    if r[1] == "split" {
                        got.push(json!("split"));
                    } else {
                        let ip = reverse::parse_ip(s);
                        got.push(json!(ip.is_some()));
                        if let Some((u, l, v4)) = ip {
                            got.extend([json!(u), json!(l), json!(v4)]);
                        }
                    }
                    got.extend(split(s));
                    d.eq(format!("ip {s:?}"), &Value::Array(got), r);
                }
                "netmap" => {
                    let t = bytes(&r[0]);
                    let nm = reverse::find_addresses(&t);
                    let got = json!([
                        r[0],
                        texts(&nm.urls),
                        texts(&nm.hosts),
                        texts(&nm.ips),
                        texts(&nm.onions),
                        text(&nm.report(&ctx, b"demo.bin"))
                    ]);
                    d.eq(format!("netmap {:.60}", String::from_utf8_lossy(&t)), &got, r);
                }
                "pure" => {
                    let what = r[0].as_str().unwrap();
                    let got = match what {
                        "condense" => json!([what, r[1], text(&reverse::condense_overview(&bytes(&r[1])))]),
                        "own" => json!([what, r[1], reverse::own_functions(&bytes(&r[1]))]),
                        "calls" => {
                            let calls: Vec<Vec<u8>> = reverse::called_functions(&String::from_utf8_lossy(&bytes(&r[1])), "main")
                                .into_iter()
                                .map(String::into_bytes)
                                .collect();
                            json!([what, r[1], texts(&calls)])
                        }
                        _ => {
                            // the cut: the fixture holds the first 20 bytes and the length; the input is rebuilt from them
                            let head = bytes(&r[1]);
                            let n = r[2].as_u64().unwrap() as usize;
                            let s: Vec<u8> = if head.is_empty() { Vec::new() } else { head.iter().copied().cycle().take(n).collect() };
                            let cut = reverse::cut_runes(&s, 14000);
                            json!([what, r[1], n, cut.is_some(), text(&cut.unwrap_or_default())])
                        }
                    };
                    d.eq(format!("{what} {:.60}", r[1]), &got, r);
                }
                f => panic!("unknown fn {f}"),
            }
        }
    }
    let _ = std::fs::remove_dir(&empty);
    d.done("reverse");
}
