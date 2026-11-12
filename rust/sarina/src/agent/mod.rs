//! Go: `go/agent`, the turn loop, ported in steps (docs/RUST_PORT.md): the answer checks (`claims.go`), the secret
//! redactor (`secrets.go`), then the pure helpers: limits, the system prompt, clearing and compacting without a model.

pub mod claims;
pub mod compact;
pub mod limits;
pub mod prompt;
pub mod secrets;

/// An event the loop emits (Go's `map[string]any`).
pub type Event = std::collections::BTreeMap<String, crate::goany::Any>;
