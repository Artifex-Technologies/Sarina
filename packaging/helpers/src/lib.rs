//! Sarina's ML and voice helper in Rust: the port of the Python package `sarina_helpers` (PROTOCOL.md). One binary, `sarina-helpers`,
//! with the Python module's three modes and the same answers:
//!
//! - `serve --model-dir D`: the JSON-lines embedding server desktop-audit and chymaera-analyze start (`serve`)
//! - `memory-search ...`: semantic search over saved sessions (`memory_search`)
//! - `voice-chat ...`: push-to-talk voice chat driving the agent one turn at a time (`voice`)
//!
//! ONNX Runtime is loaded at run time (`ortrt`); speech to text is whisper.cpp's `whisper-cli` with a ggml model, text to
//! speech is a Piper voice run here (espeak-ng's library for the phonemes). docs/RUST_PORT.md "The ML and voice helper".

pub mod argparse;
pub mod classify;
pub mod embed;
pub mod memory_search;
pub mod ortrt;
pub mod pyjson;
pub mod serve;
pub mod voice;

/// What the module printed when run without a mode (exit 2); Sarina's core starts the helper that way to see it runs.
pub const USAGE: &str = "usage: sarina-helpers {serve|memory-search|voice-chat} ...";
