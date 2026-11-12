//! Go: `go/llm`: the message types (types.go), the repair of damaged JSON (repair.go) and the normaliser for local
//! servers' quirks (normalize.go), the OpenAI-compatible wire format and transport (client.go: `openai.rs`, `client.rs`), the Anthropic client
//! (`anthropic.rs`) and the context-window discovery (`discover.rs`).

pub mod anthropic;
pub mod client;
pub mod discover;
pub mod normalize;
pub mod openai;
pub mod repair;
pub mod types;
