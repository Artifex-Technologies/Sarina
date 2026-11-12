//! Sarina's Go core (`go/`), ported to Rust piece by piece and proved against the Go code (docs/RUST_PORT.md). Each module
//! names the Go package or file it comes from in its first line.

pub mod fuzzing;
pub mod goany;
pub mod gopath;
pub mod gotext;
pub mod shellparse;
pub mod testguard;
pub mod tools;
