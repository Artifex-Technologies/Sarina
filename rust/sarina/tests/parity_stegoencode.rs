//! testdata/parity/stegoencode.json: the pictures stego writes, byte for byte: Embed's PNG and CleanImage's PNG or
//! JPEG with the noise Go drew (go/stego/encode_parity_test.go).

mod common;
use common::{short_sum, Check};
use sarina::stego::{lsb, watermark};
use serde_json::{json, Value};

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

fn out(b: &[u8]) -> Value {
    if b.len() <= 4096 {
        json!(b.iter().map(|x| format!("{x:02x}")).collect::<String>())
    } else {
        json!(format!("{}:{}", b.len(), short_sum(b)))
    }
}

#[test]
fn stego_encode_matches_go() {
    let mut d = Check::new();
    for c in common::cases("stegoencode") {
        let rows = c["out"].as_array().unwrap();
        if c["fn"] == "zlib" {
            let text = "the quick brown fox jumps over the lazy dog, and the dog jumps back over the fox; ".repeat(2000).into_bytes();
            let mut x: u64 = 88172645463325252;
            let mut next = || {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x
            };
            let random: Vec<u8> = (0..200000).map(|_| next() as u8).collect();
            let skewed: Vec<u8> = (0..70000)
                .map(|_| {
                    let v = next();
                    ((v % 16) as u8) * (((v >> 8) % 3) as u8)
                })
                .collect();
            let cat = |a: &[u8], b: &[u8]| [a, b].concat();
            let streams: Vec<Vec<u8>> = vec![
                vec![],
                b"a".to_vec(),
                b"hello world, hello world".to_vec(),
                b"ab".repeat(16),
                b"xyz".repeat(11),
                random[..40].to_vec(),
                random[..100].to_vec(),
                random[..127].to_vec(),
                random[..128].to_vec(),
                random[..1000].to_vec(),
                random[..1025].to_vec(),
                random[..5000].to_vec(),
                random[..65535].to_vec(),
                random[..65536].to_vec(),
                random.clone(),
                text[..200].to_vec(),
                text[..3000].to_vec(),
                text[..65535].to_vec(),
                text[..65536].to_vec(),
                text.clone(),
                skewed.clone(),
                skewed[..1500].to_vec(),
                cat(&text[..40000], &random[..40000]),
                cat(&random[..30000], &text[..90000]),
                vec![0; 300000],
                cat(&[7u8; 70000], &text[..500]),
            ];
            let mut rows = rows.iter();
            for data in &streams {
                for size in [0usize, 1, 1000, 70000] {
                    if size == 1 && data.len() > 5000 {
                        continue;
                    }
                    let mut z = sarina::stego::deflate::ZlibWriter::new();
                    if size == 0 {
                        z.write(data);
                    } else {
                        for part in data.chunks(size) {
                            z.write(part);
                        }
                    }
                    let pieces = z.close();
                    let sizes: Vec<usize> = pieces.iter().map(Vec::len).collect();
                    let all = pieces.concat();
                    let r = rows.next().expect("a row");
                    d.eq(
                        format!("zlib {} by {size}", data.len()),
                        &json!([data.len(), if data.is_empty() { String::new() } else { short_sum(data) }, size, sizes, out(&all)]),
                        r,
                    );
                }
            }
            continue;
        }
        if c["fn"] == "embed" {
            for r in rows {
                let src = unhex(c["in"][r[0].as_str().unwrap()].as_str().unwrap());
                let p = lsb::decode_image(&src).unwrap();
                let n = r[1].as_u64().unwrap() as usize;
                let payload: Vec<u8> = match n {
                    2 => b"hi".to_vec(),
                    47 => b"a secret meeting at the old mill, bring the key".to_vec(),
                    3000 => vec![b'Z'; 3000],
                    _ => vec![0; n],
                };
                let got = match lsb::embed(&p, &payload) {
                    Ok(b) => json!([r[0], n, out(&b), ""]),
                    Err(e) => json!([r[0], n, "", e]),
                };
                d.eq(format!("embed {} {n}", r[0]), &got, r);
            }
            continue;
        }
        for r in rows {
            let (input, noise) = (unhex(r[1].as_str().unwrap()), unhex(r[2].as_str().unwrap()));
            let got = match watermark::clean_image(&input, &mut |_| noise.clone()) {
                Ok((b, ext)) => json!([r[0], r[1], r[2], out(&b), ext, ""]),
                Err(e) => json!([r[0], r[1], r[2], "", "", e]),
            };
            d.eq(format!("clean {}", r[0]), &got, r);
        }
    }
    d.done("stegoencode");
}
