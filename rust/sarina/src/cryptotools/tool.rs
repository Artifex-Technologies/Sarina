//! Go: `cryptotools/cryptotools.go`: `sarina crypto`, the reports of the other parts on the command line.

use super::{classic, decode, hash, identify, modern};
use crate::gotext::quote;

/// `sarina crypto identify TEXT | decode [ENC] TEXT | classic CIPHER [KEY|crack] TEXT | hash [ALGO] TEXT | modern TEXT`.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    if args.len() < 2 {
        env.eprintln(
            "usage: sarina crypto identify TEXT | decode [ENC] TEXT | classic CIPHER [KEY|crack] TEXT | hash [ALGO] TEXT | modern TEXT",
        );
        return 2;
    }
    let from = |i: usize| args[i..].join(" ");
    let res: Result<Vec<u8>, String> = match args[0].as_str() {
        "identify" => Ok(identify::identify_report(from(1).as_bytes(), "string")),
        "decode" => {
            if args.len() > 2 {
                decode::decode_one(&args[1], from(2).as_bytes())
            } else {
                Ok(decode::describe(decode::decode_auto(args[1].as_bytes(), 3).as_ref()))
            }
        }
        "classic" => if args.len() < 3 {
            Err("classic CIPHER [KEY|crack] TEXT".to_string())
        } else if args[2] == "crack" {
            classic::classic(&from(3), &args[1], "", false, true)
        } else if args.len() > 3 {
            classic::classic(&from(3), &args[1], &args[2], true, false)
        } else {
            classic::classic(&args[2], &args[1], "", false, false)
        }
        .map(String::into_bytes),
        "hash" => Ok(if args.len() > 2 {
            hash::hash_report(from(2).as_bytes(), "text", &args[1], "", "")
        } else {
            hash::hash_report(args[1].as_bytes(), "text", "sha256", "", "")
        }
        .into_bytes()),
        "modern" => modern::modern(&from(1), "", now()),
        other => Err(format!("unknown subcommand {}", quote(other))),
    };
    match res {
        Ok(mut out) => {
            out.push(b'\n');
            env.stdout.extend_from_slice(&out);
            env.flush();
            0
        }
        Err(e) => {
            env.eprintln(&format!("sarina crypto: {e}"));
            2
        }
    }
}

/// Go's `time.Now()` as `modern` takes it: (Unix seconds, nanoseconds).
pub fn now() -> (i64, u32) {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() as i64, d.subsec_nanos())
}
