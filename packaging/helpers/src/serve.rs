//! `sarina-helpers serve --model-dir D`: the JSON-lines server (PROTOCOL.md, protocol 1) that
//! desktop-audit and chymaera-analyze start. One request per line, answered in order; EOF ends it (exit 0). The answers are
//! the Python's, byte for byte, but for the vectors' last bits when another ONNX Runtime build runs the model.
//!
//! Where the Python died, this answers instead (PROTOCOL.md: a bad request line never kills the server): a line that is
//! not UTF-8 (Python's stdin raised UnicodeDecodeError outside its loop) and a line over `MAX_LINE` bytes.

use crate::classify::{prototypes_fingerprint, Classifier};
use crate::embed::{cosine, EmbeddingModel};
use crate::pyjson::{self, PStr, V};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: i64 = 1;
/// A request line at most (the core's are a few kilobytes; its answers are capped at 64 MiB the same way).
pub const MAX_LINE: usize = 64 << 20;

const META_KEYS: [&str; 5] = ["deviceType", "protocol", "preset", "uid", "frequency"];

fn ok(fields: Vec<(&str, V)>) -> Vec<(PStr, V)> {
    let mut m = vec![(PStr::new("ok"), V::Bool(true))];
    m.extend(fields.into_iter().map(|(k, v)| (PStr::new(k), v)));
    m
}

fn fail(msg: &str) -> Vec<(PStr, V)> {
    vec![(PStr::new("ok"), V::Bool(false)), (PStr::new("error"), V::str(msg))]
}

fn vectors(vs: Vec<Vec<f64>>) -> V {
    V::Arr(vs.into_iter().map(|v| V::Arr(v.into_iter().map(V::Float).collect())).collect())
}

/// `json.loads(raw)` of a capture's `meta`, {} for anything that is not a JSON object (the Python's caught cases).
fn parse_meta(capture: &V) -> Result<Vec<(PStr, V)>, String> {
    let raw = match capture.get("meta") {
        Some(r) if r.truthy() => r,
        _ => return Ok(Vec::new()),
    };
    let V::Str(s) = raw else { return Ok(Vec::new()) }; // json.loads of a non-string: TypeError, caught
    match pyjson::loads(&s.s) {
        Ok(V::Obj(m)) => Ok(m),
        Ok(_) => Ok(Vec::new()),
        // a JSONDecodeError was caught; a RecursionError or the integer-digits ValueError was not
        Err(e) if e.0.contains(": line ") => Ok(Vec::new()),
        Err(e) => Err(e.0),
    }
}

/// The text embedded for one capture: name, type and the meta fields worth comparing.
pub fn capture_text(capture: &V) -> Result<PStr, String> {
    let meta = parse_meta(capture)?;
    let mut parts: Vec<PStr> =
        vec![capture.get("name").map(|v| v.py_str()).unwrap_or_default(), capture.get("type").map(|v| v.py_str()).unwrap_or_default()];
    for key in META_KEYS {
        if let Some((_, v)) = meta.iter().find(|(k, _)| k.s == key) {
            let s = v.py_str();
            parts.push(PStr { s: format!("{key}={}", s.s), lone: s.lone });
        }
    }
    let parts: Vec<PStr> = parts.into_iter().filter(|p| !p.s.is_empty()).collect();
    Ok(PStr { s: parts.iter().map(|p| p.s.as_str()).collect::<Vec<_>>().join(" "), lone: parts.iter().any(|p| p.lone) })
}

/// Greedy grouping: each ungrouped capture anchors a group of later ones within `threshold`; groups of two or more.
pub fn group_vectors(names: &[PStr], vectors: &[Vec<f64>], threshold: f64) -> Vec<Vec<PStr>> {
    let mut assigned = vec![false; names.len()];
    let mut groups = Vec::new();
    for i in 0..names.len() {
        if assigned[i] {
            continue;
        }
        let mut group = vec![names[i].clone()];
        assigned[i] = true;
        for j in i + 1..names.len() {
            if !assigned[j] && cosine(&vectors[i], &vectors[j]) >= threshold {
                group.push(names[j].clone());
                assigned[j] = true;
            }
        }
        if group.len() > 1 {
            groups.push(group);
        }
    }
    groups
}

