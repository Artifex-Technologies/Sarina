//! The side-by-side checks' entry to single functions (tests/differential.py); never installed.
//!
//! - `phonemes VOICE.onnx TEXT`: piper's phonemes per sentence, one JSON list per line
//! - `phonemes-lines VOICE.onnx`: the same for each JSON string on stdin, one line per text
//! - `clauses ESPEAK_VOICE TEXT`: espeak-ng's clauses as (phonemes, terminator, end of sentence), one JSON list per line
//! - `clauses-lines ESPEAK_VOICE`: the same for each JSON string on stdin, one line of clauses per text
//! - `transcribe MODEL WAV`: speech to text (whisper-cli on the processor)
//! - `speak VOICE.onnx OUT.wav TEXT`: the helper's TextToSpeech.synthesize into a file
//! - `seed N` before any of them: ONNX Runtime's seed for the graphs' random numbers (the voices draw noise)

use sarina_helpers::pyjson::{dumps, V};
use sarina_helpers::voice::{espeak, piper};

fn list(v: &[String]) -> String {
    dumps(&V::Arr(v.iter().map(|s| V::str(s)).collect()))
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("seed") {
        let n: i64 = args[1].parse().expect("seed");
        sarina_helpers::ortrt::set_seed(n).expect("seed");
        args.drain(..2);
    }
    match args.first().map(|s| s.as_str()) {
        Some("phonemes") => {
            let v = piper::Voice::load(&args[1], None).expect("voice");
            for s in v.phonemize(&args[2]).expect("phonemize") {
                println!("{}", list(&s));
            }
        }
        Some("phonemes-lines") => {
            // one JSON string per stdin line in, piper's phonemes per sentence as one JSON list per line out
            use std::io::BufRead;
            let v = piper::Voice::load(&args[1], None).expect("voice");
            for line in std::io::stdin().lock().lines() {
                let line = line.expect("stdin");
                let Ok(V::Str(t)) = sarina_helpers::pyjson::loads(&line) else { panic!("not a JSON string: {line}") };
                let ss = v.phonemize(&t.s).expect("phonemize");
                println!("{}", dumps(&V::Arr(ss.iter().map(|s| V::Arr(s.iter().map(|p| V::str(p)).collect())).collect())));
            }
        }
        Some("clauses") => {
            for c in espeak::phonemize(&args[1], &args[2]).expect("espeak") {
                println!("{}", dumps(&V::Arr(vec![V::str(&c.phonemes), V::str(c.terminator), V::Bool(c.end_of_sentence)])));
            }
        }
        Some("clauses-lines") => {
            // one JSON string per stdin line in, one JSON list of clauses per line out
            use std::io::BufRead;
            for line in std::io::stdin().lock().lines() {
                let line = line.expect("stdin");
                let Ok(V::Str(t)) = sarina_helpers::pyjson::loads(&line) else { panic!("not a JSON string: {line}") };
                let cs = espeak::phonemize(&args[1], &t.s).expect("espeak");
                let v = V::Arr(
                    cs.iter().map(|c| V::Arr(vec![V::str(&c.phonemes), V::str(c.terminator), V::Bool(c.end_of_sentence)])).collect(),
                );
                println!("{}", dumps(&v));
            }
        }
        Some("transcribe") => {
            let model = sarina_helpers::voice::stt::resolve_model(&args[1]).expect("model");
            let stt = sarina_helpers::voice::stt::SpeechToText { whisper: sarina_helpers::voice::stt::program(), model, gpu: false };
            println!("{}", stt.transcribe(std::path::Path::new(&args[2])).expect("transcribe"));
        }
        Some("speak") => {
            let mut v = piper::Voice::load(&args[1], None).expect("voice");
            match piper::synthesize(&mut v, &args[3], &mut |w| eprintln!("{w}")) {
                Ok(wav) => std::fs::write(&args[2], wav).expect("write"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1)
                }
            }
        }
        _ => {
            eprintln!("usage: sarina-helpers-parity [seed N] phonemes|clauses|speak ...");
            std::process::exit(2)
        }
    }
}
