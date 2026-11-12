//! Chatterbox Turbo (Resemble AI, MIT) from its official ONNX files: the port of `chatterbox_engine.py`.
//!
//! Four graphs: a speech encoder (the reference recording to conditioning, once), token embeddings, an autoregressive
//! language model with a KV cache (writes speech tokens), and a conditional decoder (speech tokens to a 24 kHz waveform).
//! The language model and the decoder are separate halves (`Writer`, `Decoder`), so the voice can decode one sentence while
//! it writes the tokens of the next.
//!
//! ONNX Runtime is loaded at run time from a library file (`ort`'s load-dynamic): by default the one in the voice's GPU
//! venv (the same build the Python used, with its CUDA provider beside it), else the system's (`onnxruntime-cpu`).

use crate::npy::{self, Generator};
use ort::session::{Session, SessionInputValue};
use ort::value::{DynTensor, DynValue, Tensor, TensorElementType};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokenizers::Tokenizer;

pub const START: i64 = 6561;
pub const STOP: i64 = 6562;
pub const SILENCE: i64 = 4299;
pub const EOT: u32 = 50256;
const KV_HEADS: i64 = 16;
const HEAD_DIM: i64 = 64;
pub const MAX_TOKENS: usize = 700;
const REPETITION_PENALTY: f32 = 1.2;
const TOP_K: usize = 1000;
const TOP_P: f64 = 0.95;

/// What went wrong while making a sentence. `Retry`: what the Python raised as a ValueError (invalid audio, probabilities
/// that do not add up), which it tried again with the next random draws; `Fatal`: anything else.
#[derive(Debug, Clone)]
pub enum Fail {
    Retry(String),
    Fatal(String),
}

impl Fail {
    pub fn message(&self) -> &str {
        match self {
            Fail::Retry(m) | Fail::Fatal(m) => m,
        }
    }
}

fn fatal<E: std::fmt::Display>(e: E) -> Fail {
    Fail::Fatal(e.to_string())
}

/// An error from running a graph, worded as Python's onnxruntime words it ("[ONNXRuntimeError] : 2 : INVALID_ARGUMENT : ...").
fn run_failed<R>(e: ort::Error<R>) -> Fail {
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
    Fail::Fatal(format!("[ONNXRuntimeError] : {} : {} : {}", n, name, e.message()))
}

// ---------------------------------------------------------------------------------------------------------------------------
// Loading ONNX Runtime

static ENV: OnceLock<Result<(ort::environment::Environment, PathBuf), String>> = OnceLock::new();

/// The library to load: $LABYRINTH_VOICE_ORT, else the voice venv's (the newest libonnxruntime.so.* under its
/// site-packages/onnxruntime/capi), else the system's.
pub fn library_path(venv: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("LABYRINTH_VOICE_ORT").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(capi) = site_packages(venv).map(|s| s.join("onnxruntime").join("capi")) {
        if let Ok(rd) = std::fs::read_dir(&capi) {
            let mut libs: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("libonnxruntime.so.")))
                .collect();
            libs.sort();
            if let Some(p) = libs.pop() {
                return p;
            }
        }
    }
    PathBuf::from("/usr/lib/libonnxruntime.so.1")
}

/// VENV/lib/python3.*/site-packages
fn site_packages(venv: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(venv.join("lib"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("python3")))
        .map(|p| p.join("site-packages"))
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    found.pop()
}

/// ONNX Runtime, loaded once per process.
fn environment(venv: &Path) -> Result<&'static ort::environment::Environment, String> {
    let r = ENV.get_or_init(|| {
        let lib = library_path(venv);
        let env = ort::init_from(&lib)
            .map_err(|e| format!("cannot load ONNX Runtime from {}: {}", lib.display(), e))?
            .with_name("sarina-voiced")
            .build()
            .map_err(|e| e.to_string())?;
        Ok((env, lib))
    });
    r.as_ref().map(|(e, _)| e).map_err(|e| e.clone())
}

/// The ONNX Runtime library this process loaded, if it did.
pub fn loaded_library() -> Option<PathBuf> {
    ENV.get().and_then(|r| r.as_ref().ok()).map(|(_, p)| p.clone())
}

