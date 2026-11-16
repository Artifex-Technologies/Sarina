//! testdata/parity/quantum.json: Go's math.Sin, Cos, Sincos and cmplx.Exp of a pure imaginary number bit for bit, the
//! report's numbers, the qubit count as `sarina quantum` reads it (fmt.Sscan), and whole simulations with fixed seeds
//! (go/quantum/quantum_parity_test.go).

mod common;
use common::{bytes, cases, Check};
use sarina::{gomath, quantum};
use serde_json::{json, Value};

fn hex(x: f64) -> String {
    format!("{:016x}", x.to_bits())
}

#[test]
fn quantum_matches_go() {
    let mut d = Check::new();
    for c in cases("quantum") {
        let rows = c["out"].as_array().unwrap();
        match c["fn"].as_str().unwrap() {
            "trig" => {
                for r in rows {
                    let r: Vec<&str> = r.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
                    let x = f64::from_bits(u64::from_str_radix(r[0], 16).unwrap());
                    let (s, co) = gomath::sincos(x);
                    let e = (gomath::exp(0.0) * co, gomath::exp(0.0) * s);
                    let got = [hex(x), hex(gomath::sin(x)), hex(gomath::cos(x)), hex(s), hex(co), hex(e.0), hex(e.1)];
                    // NaN's sign and payload are not Go's to promise; any NaN is NaN
                    for (k, (g, w)) in got.iter().zip(&r).enumerate() {
                        let (gv, wv) =
                            (f64::from_bits(u64::from_str_radix(g, 16).unwrap()), f64::from_bits(u64::from_str_radix(w, 16).unwrap()));
                        if !(gv.is_nan() && wv.is_nan()) {
                            d.eq(format!("trig {} [{k}]", r[0]), &json!(g), &json!(w));
                        }
                    }
                }
            }
            "num" => {
                for r in rows {
                    let r: Vec<&str> = r.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
                    let x = f64::from_bits(u64::from_str_radix(r[0], 16).unwrap());
                    let c = quantum::C;
                    let got = [
                        quantum::num(x),
                        quantum::cnum(c(x, 0.0)),
                        quantum::cnum(c(0.0, x)),
                        quantum::cnum(c(x, x)),
                        quantum::cnum(c(x, -x)),
                        quantum::cnum(c(-x, 0.5)),
                        quantum::cnum(c(0.5, x)),
                    ];
                    for (k, g) in got.iter().enumerate() {
                        d.eq(format!("num {} [{k}]", r[0]), &json!(g), &json!(r[k + 1]));
                    }
                }
            }
            "sscan" => {
                for r in rows {
                    let s = r[0].as_str().unwrap();
                    let got = sarina::gotext::sscan_int(s);
                    let want = if r[2].as_bool().unwrap() { Value::Null } else { r[1].clone() };
                    d.eq(format!("sscan {s:?}"), &json!(got), &want);
                }
            }
            "simulate" => {
                for r in rows {
                    let circuit = bytes(&r[1]);
                    let got = quantum::simulate(r[0].as_i64().unwrap(), &circuit, r[2].as_i64().unwrap(), r[3].as_i64().unwrap());
                    let (out, err) = match got {
                        Ok(o) => (o, String::new()),
                        Err(e) => (String::new(), e),
                    };
                    let what = format!("simulate {} {:.60}", r[0], String::from_utf8_lossy(&circuit));
                    d.eq(format!("{what}: out"), &json!(out), &r[4]);
                    d.eq(format!("{what}: error"), &json!(err), &r[5]);
                }
            }
            "state" => {
                for r in rows {
                    let circuit = bytes(&r[1]);
                    let n = r[0].as_i64().unwrap();
                    let mut st = quantum::State::new(n).unwrap();
                    st.run(&quantum::parse(&circuit, n).unwrap());
                    // any NaN is NaN (Go's NaN() has its own payload)
                    let nan = |h: &str| {
                        let v = f64::from_bits(u64::from_str_radix(h, 16).unwrap());
                        if v.is_nan() {
                            "nan".to_owned()
                        } else {
                            h.to_owned()
                        }
                    };
                    let pair = |h: &str| nan(&h[..16]) + &nan(&h[16..]);
                    let got: Vec<String> = st.amp.iter().take(64).map(|a| pair(&(hex(a.0) + &hex(a.1)))).collect();
                    let want: Vec<String> = r[2].as_array().unwrap().iter().map(|v| pair(v.as_str().unwrap())).collect();
                    d.eq(format!("state {n} {:.60}", String::from_utf8_lossy(&circuit)), &json!(got), &json!(want));
                }
            }
            f => panic!("unknown fn {f}"),
        }
    }
    d.done("quantum");
}
