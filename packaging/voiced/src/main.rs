//! sarina-voiced: Sarina's voice service, the Rust port of labyrinth-voiced (see the crate README).

use sarina_voice::py::{self, Env};
use sarina_voiced::{client, server, voice};

const DEFAULT_CHECK: &str = "The backup finished at three, and nothing was skipped this time. The snapshot from this morning is \
                             still there if you need it. Your fan curve is back to normal, so the noise should stop.";
use std::path::PathBuf;
use std::sync::Arc;

const HELP: &str = "sarina-voiced (labyrinth-voiced): Sarina's voice service (a user service). Keeps the Chatterbox Turbo voice
ready and makes speech on request, one sentence at a time, so the first sentence can be played while the next is still
being made.

  talk to it:   a unix socket, $XDG_RUNTIME_DIR/labyrinth-sarina/voiced.sock (private to you), JSON lines
    {\"op\":\"speak\",\"text\":\"Hello. How are you?\"}  ->  {\"wav\":\"/run/user/1000/labyrinth-sarina/voiced-1.wav\",\"text\":\"Hello.\"} per sentence, then {\"done\":true}
                                                      (the caller plays and deletes each file)
    {\"op\":\"ping\"}    ->  {\"ok\":true,\"loaded\":false,\"engine\":\"chatterbox\"}
    {\"op\":\"warm\"}    ->  {\"ok\":true} at once; loads the models in the background (the panel sends it when you start to talk)
    {\"op\":\"unload\"}  ->  frees the graphics card memory now
  The models load on the first request (a few seconds) and are freed after 10 minutes without one (LABYRINTH_VOICE_IDLE
  seconds), so the card is free for games and for Sarina's language model.

  Where things are: models in ~/.local/share/labyrinth/tts/chatterbox/onnx (fp16 by default, LABYRINTH_VOICE_DTYPE), the
  voice to copy in ~/.local/share/labyrinth/tts/chatterbox/voice/sarina.wav (LABYRINTH_VOICE_REFERENCE; about 10 seconds
  of clean speech; it never leaves this computer). ONNX Runtime and the CUDA libraries come from the voice's venv,
  ~/.local/share/labyrinth/tts/gpu-venv (LABYRINTH_VOICE_VENV; LABYRINTH_VOICE_ORT names the library itself).
  With the graphics card the next sentence is written while one is decoded (after the first); on the processor they are made
  one after the other. LABYRINTH_VOICE_PIPELINE=1 or =0 decides instead.

  sarina-voiced check [TEXT]: asks the running voice service (this one or the Python one: the same socket) to say TEXT,
  plays nothing, and prints when each sentence arrived, how long it is, the time to the first one and the gaps a listener
  would have heard; each file is deleted.
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && (args[1] == "-h" || args[1] == "--help") {
        println!("{}", HELP);
        return;
    }
    let env = Env::from_process();
    let check = args.len() > 1 && args[1] == "check";
    let run = PathBuf::from(py::join(
        &match env.truthy("XDG_RUNTIME_DIR") {
            Some(d) => d.to_string(),
            None => format!("/run/user/{}", py::getuid()),
        },
        "labyrinth-sarina",
    ));
    let sock =
        env.get("LABYRINTH_VOICED_SOCKET").map(|s| s.to_string()).unwrap_or_else(|| run.join("voiced.sock").to_string_lossy().into_owned());
    if check {
        let text = if args.len() > 2 { args[2..].join(" ") } else { DEFAULT_CHECK.to_string() };
        std::process::exit(client::check(&sock, &text));
    }
    let dir = py::expanduser(env.get_or("LABYRINTH_VOICE_DIR", "~/.local/share/labyrinth/tts/chatterbox"), &env);
    let idle = match py::parse_float(env.get_or("LABYRINTH_VOICE_IDLE", "600")) {
        Some(v) => v,
        None => {
            eprintln!("voiced: LABYRINTH_VOICE_IDLE is not a number");
            std::process::exit(1);
        }
    };
    let reference =
        env.get("LABYRINTH_VOICE_REFERENCE").map(|s| s.to_string()).unwrap_or_else(|| py::join_all(&[&dir, "voice", "sarina.wav"]));
    let settings = voice::Settings {
        run,
        dir: PathBuf::from(&dir),
        dtype: env.get_or("LABYRINTH_VOICE_DTYPE", "fp16").to_string(),
        idle,
        reference,
        venv: PathBuf::from(py::expanduser(env.get_or("LABYRINTH_VOICE_VENV", "~/.local/share/labyrinth/tts/gpu-venv"), &env)),
        pipeline: match env.get("LABYRINTH_VOICE_PIPELINE") {
            Some("0") => Some(false),
            Some("1") => Some(true),
            _ => None,
        },
        seed: env.truthy("LABYRINTH_VOICE_SEED").and_then(|s| s.parse::<u128>().ok()),
        pronunciations: py::expanduser("~/.config/labyrinth/pronunciations", &env),
    };
    let service = Arc::new(voice::Service::new(settings));
    if let Err(e) = server::serve(service, &sock) {
        eprintln!("voiced: {}: {}", sock, e);
        std::process::exit(1);
    }
}
