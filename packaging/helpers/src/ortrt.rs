//! ONNX Runtime, loaded at run time (as packaging/voiced does): `ORT_DYLIB_PATH` names the library, else the system's
//! `libonnxruntime.so.1` (Arch: onnxruntime-cpu) is found the way the dynamic loader finds libraries. Nothing of ONNX
//! Runtime is linked or downloaded at build time. Sessions run on the processor only, as the Python's
//! `providers=['CPUExecutionProvider']` did, with ONNX Runtime's default options (as Python's).

use ort::session::Session;
use std::sync::OnceLock;

/// The variable naming the library (ort's own name for it).
pub const LIB_ENV: &str = "ORT_DYLIB_PATH";
pub const DEFAULT_LIB: &str = "libonnxruntime.so.1";

static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

pub fn library() -> String {
    std::env::var(LIB_ENV).ok().filter(|p| !p.is_empty()).unwrap_or_else(|| DEFAULT_LIB.to_string())
}

/// ONNX Runtime, once per process. The error replaces the Python's "missing onnxruntime ..." (the Rust helper needs the
/// library, not a Python package).
pub fn environment() -> Result<&'static ort::environment::Environment, String> {
    ENV.get_or_init(|| {
        let lib = library();
        ort::init_from(&lib)
            .map_err(|e| format!("missing onnxruntime - install onnxruntime-cpu (Arch) or set {LIB_ENV} to libonnxruntime.so ({e})"))?
            .with_name("sarina-helpers")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| e.clone())
}

pub fn cpu_session(path: &str) -> Result<Session, String> {
    let env = environment()?;
    Session::builder(env).map_err(python_error)?.commit_from_file(path).map_err(python_error)
}

/// An ONNX Runtime error worded as Python's onnxruntime words it: "[ONNXRuntimeError] : 2 : INVALID_ARGUMENT : ...".
pub fn python_error<R>(e: ort::Error<R>) -> String {
    use ort::error::ErrorCode as C;
    let (n, name) = match e.code() {
        C::Ok => (0, "OK"),
        C::GenericFailure => (1, "FAIL"),
        C::InvalidArgument => (2, "INVALID_ARGUMENT"),
        C::NoSuchFile => (3, "NO_SUCHFILE"),
        C::NoModel => (4, "NO_MODEL"),
        C::EngineError => (5, "ENGINE_ERROR"),
        C::RuntimeException => (6, "RUNTIME_EXCEPTION"),
        C::InvalidProtobuf => (7, "INVALID_PROTOBUF"),
        C::ModelLoaded => (8, "MODEL_LOADED"),
        C::NotImplemented => (9, "NOT_IMPLEMENTED"),
        C::InvalidGraph => (10, "INVALID_GRAPH"),
        C::ExecutionProviderFailure => (11, "EP_FAIL"),
        _ => (1, "FAIL"),
    };
    format!("[ONNXRuntimeError] : {} : {} : {}", n, name, e.message())
}

/// ONNX Runtime's seed for the random numbers inside graphs (a Piper voice draws noise), through the training API's
/// SetSeed as packaging/voiced does; for the side-by-side checks only (Python's `onnxruntime.set_seed`).
pub fn set_seed(seed: i64) -> Result<(), String> {
    environment()?;
    let api = ort::api();
    let training = unsafe { (api.GetTrainingApi)(ort::sys::ORT_API_VERSION) };
    if training.is_null() {
        return Err("this ONNX Runtime has no training API, so no SetSeed".into());
    }
    let status = unsafe { ((*training).SetSeed)(seed) };
    if status.0.is_null() {
        Ok(())
    } else {
        Err("SetSeed failed".into())
    }
}