/// Python's `float(x)` of a JSON number (bool included: it is an int there).
fn as_float(v: &V) -> Option<Result<f64, String>> {
    match v {
        V::Float(f) => Some(Ok(*f)),
        V::Bool(b) => Some(Ok(if *b { 1.0 } else { 0.0 })),
        V::Int(s) => {
            let f: f64 = s.parse().unwrap_or(f64::INFINITY);
            Some(if f.is_infinite() { Err("int too large to convert to float".into()) } else { Ok(f) })
        }
        _ => None,
    }
}

/// `handle(request, model, classifier)`: Err is an exception's text (answered without the request's id).
pub fn handle(req: &V, model: &mut EmbeddingModel, classifier: &Classifier) -> Result<Vec<(PStr, V)>, String> {
    let op = req.get("op");
    let is = |name: &str| matches!(op, Some(V::Str(s)) if s.s == name);
    if is("ping") {
        return Ok(ok(vec![("protocol", V::Int(PROTOCOL_VERSION.to_string())), ("prototypes", V::str(&prototypes_fingerprint()))]));
    }
    if is("embed") {
        let texts: Option<Vec<PStr>> = match req.get("texts") {
            Some(V::Arr(a)) => a.iter().map(|t| if let V::Str(s) = t { Some(s.clone()) } else { None }).collect(),
            _ => None,
        };
        let Some(texts) = texts else { return Ok(fail("embed needs texts: [string]")) };
        return Ok(ok(vec![("vectors", vectors(model.embed(&texts)?))]));
    }
    if is("classify") {
        let Some(V::Str(text)) = req.get("text") else { return Ok(fail("classify needs text: string")) };
        let (label, score) = classifier.classify(model, text)?;
        return Ok(ok(vec![("label", label.map(|l| V::str(&l)).unwrap_or(V::Null)), ("score", V::Float(score))]));
    }
    if is("group") {
        let threshold = req.get("threshold").cloned().unwrap_or(V::Float(0.75));
        let (Some(V::Arr(captures)), Some(t)) = (req.get("captures"), as_float(&threshold)) else {
            return Ok(fail("group needs captures: [object] and threshold: number"));
        };
        let threshold = t?;
        let captures: Vec<&V> = captures.iter().filter(|c| matches!(c, V::Obj(_))).collect();
        if captures.is_empty() {
            return Ok(ok(vec![("groups", V::Arr(Vec::new()))]));
        }
        let names: Vec<PStr> = captures.iter().map(|c| c.get("name").map(|v| v.py_str()).unwrap_or_else(|| PStr::new("?"))).collect();
        let texts = captures.iter().map(|c| capture_text(c)).collect::<Result<Vec<_>, _>>()?;
        let vs = model.embed(&texts)?;
        let groups = group_vectors(&names, &vs, threshold);
        return Ok(ok(vec![("groups", V::Arr(groups.into_iter().map(|g| V::Arr(g.into_iter().map(V::Str).collect())).collect()))]));
    }
    Ok(fail(&format!("unknown op: {}", op.unwrap_or(&V::Null).py_repr())))
}

/// Python's UnicodeDecodeError text for a line that is not UTF-8.
fn utf8_error(line: &[u8], e: std::str::Utf8Error) -> String {
    let pos = e.valid_up_to();
    let b = line[pos];
    let why = match e.error_len() {
        None => "unexpected end of data",
        Some(_) if (0x80..0xc2).contains(&b) || b >= 0xf5 => "invalid start byte",
        Some(_) => "invalid continuation byte",
    };
    format!("'utf-8' codec can't decode byte 0x{b:02x} in position {pos}: {why}")
}

/// One answer for one request line (None: a blank line, skipped).
pub fn answer(line: &[u8], model: &mut EmbeddingModel, classifier: &Classifier) -> Option<String> {
    let resp = match std::str::from_utf8(line) {
        Err(e) => fail(&utf8_error(line, e)),
        Ok(text) => {
            let text = sarina_voice::py::strip(text);
            if text.is_empty() {
                return None;
            }
            match pyjson::loads(text) {
                Err(e) => fail(&e.0),
                Ok(req @ V::Obj(_)) => match handle(&req, model, classifier) {
                    Ok(mut r) => {
                        if let Some(id) = req.get("id") {
                            r.push((PStr::new("id"), id.clone()));
                        }
                        r
                    }
                    Err(e) => fail(&e),
                },
                Ok(_) => fail("request must be an object"),
            }
        }
    };
    Some(pyjson::dumps(&V::Obj(resp)))
}

