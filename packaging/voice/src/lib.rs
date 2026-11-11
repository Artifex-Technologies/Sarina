//! Sarina's spoken-question programs, ported from Python to Rust (docs/VOICE_RUST_PORT.md):
//!
//! - `ask`: labyrinth-ask-sarina, the conversation (Super+S, the crash watch's "Ask Sarina", her evals)
//! - `checkup`: labyrinth-sarina-checkup, the daily check-up and the 10-minute look at what Heimdall holds
//! - `speak_voiced`: labyrinth-sarina-speak-voiced, the client of the voice service (labyrinth-voiced, which stays Python)
//!
//! One binary (`sarina-voice`) answers to each program's name; `py`, `rx`, `json`, `proc` and `http` hold the parts of Python's
//! behaviour the ports depend on, so their output stays byte for byte what the Python scripts printed and wrote.

pub mod argparse;
pub mod ask;
pub mod checkup;
pub mod http;
pub mod json;
pub mod proc;
pub mod py;
pub mod rx;
pub mod sha1;
pub mod speak_voiced;

/// Which program a name (argv[0]'s file name, or the first argument of `sarina-voice`) means.
pub fn program(name: &str) -> Option<&'static str> {
    Some(match name {
        "labyrinth-ask-sarina" | "sarina-converse" | "ask-sarina" => "ask",
        "labyrinth-sarina-checkup" | "sarina-checkup" | "checkup" => "checkup",
        "labyrinth-sarina-speak-voiced" | "sarina-speak-voiced" | "speak-voiced" => "speak-voiced",
        _ => return None,
    })
}
