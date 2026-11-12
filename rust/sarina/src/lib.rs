//! Sarina's Go core (`go/`), ported to Rust piece by piece and proved against the Go code (docs/RUST_PORT.md). Each module
//! names the Go package or file it comes from in its first line.

pub mod flagx;
pub mod fuzzing;
pub mod goany;
pub mod goerr;
pub mod goflag;
pub mod gojson;
pub mod gopath;
pub mod goregexp;
pub mod gotext;
pub mod pyfmt;
pub mod shellparse;
pub mod testguard;
pub mod tools;
pub mod version;
