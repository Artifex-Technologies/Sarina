//! Go: `go/agent`, the turn loop, ported in steps (docs/RUST_PORT.md): first the answer checks (`claims.go`) and the secret
//! redactor (`secrets.go`), pure functions of the answer and the task.

pub mod claims;
pub mod secrets;
