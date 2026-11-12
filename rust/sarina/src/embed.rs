//! Go: `embed`: texts into vectors with a small local embedding model through Ollama (`POST /api/embed`,
//! nomic-embed-text by default), for the vault's recall by meaning and the search in long skills. The model stays loaded
//! for `KEEP_ALIVE` after its last use.
//!
//! As Go: the answer's status is not looked at (Ollama's `error` field says what went wrong), only its first JSON value is
//! read, and the body is read without a size limit (the server is the local Ollama).

use crate::gohttp;
use crate::gojson::{self, Obj};
use crate::gostruct::{self, Field, Ty, Val};
use std::time::Duration;

/// How long Ollama keeps the embedding model loaded after a request (the owner's decision D5, 2026-11-02).
pub const KEEP_ALIVE: &str = "30m";

/// Texts into vectors (Go's `Embedder`).
pub trait Embedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
    fn name(&self) -> String;
}

/// Ollama's embedding endpoint (Go's `ollamaEmbedder`).
#[derive(Clone, Debug, PartialEq)]
pub struct Ollama {
    pub base: String,
    pub model: String,
}

/// Go's `Default`: OLLAMA_HOST, else OPENAI_BASE_URL without its /v1, else 127.0.0.1:11434; SARINA_EMBED_MODEL, else
/// nomic-embed-text.
pub fn default_embedder(getenv: &dyn Fn(&str) -> String) -> Ollama {
    let mut base = getenv("OLLAMA_HOST");
    if base.is_empty() {
        let b = getenv("OPENAI_BASE_URL");
        let b = b.strip_suffix('/').unwrap_or(&b);
        base = b.strip_suffix("/v1").unwrap_or(b).to_string();
    }
    if base.is_empty() {
        base = "http://127.0.0.1:11434".into();
    }
    if !base.starts_with("http") {
        base = format!("http://{base}");
    }
    let mut model = getenv("SARINA_EMBED_MODEL");
    if model.is_empty() {
        model = "nomic-embed-text".into();
    }
    let base = base.strip_suffix('/').unwrap_or(&base).to_string();
    Ollama { base, model }
}

static ANSWER: Ty = Ty::Struct(
    r#"struct { Embeddings [][]float32 "json:\"embeddings\""; Error string "json:\"error\"" }"#,
    "",
    &[Field("embeddings", &Ty::Slice(&Ty::Slice(&Ty::Float32))), Field("error", &Ty::Str)],
);

impl Embedder for Ollama {
    fn name(&self) -> String {
        self.model.clone()
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let input: Vec<String> = texts.to_vec();
        let body = Obj::new(true).strs("input", &input).str("keep_alive", KEEP_ALIVE).str("model", &self.model).end().map_err(|e| e.0)?;
        let url = format!("{}/api/embed", self.base);
        let reply = gohttp::post(&url, "application/json", body.as_bytes(), Duration::from_secs(120), usize::MAX)?;
        // Go's Decoder reads the first value from the stream: what it needs before an error in the body counts
        let v = match gojson::parse_first(&reply.body).0 {
            Ok(v) => v,
            Err(e) => return Err(reply.body_err.filter(|_| e.0 == "unexpected EOF" || e.0 == "EOF").unwrap_or(e.0)),
        };
        let (out, err) = gostruct::read_value(&ANSWER, &v);
        if let Some(e) = err {
            return Err(e.0);
        }
        let error = gostruct::str_field(&ANSWER, &out, "error");
        if !error.is_empty() {
            return Err(error.to_string());
        }
        let vectors: Vec<Vec<f32>> = match gostruct::field(&ANSWER, &out, "embeddings") {
            Some(Val::Slice(Some(vs))) => vs
                .iter()
                .map(|v| match v {
                    Val::Slice(Some(xs)) => xs.iter().map(|x| if let Val::Float32(f) = x { *f } else { 0.0 }).collect(),
                    _ => Vec::new(),
                })
                .collect(),
            _ => Vec::new(),
        };
        if vectors.len() != texts.len() {
            return Err(format!("embedder returned {} vectors for {} texts", vectors.len(), texts.len()));
        }
        Ok(vectors)
    }
}