/// What onnxruntime.preload_dlls() does before the CUDA provider is used: the CUDA and cuDNN libraries the venv's NVIDIA
/// packages carry, loaded in its order, so the provider finds them. A library that is not there is skipped (the system's
/// is used then, as in Python).
fn preload_cuda(venv: &Path) {
    let Some(sp) = site_packages(venv) else { return };
    let nv = sp.join("nvidia");
    let mut paths: Vec<PathBuf> = Vec::new();
    // CUDA 13 wheels put everything in nvidia/cu13/lib; CUDA 12 ones had one package per component
    let cu13 = nv.join("cu13").join("lib");
    if cu13.is_dir() {
        for n in ["libcublasLt.so.13", "libcublas.so.13", "libnvrtc.so.13", "libcurand.so.10", "libcufft.so.12", "libcudart.so.13"] {
            paths.push(cu13.join(n));
        }
    } else {
        for (d, n) in [
            ("cublas", "libcublasLt.so.12"),
            ("cublas", "libcublas.so.12"),
            ("cuda_nvrtc", "libnvrtc.so.12"),
            ("curand", "libcurand.so.10"),
            ("cufft", "libcufft.so.11"),
            ("cuda_runtime", "libcudart.so.12"),
        ] {
            paths.push(nv.join(d).join("lib").join(n));
        }
    }
    paths.push(nv.join("cudnn").join("lib").join("libcudnn.so.9"));
    for p in paths {
        if let Ok(c) = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()) {
            if p.is_file() {
                // the handle is kept for the life of the process, as ctypes.CDLL's is
                unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Tensors

fn tensor_f32(shape: Vec<i64>, data: Vec<f32>) -> Result<DynValue, Fail> {
    Ok(Tensor::from_array((shape, data)).map_err(fatal)?.into_dyn())
}

fn tensor_i64(shape: Vec<i64>, data: Vec<i64>) -> Result<DynValue, Fail> {
    Ok(Tensor::from_array((shape, data)).map_err(fatal)?.into_dyn())
}

/// (shape, data) of a float32 output
fn read_f32(v: &DynValue) -> Result<(Vec<i64>, Vec<f32>), Fail> {
    let (shape, data) = v.try_extract_tensor::<f32>().map_err(fatal)?;
    Ok((shape.iter().copied().collect(), data.to_vec()))
}

fn read_i64(v: &DynValue) -> Result<(Vec<i64>, Vec<i64>), Fail> {
    let (shape, data) = v.try_extract_tensor::<i64>().map_err(fatal)?;
    Ok((shape.iter().copied().collect(), data.to_vec()))
}

/// np.nan_to_num for float32
fn nan_to_num(v: &mut [f32]) {
    for x in v.iter_mut() {
        if x.is_nan() {
            *x = 0.0;
        } else if *x == f32::INFINITY {
            *x = f32::MAX;
        } else if *x == f32::NEG_INFINITY {
            *x = f32::MIN;
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Sampling

/// Temperature, top-k (1000) and top-p (0.95) sampling, with NumPy's arithmetic: `sample` in chatterbox_engine.py.
/// Err is what NumPy raised as a ValueError ("probabilities contain NaN", "... do not sum to 1").
pub fn sample(logits: &[f32], rng: &mut Generator, temperature: f64) -> Result<usize, &'static str> {
    let t = temperature as f32;
    let mut l: Vec<f32> = logits.iter().map(|&x| x / t).collect();
    // A NaN or +inf logit, or nothing above -inf, makes every probability NaN, and choice() refuses that before it draws.
    if l.iter().any(|x| x.is_nan() || *x == f32::INFINITY) || l.iter().all(|x| *x == f32::NEG_INFINITY) {
        return Err("probabilities contain NaN");
    }
    let n = l.len();
    if TOP_K < n {
        let mut s = l.clone();
        let k = n - TOP_K;
        s.select_nth_unstable_by(k, |a, b| a.total_cmp(b));
        let kth = s[k];
        for v in l.iter_mut() {
            if *v < kth {
                *v = f32::NEG_INFINITY;
            }
        }
    }
    let max = l.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut p: Vec<f32> = l.iter().map(|&x| npy::exp_f32(x - max)).collect();
    let s = npy::sum_f32(&p);
    for v in p.iter_mut() {
        *v /= s;
    }
    if p.iter().any(|x| x.is_nan()) {
        return Err("probabilities contain NaN");
    }
    // order = argsort(-p): largest first (equal ones in index order; NumPy's sort is not stable, which matters only when
    // equal probabilities straddle the top-p cut)
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| p[b].total_cmp(&p[a]));
    let mut cum = 0.0f32;
    let mut cut = n;
    for (i, &o) in order.iter().enumerate() {
        cum = if i == 0 { p[o] } else { cum + p[o] };
        if cum as f64 >= TOP_P {
            cut = i;
            break;
        }
    }
    let mut q = vec![0.0f32; n];
    for &o in &order[..(cut + 1).min(n)] {
        q[o] = p[o];
    }
    let s = npy::sum_f32(&q);
    for v in q.iter_mut() {
        *v /= s;
    }
    rng.choice_p(&q)
}

// ---------------------------------------------------------------------------------------------------------------------------
// The engine

/// Where the files are and how to run them.
#[derive(Clone, Debug)]
pub struct Config {
    /// the Chatterbox folder (onnx/, tokenizer.json)
    pub dir: PathBuf,
    /// fp16 (the default on the graphics card), fp32, q8 or q4/q4f16: the files' suffix
    pub dtype: String,
    pub gpu: bool,
    /// the venv whose ONNX Runtime and CUDA libraries are used
    pub venv: PathBuf,
}

/// The conditioning for the voice to copy, worked out once from the reference recording.
pub struct Voice {
    cond_emb: (Vec<i64>, Vec<f32>),
    prompt_token: Vec<i64>,
    speaker_embeddings: (Vec<i64>, Vec<f32>),
    speaker_features: (Vec<i64>, Vec<f32>),
}

/// The half that writes speech tokens: token embeddings, the language model, the tokenizer.
pub struct Writer {
    emb: Session,
    lm: Session,
    tok: Tokenizer,
    kv_names: Vec<String>,
    kv_types: Vec<TensorElementType>,
    present_names: Vec<String>,
}

/// The half that turns speech tokens into sound.
pub struct Decoder {
    dec: Session,
}

pub struct Engine {
    pub writer: Writer,
    pub decoder: Decoder,
    pub voice: Option<Arc<Voice>>,
    pub providers: Vec<&'static str>,
    pub load_seconds: f64,
    cfg: Config,
}

fn suffix(dtype: &str) -> String {
    match dtype {
        "fp32" => String::new(),
        "q8" => "_quantized".into(),
        d => format!("_{}", d),
    }
}

fn session(cfg: &Config, name: &str, dtype: &str, gpu: bool) -> Result<Session, String> {
    let env = environment(&cfg.venv)?;
    let path = cfg.dir.join("onnx").join(format!("{}{}.onnx", name, suffix(dtype)));
    let mut b = Session::builder(env).map_err(|e| e.to_string())?;
    if gpu {
        // the CUDA provider's defaults, as Python's providers=["CUDAExecutionProvider", "CPUExecutionProvider"] gives
        b = b.with_execution_providers([ort::ep::CUDA::default().build().error_on_failure()]).map_err(|e| e.to_string())?;
    }
    b.commit_from_file(&path).map_err(|e| format!("{}: {}", path.display(), e))
}

impl Engine {
    /// Loads the graphs (the speech encoder only later, in `set_voice`, since it runs once) and the tokenizer.
    pub fn load(cfg: &Config) -> Result<Engine, String> {
        let t = std::time::Instant::now();
        environment(&cfg.venv)?;
        if cfg.gpu {
            preload_cuda(&cfg.venv);
        }
        let d = cfg.dtype.as_str();
        let emb = session(cfg, "embed_tokens", d, cfg.gpu)?;
        let lm = session(cfg, "language_model", d, cfg.gpu)?;
        // The decoder in half precision overflows for some sentences (NaN audio, a loud pop), so it stays in full precision.
        let dec = session(cfg, "conditional_decoder", "fp32", cfg.gpu)?;
        let tok = Tokenizer::from_file(cfg.dir.join("tokenizer.json")).map_err(|e| format!("tokenizer.json: {}", e))?;
        let mut kv_names = Vec::new();
        let mut kv_types = Vec::new();
        for i in lm.inputs() {
            if i.name().contains("past_key_values") {
                kv_names.push(i.name().to_string());
                kv_types.push(i.dtype().tensor_type().unwrap_or(TensorElementType::Float32));
            }
        }
        // present.N.key/value in the outputs' order after logits, matched to the inputs by position as the Python did
        let present_names: Vec<String> = lm.outputs().iter().skip(1).map(|o| o.name().to_string()).collect();
        if present_names.len() != kv_names.len() {
            return Err(format!("language model: {} cache inputs but {} cache outputs", kv_names.len(), present_names.len()));
        }
        let providers = if cfg.gpu { vec!["CUDAExecutionProvider", "CPUExecutionProvider"] } else { vec!["CPUExecutionProvider"] };
        Ok(Engine {
            writer: Writer { emb, lm, tok, kv_names, kv_types, present_names },
            decoder: Decoder { dec },
            voice: None,
            providers,
            load_seconds: t.elapsed().as_secs_f64(),
            cfg: cfg.clone(),
        })
    }

    /// Works out the conditioning for the voice to copy (about 10 seconds of clean speech). The half-precision speech encoder
    /// gives NaN in some of its features, so when the full-precision file is there this one step runs in full precision on
    /// the processor; whatever NaN remains is zeroed. The session is dropped afterwards (it is never needed again).
    pub fn set_voice(&mut self, reference_wav: &str) -> Result<(), String> {
        let audio = crate::wav::load_audio(reference_wav)?;
        let full = self.cfg.dir.join("onnx").join("speech_encoder.onnx");
        let mut enc = if self.cfg.dtype != "fp32" && full.exists() {
            session(&self.cfg, "speech_encoder", "fp32", false)?
        } else {
            session(&self.cfg, "speech_encoder", &self.cfg.dtype, self.cfg.gpu)?
        };
        let n = audio.len() as i64;
        let input = tensor_f32(vec![1, n], audio).map_err(|f| f.message().to_string())?;
        let out = enc.run(ort::inputs![input]).map_err(|e| e.to_string())?;
        let m = |f: Fail| f.message().to_string();
        let mut cond_emb = read_f32(&out[0]).map_err(m)?;
        let prompt_token = read_i64(&out[1]).map_err(m)?.1;
        let mut speaker_embeddings = read_f32(&out[2]).map_err(m)?;
        let mut speaker_features = read_f32(&out[3]).map_err(m)?;
        nan_to_num(&mut cond_emb.1);
        nan_to_num(&mut speaker_embeddings.1);
        nan_to_num(&mut speaker_features.1);
        self.voice = Some(Arc::new(Voice { cond_emb, prompt_token, speaker_embeddings, speaker_features }));
        Ok(())
    }

    /// One sentence to a float32 waveform at 24 kHz, the two halves one after the other (as the Python did).
    pub fn speak(&mut self, text: &str, rng: &mut Generator, temperature: f64) -> Result<Vec<f32>, Fail> {
        let voice = self.voice.clone().ok_or_else(|| Fail::Fatal("set_voice first".into()))?;
        let tokens = self.writer.write(&voice, text, rng, temperature, &|| false)?;
        self.decoder.decode(&voice, &tokens)
    }
}

impl Writer {
    /// The speech tokens for one sentence. `stop` is asked between steps: true abandons the sentence (Fatal "stopped").
    pub fn write(
        &mut self,
        voice: &Voice,
        text: &str,
        rng: &mut Generator,
        temperature: f64,
        stop: &dyn Fn() -> bool,
    ) -> Result<Vec<i64>, Fail> {
        let enc = self.tok.encode(text, false).map_err(|e| Fail::Fatal(e.to_string()))?;
        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        ids.push(EOT as i64);
        let mut ids_shape = vec![1i64, ids.len() as i64];
        let mut tokens: Vec<i64> = vec![START];
        let mut past: Vec<DynValue> = Vec::new();
        let mut mask_len: i64 = 0;
        let mut pos: Vec<i64> = Vec::new();
        let allocator = ort::memory::Allocator::default();
        for i in 0..MAX_TOKENS {
            if stop() {
                return Err(Fail::Fatal("stopped".into()));
            }
            let ids_t = tensor_i64(ids_shape.clone(), ids.clone())?;
            let mut eo = self.emb.run(ort::inputs![ids_t]).map_err(run_failed)?;
            let mut embeds = eo.remove("inputs_embeds").ok_or_else(|| Fail::Fatal("embed_tokens: no inputs_embeds".into()))?;
            drop(eo);
            if i == 0 {
                let (shape, data) = read_f32(&embeds)?;
                let (cshape, cdata) = &voice.cond_emb;
                let n = cshape[1] + shape[1];
                let mut all = Vec::with_capacity(cdata.len() + data.len());
                all.extend_from_slice(cdata);
                all.extend_from_slice(&data);
                embeds = tensor_f32(vec![1, n, shape[2]], all)?;
                for ty in &self.kv_types {
                    past.push(DynTensor::new(&allocator, *ty, [1i64, KV_HEADS, 0, HEAD_DIM]).map_err(fatal)?.into_dyn());
                }
                mask_len = n;
                pos = (0..n).collect();
            }
            let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = Vec::with_capacity(3 + past.len());
            inputs.push((Cow::Borrowed("inputs_embeds"), embeds.into()));
            inputs.push((Cow::Borrowed("attention_mask"), tensor_i64(vec![1, mask_len], vec![1; mask_len as usize])?.into()));
            inputs.push((Cow::Borrowed("position_ids"), tensor_i64(vec![1, pos.len() as i64], pos.clone())?.into()));
            for (name, v) in self.kv_names.iter().zip(past.drain(..)) {
                inputs.push((Cow::Owned(name.clone()), v.into()));
            }
            let mut out = self.lm.run(inputs).map_err(run_failed)?;
            let logits_v = out.remove("logits").ok_or_else(|| Fail::Fatal("language model: no logits".into()))?;
            for name in &self.present_names {
                past.push(out.remove(name.as_str()).ok_or_else(|| Fail::Fatal(format!("language model: no {}", name)))?);
            }
            drop(out);
            let (lshape, ldata) = logits_v.try_extract_tensor::<f32>().map_err(fatal)?;
            let vocab = lshape[2] as usize;
            let seq = lshape[1] as usize;
            let mut logits: Vec<f32> = ldata[(seq - 1) * vocab..seq * vocab].to_vec();
            // the repetition penalty, once for each token said so far (START included)
            let mut seen = tokens.clone();
            seen.sort_unstable();
            seen.dedup();
            for &t in &seen {
                let v = &mut logits[t as usize];
                *v = if *v < 0.0 { *v * REPETITION_PENALTY } else { *v / REPETITION_PENALTY };
            }
            let next = sample(&logits, rng, temperature).map_err(|e| Fail::Retry(e.to_string()))? as i64;
            tokens.push(next);
            if next == STOP {
                break;
            }
            ids = vec![next];
            ids_shape = vec![1, 1];
            mask_len += 1;
            pos = vec![pos[pos.len() - 1] + 1];
        }
        // tokens[1:-1]: without START and the last token (STOP, or the 700th when it never stopped, as the Python had it)
        Ok(tokens[1..tokens.len() - 1].to_vec())
    }
}

impl Decoder {
    /// The waveform for the speech tokens: the reference's prompt tokens first and three silence tokens after, NaN refused
    /// (Retry), scaled down when it overshoots 0.95, and the digital silence at the end cut to 0.2 s.
    pub fn decode(&mut self, voice: &Voice, speech: &[i64]) -> Result<Vec<f32>, Fail> {
        let mut st: Vec<i64> = voice.prompt_token.clone();
        st.extend_from_slice(speech);
        st.extend_from_slice(&[SILENCE; 3]);
        let n = st.len() as i64;
        let (es, ed) = &voice.speaker_embeddings;
        let (fs, fd) = &voice.speaker_features;
        let out = self
            .dec
            .run(ort::inputs![
                "speech_tokens" => tensor_i64(vec![1, n], st)?,
                "speaker_embeddings" => tensor_f32(es.clone(), ed.clone())?,
                "speaker_features" => tensor_f32(fs.clone(), fd.clone())?,
            ])
            .map_err(run_failed)?;
        let (_, mut wav) = read_f32(&out[0])?;
        drop(out);
        if wav.iter().any(|x| !x.is_finite()) {
            return Err(Fail::Retry("the voice produced invalid audio".into()));
        }
        let peak = wav.iter().fold(None, |m: Option<f32>, &x| Some(m.map_or(x.abs(), |m| m.max(x.abs())))).unwrap_or(0.0);
        if peak > 0.95 {
            let f = (0.95f64 / peak as f64) as f32;
            for x in wav.iter_mut() {
                *x *= f;
            }
        }
        Ok(trim_silence(wav))
    }
}

/// Cuts digital silence from the end (the model likes to end with a long pause), keeping 0.2 s.
pub fn trim_silence(mut wav: Vec<f32>) -> Vec<f32> {
    const THRESH: f32 = 1e-3;
    if let Some(last) = wav.iter().rposition(|x| x.abs() > THRESH) {
        let end = wav.len().min(last + 4800);
        wav.truncate(end);
    }
    wav
}
