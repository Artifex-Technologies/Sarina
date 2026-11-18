//! `sarina-helpers <serve|memory-search|voice-chat> ...` (the Python's `python -m sarina_helpers ...`).

use sarina_helpers::argparse::{Opt, Outcome, Parser};
use std::io::Write;

const SERVE: Parser = Parser { prog: "sarina_helpers serve", opts: &[Opt::store("--model-dir").required()], positional: None };

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    match args.first().map(|s| s.as_str()) {
        Some("serve") => match SERVE.parse(&args[1..]) {
            Outcome::Ok(p) => {
                let dir = p.get("--model-dir").unwrap_or_default().to_string();
                sarina_helpers::serve::serve(&dir, &mut std::io::stdin().lock(), &mut out.lock())
            }
            Outcome::Help(h) => {
                let _ = out.write_all(h.as_bytes());
                0
            }
            Outcome::Error(e) => {
                let _ = err.write_all(e.as_bytes());
                2
            }
        },
        Some("memory-search") => sarina_helpers::memory_search::main(&args[1..], &mut out.lock(), &mut err.lock()),
        Some("voice-chat") => sarina_helpers::voice::main(&args[1..]),
        _ => {
            let _ = writeln!(err, "{}", sarina_helpers::USAGE);
            2
        }
    }
}

fn main() {
    // JSON nested 50,000 deep (pyjson::MAX_DEPTH) and its repr need more than the main thread's 8 MiB: a thread with room
    // (reserved, not used, until a request is that deep)
    let code = std::thread::Builder::new().stack_size(256 << 20).spawn(run).map(|h| h.join().unwrap_or(1)).unwrap_or_else(|_| run());
    std::process::exit(code);
}
