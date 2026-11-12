//! Go: `go/agent`, the turn loop (docs/RUST_PORT.md): the answer checks (`claims.go`), the secret redactor (`secrets.go`),
//! the pure helpers (limits, the system prompt, clearing and compacting without a model) and the loop itself (`run`:
//! model calls, tools, nudges, the claim check, budgets, notes and summaries, plan decisions, resume, sub-agents).

pub mod claims;
pub mod compact;
pub mod limits;
pub mod prompt;
pub mod run;
pub mod secrets;

/// An event the loop emits (Go's `map[string]any`).
pub type Event = std::collections::BTreeMap<String, crate::goany::Any>;
