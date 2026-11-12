//! Go: `go/llm`: the message types (types.go), the repair of damaged JSON (repair.go) and the normaliser for local
//! servers' quirks (normalize.go); the clients come in step 3 (docs/RUST_PORT.md).

pub mod normalize;
pub mod openai;
pub mod repair;
pub mod types;
