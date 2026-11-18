//! Text embeddings (Python's `embeddings.py`): a sentence-embedding model's ONNX export (`model.onnx` and `tokenizer.json`
//! in one folder, what `optimum-cli export onnx` writes) run on the processor, its token vectors mean-pooled over the real
//! tokens and L2-normalised. No model is ever downloaded; the operator gives the folder.
//!
//! The arithmetic is the Python's, step for step, so the same ONNX Runtime gives the same bits: the token vectors are
//! widened to f64 and summed row after row as NumPy's `sum(axis=0)` does, then divided by the token count; norms, dot
//! products and cosines add up with Python 3.12's `sum()` (Neumaier's compensated sum), not a plain loop.

use crate::ortrt;
use crate::pyjson::PStr;
use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, TensorElementType, ValueType};
use std::borrow::Cow;
use std::path::Path;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

/// The tokenizer's truncation, as the Python set it.
pub const MAX_LENGTH: usize = 256;

/// Python's `str(Path(p))`: separators collapsed (a leading "//" kept), "." parts and a trailing "/" dropped, "" is ".".
pub fn pathlib_str(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let root = if p.starts_with("//") && !p.starts_with("///") {
        "//"
    } else if p.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let out = format!("{root}{}", parts.join("/"));
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

/// `Path(dir) / name`, as str()
pub fn path_join(dir: &str, name: &str) -> String {
    let d = pathlib_str(dir);
    if d == "." {
        name.to_string()
    } else if d.ends_with('/') {
        format!("{d}{name}")
    } else {
        format!("{d}/{name}")
    }
}

/// Python 3.12's `sum()` of floats: starts from the int 0 (so the first item is `0 + x`), then Neumaier's compensated sum.
pub fn py_sum(it: impl IntoIterator<Item = f64>) -> f64 {
    let mut it = it.into_iter();
    let Some(first) = it.next() else { return 0.0 };
    let mut f = 0.0 + first;
    let mut c = 0.0f64;
    for x in it {
        let t = f + x;
        if f.abs() >= x.abs() {
            c += (f - t) + x;
        } else {
            c += (x - t) + f;
        }
        f = t;
    }
    if c != 0.0 && c.is_finite() {
        f += c;
    }
    f
}

pub fn normalize(v: Vec<f64>) -> Vec<f64> {
    let norm = py_sum(v.iter().map(|x| x * x)).sqrt();
    if norm == 0.0 {
        return v;
    }
    v.into_iter().map(|x| x / norm).collect()
}

/// `cosine_similarity` (the vectors are the same length: one model made them)
pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot = py_sum(a.iter().zip(b).map(|(x, y)| x * y));
    let na = py_sum(a.iter().map(|x| x * x)).sqrt();
    let nb = py_sum(b.iter().map(|y| y * y)).sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// `rank_by_similarity`: highest first, ties in their first order (Python's sort is stable, also reversed), then `[:top_k]`.
pub fn rank<T: Clone>(query: &[f64], candidates: &[(T, Vec<f64>)], top_k: Option<i64>) -> Vec<(T, f64)> {
    let mut scored: Vec<(T, f64)> = candidates.iter().map(|(l, v)| (l.clone(), cosine(query, v))).collect();
    // list.sort(key=..., reverse=True): reversing keeps equal items in order; NaN compares false both ways in Python, here
    // it sorts as equal to everything, which is the order Python's merge leaves an all-NaN list in
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(k) = top_k {
        let n = scored.len() as i64;
        let end = if k < 0 { (n + k).max(0) } else { k.min(n) };
        scored.truncate(end as usize);
    }
    scored
}

/// The Python's `TextEncodeInput must be ...`: a text holding a lone surrogate, which its tokenizer could not take.
pub const SURROGATE_ERROR: &str = "TextEncodeInput must be Union[TextInputSequence, Tuple[InputSequence, InputSequence]]";

pub struct EmbeddingModel {
    session: Session,
    tokenizer: Tokenizer,
    inputs: Vec<String>,
    optional: Vec<bool>,
}

impl EmbeddingModel {
    /// `EmbeddingModel(model_dir)`: the error is the text the Python's exception carried.
    pub fn load(model_dir: &str) -> Result<EmbeddingModel, String> {
        ortrt::environment()?;
        let model_path = path_join(model_dir, "model.onnx");
        let tokenizer_path = path_join(model_dir, "tokenizer.json");
        let shown = pathlib_str(model_dir);
        if !Path::new(&model_path).is_file() {
            return Err(format!("no model.onnx in {shown}"));
        }
        if !Path::new(&tokenizer_path).is_file() {
            return Err(format!("no tokenizer.json in {shown}"));
        }
        let session = ortrt::cpu_session(&model_path)?;
        let mut tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| e.to_string())?;
        tokenizer.with_truncation(Some(TruncationParams { max_length: MAX_LENGTH, ..Default::default() })).map_err(|e| e.to_string())?;
        tokenizer.with_padding(Some(PaddingParams::default()));
        let inputs = session.inputs().iter().map(|i| i.name().to_string()).collect();
        let optional = session.inputs().iter().map(|i| matches!(i.dtype(), ValueType::Optional(_))).collect();
        Ok(EmbeddingModel { session, tokenizer, inputs, optional })
    }

    /// One L2-normalised vector per text (`embed`).
    pub fn embed(&mut self, texts: &[PStr]) -> Result<Vec<Vec<f64>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if texts.iter().any(|t| t.lone) {
            return Err(SURROGATE_ERROR.into());
        }
        let encs = self.tokenizer.encode_batch(texts.iter().map(|t| t.s.as_str()).collect::<Vec<_>>(), true).map_err(|e| e.to_string())?;
        let batch = encs.len();
        let seq = encs.first().map(|e| e.get_ids().len()).unwrap_or(0);
        let flat = |f: &dyn Fn(&tokenizers::Encoding) -> &[u32]| -> Vec<i64> {
            encs.iter().flat_map(|e| f(e).iter().map(|&x| x as i64)).collect()
        };
        let ids = flat(&|e| e.get_ids());
        let mask = flat(&|e| e.get_attention_mask());
        let types = flat(&|e| e.get_type_ids());
        let mut feed: Vec<(&str, Vec<i64>)> = vec![("input_ids", ids), ("attention_mask", mask.clone())];
        if self.inputs.iter().any(|n| n == "token_type_ids") {
            feed.push(("token_type_ids", types));
        }
        feed.retain(|(n, _)| self.inputs.iter().any(|i| i == n));
        // the Python binding's own check, worded as it words it
        let missing: Vec<&String> = self
            .inputs
            .iter()
            .zip(&self.optional)
            .filter(|(n, opt)| !**opt && !feed.iter().any(|(f, _)| f == *n))
            .map(|(n, _)| n)
            .collect();
        if !missing.is_empty() {
            let list = |v: Vec<&str>| format!("[{}]", v.iter().map(|s| sarina_voice::py::str_repr(s)).collect::<Vec<_>>().join(", "));
            return Err(format!(
                "Required inputs ({}) are missing from input feed ({}).",
                list(missing.iter().map(|s| s.as_str()).collect()),
                list(feed.iter().map(|(n, _)| *n).collect())
            ));
        }
        let mut values: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = Vec::new();
        for (name, data) in feed {
            let t = Tensor::from_array((vec![batch as i64, seq as i64], data)).map_err(|e| e.to_string())?;
            values.push((Cow::Owned(name.to_string()), t.into_dyn().into()));
        }
        let out = self.session.run(values).map_err(ortrt::python_error)?;
        let first = &out[0];
        let (shape, data): (Vec<i64>, Vec<f64>) = match first.dtype() {
            ValueType::Tensor { ty: TensorElementType::Float32, .. } => {
                let (s, d) = first.try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
                (s.to_vec(), d.iter().map(|&x| x as f64).collect())
            }
            ValueType::Tensor { ty: TensorElementType::Float64, .. } => {
                let (s, d) = first.try_extract_tensor::<f64>().map_err(|e| e.to_string())?;
                (s.to_vec(), d.to_vec())
            }
            other => return Err(format!("the model's first output is {other}, not float token vectors")),
        };
        let (rows, hidden, per_row) = match shape.as_slice() {
            [b, s, h] if *b as usize == batch => (*s as usize, *h as usize, true),
            // a model that pools itself: (batch, hidden); the Python's NumPy broadcast the one vector over every token
            [b, h] if *b as usize == batch => (seq, *h as usize, false),
            _ => return Err(format!("the model's first output has shape {shape:?}, not (batch, tokens, hidden)")),
        };
        let mut vectors = Vec::with_capacity(batch);
        for i in 0..batch {
            let m = &mask[i * seq..(i + 1) * seq];
            let count: f64 = m.iter().map(|&x| x as f64).sum();
            if count == 0.0 {
                vectors.push(normalize(vec![0.0; hidden]));
                continue;
            }
            let row = |r: usize| -> &[f64] {
                let start = if per_row { (i * rows + r) * hidden } else { i * hidden };
                &data[start..start + hidden]
            };
            let mut acc: Vec<f64> = row(0).iter().map(|x| x * m[0] as f64).collect();
            for (r, &mr) in m.iter().enumerate().take(rows.min(seq)).skip(1) {
                let w = mr as f64;
                for (a, x) in acc.iter_mut().zip(row(r)) {
                    *a += x * w;
                }
            }
            vectors.push(normalize(acc.into_iter().map(|x| x / count).collect()));
        }
        Ok(vectors)
    }

    pub fn embed_one(&mut self, text: &PStr) -> Result<Vec<f64>, String> {
        Ok(self.embed(std::slice::from_ref(text))?.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathlib() {
        assert_eq!(pathlib_str(""), ".");
        assert_eq!(pathlib_str("a//b/"), "a/b");
        assert_eq!(pathlib_str("./a/./b"), "a/b");
        assert_eq!(pathlib_str("//x"), "//x");
        assert_eq!(pathlib_str("///x"), "/x");
        assert_eq!(pathlib_str("/"), "/");
        assert_eq!(path_join(".", "model.onnx"), "model.onnx");
        assert_eq!(path_join("/", "model.onnx"), "/model.onnx");
    }

    #[test]
    fn python_sum() {
        // Neumaier keeps what a plain loop loses
        assert_eq!(py_sum([1e16, 1.0, -1e16]), 1.0);
        assert_eq!(py_sum([]), 0.0);
        assert!(py_sum([-0.0]).is_sign_positive());
        assert_eq!(py_sum([0.1; 10]), 1.0);
    }

    #[test]
    fn ranking() {
        let c = vec![("a", vec![1.0, 0.0]), ("b", vec![0.0, 1.0]), ("c", vec![1.0, 0.0])];
        let r = rank(&[1.0, 0.0], &c, None);
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), ["a", "c", "b"]);
        assert_eq!(rank(&[1.0, 0.0], &c, Some(-1)).len(), 2);
        assert_eq!(rank(&[1.0, 0.0], &c, Some(0)).len(), 0);
        assert_eq!(rank(&[1.0, 0.0], &c, Some(99)).len(), 3);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }
}
