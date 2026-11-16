//! testdata/parity/classic.json: the classical ciphers as crypto_classic answers (go/cryptotools/classic_parity_test.go):
//! every cipher encoded and decoded with good and bad keys, every cracker, and the English statistics bit for bit.

mod common;
use common::{cases, Check};
use sarina::cryptotools::{classic, english, truncate_text};
use serde_json::json;

const PROSE: &str = "Attack at dawn. The enemy camp lies to the north of the river and we must move before the first light of the morning or all is lost. Send word to the other side and tell them to hold the bridge until we come. The gold is hidden under the old tree by the water.";
const TEXTS2: [&str; 9] = [
    "the cat and dog",
    "the zzz qqq xxx yyy",
    "the and zzz qqq xxx",
    "one two three four five",
    "xx yy zz",
    "the be to of xx",
    "the be to of xx yy",
    "attack at dawn",
    "qqq www eee",
];

fn bits(x: f64) -> String {
    format!("{:016x}", x.to_bits())
}

#[test]
fn classic_matches_go() {
    let mut d = Check::new();
    for c in cases("classic") {
        for r in c["out"].as_array().unwrap() {
            match c["fn"].as_str().unwrap() {
                "english" => {
                    let s = r[0].as_str().unwrap();
                    let got = json!([
                        s,
                        bits(english::chi_squared(s)),
                        bits(english::word_ratio(s)),
                        bits(english::printable(s)),
                        english::looks_like_text(s),
                        bits(english::index_of_coincidence(s)),
                        bits(english::entropy(s.as_bytes())),
                        classic::lower_letters(s),
                        classic::letter_order(s),
                        bits(classic::ngram_rate(s)),
                        bits(classic::word_cover(&classic::lower_letters(s))),
                        classic::reads_as_english(s),
                        bits(classic::substitution_fitness(s)),
                        truncate_text(s, 10)
                    ]);
                    d.eq(format!("english {s:.40?}"), &got, r);
                }
                "classic" => {
                    let s = |i: usize| r[i].as_str().unwrap();
                    let (out, err) = match classic::classic(s(0), s(1), s(2), r[3].as_bool().unwrap(), r[4].as_bool().unwrap()) {
                        Ok(o) => (o, String::new()),
                        Err(e) => (String::new(), e),
                    };
                    let got = json!([s(0), s(1), s(2), r[3], r[4], out, err]);
                    d.eq(format!("classic {} key {:?} decode {} crack {} on {:.30?}", s(1), s(2), r[3], r[4], s(0)), &got, r);
                }
                "rank" => {
                    let got = match r[0].as_str().unwrap() {
                        "keylengths" => {
                            let v = classic::vigenere(PROSE, r[1].as_str().unwrap(), false).unwrap();
                            let ls = classic::lower_letters(&v);
                            let ics: Vec<String> = (1..=16).map(|n| bits(classic::column_ic(ls.as_bytes(), n))).collect();
                            json!(["keylengths", r[1], classic::vigenere_key_lengths(ls.as_bytes(), 16), ics])
                        }
                        "ic" => {
                            let x = f64::from_bits(u64::from_str_radix(r[1].as_str().unwrap(), 16).unwrap());
                            json!(["ic", r[1], classic::ic_meaning(x)])
                        }
                        _ => {
                            let i = r[1].as_i64().unwrap();
                            let mut gs: Vec<classic::Guess> = TEXTS2
                                .iter()
                                .enumerate()
                                .map(|(j, t)| classic::Guess {
                                    key: j.to_string(),
                                    text: t.to_string(),
                                    score: ((i * 7 + j as i64 * 3) % 11) as f64 - 5.0,
                                    note: String::new(),
                                })
                                .collect();
                            classic::rank_keyed_guesses(&mut gs);
                            json!(["keyed", i, gs.iter().map(|g| g.key.clone()).collect::<Vec<_>>()])
                        }
                    };
                    d.eq(format!("rank {}", r), &got, r);
                }
                f => panic!("unknown fn {f}"),
            }
        }
    }
    d.done("classic");
}
