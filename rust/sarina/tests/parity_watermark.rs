//! testdata/parity/watermark.json: stego's text watermarks (Watermarks, Describe, CleanText) and ScanFolder on a tree
//! made the same way as the Go test makes it (go/stego/watermark_parity_test.go).

mod common;
use common::{bytes, text, Check};
use sarina::stego::watermark;
use serde_json::{json, Value};

#[test]
fn watermark_matches_go() {
    let mut d = Check::new();
    for c in common::cases("watermark") {
        let rows = c["out"].as_array().unwrap();
        match c["fn"].as_str().unwrap() {
            "marks" => {
                for r in rows {
                    let s = bytes(&r[0]);
                    let w = watermark::watermarks(&s);
                    let hg: Vec<Value> = w.homoglyphs.iter().map(|(l, word, f)| json!([l, word, f])).collect();
                    let got = json!([
                        r[0],
                        w.found(),
                        w.selectors,
                        text(&w.selector_text),
                        text(w.trailing_bits.as_bytes()),
                        if hg.is_empty() { Value::Null } else { Value::Array(hg) },
                        text(&w.describe())
                    ]);
                    d.eq(format!("marks {:.60}", r[0].to_string()), &got, r);
                }
            }
            "clean" => {
                for r in rows {
                    let (out, removed) = watermark::clean_text(&bytes(&r[0]));
                    let got = json!([r[0], text(&out), removed]);
                    d.eq(format!("clean {:.60}", r[0].to_string()), &got, r);
                }
            }
            _ => {
                let base = std::env::temp_dir().join(format!("parity-watermark-{}", std::process::id()));
                let dir = base.join("tree");
                let write = |rel: &str, data: &[u8]| {
                    let p = dir.join(rel);
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(p, data).unwrap();
                };
                let trail = |s: &str| {
                    let mut b = String::new();
                    for c in s.bytes() {
                        b += "line";
                        for i in (0..8).rev() {
                            b.push(if c >> i & 1 == 1 { '\t' } else { ' ' });
                        }
                        b.push('\n');
                    }
                    b
                };
                write("a.txt", b"clean text\n");
                write("b.md", "hidden\u{200b}mark\n".as_bytes());
                write("c/d.txt", "p\u{430}ypal\n".as_bytes());
                write("c/e.bin", "bin\0ary with \u{200b}".as_bytes());
                write("c/pic.PNG", b"not really");
                write("c/z.jpeg", b"");
                write(".git/x.txt", "\u{200b}".as_bytes());
                write("node_modules/y.txt", "\u{200b}".as_bytes());
                write("deep/er/f.txt", trail("hi").as_bytes());
                write("big.txt", &["\u{200b}".as_bytes(), "x".repeat(4 << 20).as_bytes()].concat());
                write("g.txt", "x".repeat(4 << 20).as_bytes());
                write("target/t.txt", "\u{200b}".as_bytes());
                write("h.log", b"bad \xff\xe2\x80\x8b");
                std::os::unix::fs::symlink(dir.join("b.md"), dir.join("link.txt")).unwrap();
                let empty = base.join("empty");
                std::fs::create_dir_all(&empty).unwrap();
                for r in rows {
                    let limit = r[0].as_i64().unwrap();
                    let (target, root) = match limit {
                        -1 => (empty.clone(), empty.clone()),
                        -2 => (empty.join("missing"), empty.clone()),
                        _ => (dir.clone(), dir.clone()),
                    };
                    let out = watermark::scan_folder(target.to_str().unwrap(), if limit < 0 { 10 } else { limit as usize });
                    let out = String::from_utf8(out).unwrap().replace(root.to_str().unwrap(), "{DIR}");
                    d.eq(format!("scan {limit}"), &json!([limit, out]), r);
                }
                let _ = std::fs::remove_dir_all(&base);
            }
        }
    }
    d.done("watermark");
}
