//! Sarina's voice service in Rust (`sarina-voiced`), a port of Labyrinth OS's `labyrinth-voiced` and `chatterbox_engine.py`.
//!
//! - `text`: the words as the voice says them (cleaning, sentences, delivery tags, pronunciations);
//! - `npy`: the parts of NumPy the sampling depends on, so a seed gives the Python's speech tokens;
//! - `engine`: Chatterbox Turbo through ONNX Runtime;
//! - `wav`: the reference recording in, each sentence out;
//! - `voice`: one request, sentence by sentence, the next one written while the last is decoded;
//! - `server`: the socket.
pub mod engine;
pub mod npy;
pub mod server;
pub mod text;
pub mod voice;
pub mod wav;