/// Reads up to and including the next newline, at most `max` bytes kept; Err(()) when the line was longer (the rest of it
/// is read and dropped).
fn read_line(r: &mut impl BufRead, max: usize, line: &mut Vec<u8>) -> std::io::Result<Result<bool, ()>> {
    line.clear();
    let mut too_long = false;
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(if too_long { Err(()) } else { Ok(!line.is_empty()) });
        }
        let (take, end) = match buf.iter().position(|&c| c == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        if !too_long {
            if line.len() + take > max + 1 {
                too_long = true;
                line.clear();
            } else {
                line.extend_from_slice(&buf[..take]);
            }
        }
        r.consume(take);
        if end {
            if too_long {
                return Ok(Err(()));
            }
            line.pop();
            return Ok(Ok(true));
        }
    }
}

/// The server: the model loaded (or one error line and exit 1), then one answer per request until EOF.
pub fn serve(model_dir: &str, stdin: &mut impl BufRead, stdout: &mut impl Write) -> i32 {
    let mut emit = |s: &str| stdout.write_all(s.as_bytes()).and_then(|_| stdout.write_all(b"\n")).and_then(|_| stdout.flush()).is_ok();
    let loaded = EmbeddingModel::load(model_dir).and_then(|mut m| Classifier::new(&mut m).map(|c| (m, c)));
    let (mut model, classifier) = match loaded {
        Ok(x) => x,
        Err(e) => {
            emit(&pyjson::dumps(&V::Obj(fail(&e))));
            return 1;
        }
    };
    let mut line = Vec::new();
    loop {
        match read_line(stdin, MAX_LINE, &mut line) {
            Err(_) | Ok(Ok(false)) => return 0,
            Ok(Err(())) => {
                if !emit(&pyjson::dumps(&V::Obj(fail(&format!("request line longer than {MAX_LINE} bytes"))))) {
                    return 1;
                }
            }
            Ok(Ok(true)) => {
                if let Some(a) = answer(&line, &mut model, &classifier) {
                    if !emit(&a) {
                        return 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_texts() {
        let c =
            pyjson::loads(r#"{"name":"a.nfc","type":"NFC","meta":"{\"uid\":12,\"preset\":null,\"x\":1,\"frequency\":4.33e8}"}"#).unwrap();
        assert_eq!(capture_text(&c).unwrap().s, "a.nfc NFC preset=None uid=12 frequency=433000000.0");
        let c = pyjson::loads(r#"{"name":["x",1.0,{"k":true}],"meta":"[1]"}"#).unwrap();
        assert_eq!(capture_text(&c).unwrap().s, "['x', 1.0, {'k': True}]");
        let c = pyjson::loads(r#"{"type":"","meta":"{bad"}"#).unwrap();
        assert_eq!(capture_text(&c).unwrap().s, "");
    }

    #[test]
    fn grouping() {
        let n: Vec<PStr> = ["a", "b", "c", "d"].iter().map(|s| PStr::new(*s)).collect();
        let v = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![1.0, 0.1], vec![0.0, 1.0]];
        let g = group_vectors(&n, &v, 0.9);
        let names: Vec<Vec<&str>> = g.iter().map(|g| g.iter().map(|p| p.s.as_str()).collect()).collect();
        assert_eq!(names, vec![vec!["a", "c"], vec!["b", "d"]]);
    }

    #[test]
    fn lines() {
        let mut r = std::io::Cursor::new(b"ab\ncdefgh\nij".to_vec());
        let mut l = Vec::new();
        assert_eq!(read_line(&mut r, 4, &mut l).unwrap(), Ok(true));
        assert_eq!(l, b"ab");
        assert_eq!(read_line(&mut r, 4, &mut l).unwrap(), Err(()));
        assert_eq!(read_line(&mut r, 4, &mut l).unwrap(), Ok(true));
        assert_eq!(l, b"ij");
        assert_eq!(read_line(&mut r, 4, &mut l).unwrap(), Ok(false));
        let bad: Vec<u8> = [b"ab".as_slice(), &[0xff]].concat();
        let bad = bad.as_slice();
        assert_eq!(
            utf8_error(bad, std::str::from_utf8(bad).unwrap_err()),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
    }
}
