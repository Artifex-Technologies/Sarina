//! JSON as the Python package read and wrote it: `json.loads` with CPython's own error messages and positions
//! ("Expecting ',' delimiter: line 1 column 7 (char 6)"), and `json.dumps(obj, separators=(',', ':'))` (every character
//! outside printable ASCII escaped, floats as `repr()` writes them, `NaN` and `Infinity` allowed). `py_str` is Python's
//! `str()` of a decoded value, which the capture grouping embeds.
//!
//! One difference, on purpose: Python keeps a lone UTF-16 surrogate (`"\ud800"`) in a string; a Rust string cannot hold
//! one, so it is read as U+FFFD and the string is marked (`PStr::lone`), which is enough to fail the tokenizer as Python's did.

use sarina_voice::py;

/// A decoded string; `lone` when an escape in it was a lone surrogate (read as U+FFFD).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PStr {
    pub s: String,
    pub lone: bool,
}

impl PStr {
    pub fn new(s: impl Into<String>) -> PStr {
        PStr { s: s.into(), lone: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Null,
    Bool(bool),
    /// an integer as its canonical decimal digits ("-0" is "0"); Python's int has no size limit
    Int(String),
    Float(f64),
    Str(PStr),
    Arr(Vec<V>),
    /// a dict: insertion order, a repeated key keeps its first place and its last value
    Obj(Vec<(PStr, V)>),
}

impl V {
    pub fn get(&self, k: &str) -> Option<&V> {
        match self {
            V::Obj(m) => m.iter().find(|(kk, _)| kk.s == k).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(s: &str) -> V {
        V::Str(PStr::new(s))
    }

    /// Python truthiness (`if not raw`)
    pub fn truthy(&self) -> bool {
        match self {
            V::Null => false,
            V::Bool(b) => *b,
            V::Int(s) => s != "0",
            V::Float(f) => *f != 0.0,
            V::Str(s) => !s.s.is_empty(),
            V::Arr(a) => !a.is_empty(),
            V::Obj(o) => !o.is_empty(),
        }
    }

    /// `str(value)`
    pub fn py_str(&self) -> PStr {
        match self {
            V::Str(s) => s.clone(),
            other => PStr::new(other.py_repr()),
        }
    }

    /// `repr(value)`
    pub fn py_repr(&self) -> String {
        match self {
            V::Null => "None".into(),
            V::Bool(true) => "True".into(),
            V::Bool(false) => "False".into(),
            V::Int(s) => s.clone(),
            V::Float(f) => py::float_repr(*f),
            V::Str(s) => py::str_repr(&s.s),
            V::Arr(a) => format!("[{}]", a.iter().map(|v| v.py_repr()).collect::<Vec<_>>().join(", ")),
            V::Obj(o) => {
                format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", py::str_repr(&k.s), v.py_repr())).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// How deep arrays and objects may nest. Python 3.14's limit is its C stack (8 MB: about 52,100 levels in a fresh
/// interpreter, a little less inside the helper's loop, then "Stack overflow (used N kB) while decoding ..."); this one is
/// fixed just below it and says "maximum recursion depth exceeded while decoding ...".
pub const MAX_DEPTH: usize = 50_000;

/// `str(exc)` of what `json.loads` raised.
#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);

fn decode_error(msg: &str, doc: &[char], pos: usize) -> Error {
    let lineno = doc[..pos].iter().filter(|&&c| c == '\n').count() + 1;
    let colno = match doc[..pos].iter().rposition(|&c| c == '\n') {
        Some(i) => pos - i,
        None => pos + 1,
    };
    Error(format!("{msg}: line {lineno} column {colno} (char {pos})"))
}

enum Fail {
    /// StopIteration(idx): the caller says "Expecting value" there
    Stop(usize),
    Err(Error),
}

/// `json.loads(text)` for a str.
pub fn loads(text: &str) -> Result<V, Error> {
    let s: Vec<char> = text.chars().collect();
    if s.first() == Some(&'\u{feff}') {
        return Err(decode_error("Unexpected UTF-8 BOM (decode using utf-8-sig)", &s, 0));
    }
    let ws = |mut i: usize| {
        while i < s.len() && matches!(s[i], ' ' | '\t' | '\n' | '\r') {
            i += 1;
        }
        i
    };
    let mut sc = Scanner { s: &s, depth: 0 };
    let start = ws(0);
    let (v, end) = match sc.scan_once(start) {
        Ok(r) => r,
        Err(Fail::Stop(i)) => return Err(decode_error("Expecting value", &s, i)),
        Err(Fail::Err(e)) => return Err(e),
    };
    let end = ws(end);
    if end != s.len() {
        return Err(decode_error("Extra data", &s, end));
    }
    Ok(v)
}

struct Scanner<'a> {
    s: &'a [char],
    depth: usize,
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

impl Scanner<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn err(&self, msg: &str, pos: usize) -> Fail {
        Fail::Err(decode_error(msg, self.s, pos))
    }

    fn lit(&self, idx: usize, w: &str) -> bool {
        let n = w.chars().count();
        // CPython checks idx + n - 1 < length, then the characters
        idx + n <= self.s.len() && self.s[idx..idx + n].iter().copied().eq(w.chars())
    }

    fn scan_once(&mut self, idx: usize) -> Result<(V, usize), Fail> {
        let Some(c) = self.at(idx) else { return Err(Fail::Stop(idx)) };
        match c {
            '"' => {
                let (st, end) = self.scanstring(idx + 1)?;
                return Ok((V::Str(st), end));
            }
            '{' | '[' => {
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    let what = if c == '{' { "object" } else { "array" };
                    return Err(Fail::Err(Error(format!(
                        "maximum recursion depth exceeded while decoding a JSON {what} from a unicode string"
                    ))));
                }
                let r = if c == '{' { self.object(idx + 1) } else { self.array(idx + 1) };
                self.depth -= 1;
                return r;
            }
            'n' if self.lit(idx, "null") => return Ok((V::Null, idx + 4)),
            't' if self.lit(idx, "true") => return Ok((V::Bool(true), idx + 4)),
            'f' if self.lit(idx, "false") => return Ok((V::Bool(false), idx + 5)),
            'N' if self.lit(idx, "NaN") => return Ok((V::Float(f64::NAN), idx + 3)),
            'I' if self.lit(idx, "Infinity") => return Ok((V::Float(f64::INFINITY), idx + 8)),
            '-' if self.lit(idx, "-Infinity") => return Ok((V::Float(f64::NEG_INFINITY), idx + 9)),
            _ => {}
        }
        self.number(idx)
    }

    fn number(&self, start: usize) -> Result<(V, usize), Fail> {
        let digit = |i: usize| self.at(i).is_some_and(|c| c.is_ascii_digit());
        let mut idx = start;
        if self.at(idx) == Some('-') {
            idx += 1;
            if idx >= self.s.len() {
                return Err(Fail::Stop(start));
            }
        }
        match self.at(idx) {
            Some('1'..='9') => {
                idx += 1;
                while digit(idx) {
                    idx += 1;
                }
            }
            Some('0') => idx += 1,
            _ => return Err(Fail::Stop(start)),
        }
        let mut is_float = false;
        if self.at(idx) == Some('.') && digit(idx + 1) {
            is_float = true;
            idx += 2;
            while digit(idx) {
                idx += 1;
            }
        }
        if matches!(self.at(idx), Some('e' | 'E')) {
            let e_start = idx;
            idx += 1;
            if matches!(self.at(idx), Some('-' | '+')) {
                idx += 1;
            }
            while digit(idx) {
                idx += 1;
            }
            if digit(idx - 1) {
                is_float = true;
            } else {
                idx = e_start;
            }
        }
        let text: String = self.s[start..idx].iter().collect();
        if is_float {
            // Python's float() of the text: correctly rounded, overflow is inf
            let f: f64 = text.parse().unwrap_or(f64::NAN);
            return Ok((V::Float(f), idx));
        }
        let neg = text.starts_with('-');
        let digits = text.trim_start_matches('-');
        if digits.len() > 4300 {
            return Err(Fail::Err(Error(format!(
                "Exceeds the limit (4300 digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                digits.len()
            ))));
        }
        let v = if digits == "0" || !neg { digits.to_string() } else { format!("-{digits}") };
        Ok((V::Int(v), idx))
    }

    fn hex(&self, from: usize) -> Option<u32> {
        let mut v = 0u32;
        for i in from..from + 4 {
            v = (v << 4) | self.at(i)?.to_digit(16)?;
        }
        Some(v)
    }

    /// CPython's scanstring_unicode: `end` is just after the opening quote.
    fn scanstring(&self, end: usize) -> Result<(PStr, usize), Fail> {
        let len = self.s.len();
        let begin = end - 1;
        let mut out = PStr::default();
        let mut end = end;
        loop {
            let mut next = end;
            let mut c = '\0';
            while next < len {
                c = self.s[next];
                if c == '"' || c == '\\' {
                    break;
                }
                if (c as u32) <= 0x1f {
                    return Err(self.err("Invalid control character at", next));
                }
                next += 1;
            }
            if next >= len || !(c == '"' || c == '\\') {
                return Err(self.err("Unterminated string starting at", begin));
            }
            out.s.extend(&self.s[end..next]);
            if c == '"' {
                return Ok((out, next + 1));
            }
            next += 1;
            if next == len {
                return Err(self.err("Unterminated string starting at", begin));
            }
            let e = self.s[next];
            if e != 'u' {
                end = next + 1;
                out.s.push(match e {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\x08',
                    'f' => '\x0c',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return Err(self.err("Invalid \\escape", end - 2)),
                });
                continue;
            }
            next += 1;
            end = next + 4;
            if end >= len {
                return Err(self.err("Invalid \\uXXXX escape", next - 1));
            }
            let Some(mut u) = self.hex(next) else { return Err(self.err("Invalid \\uXXXX escape", end - 5)) };
            let mut lone = (0xd800..0xe000).contains(&u);
            if (0xd800..0xdc00).contains(&u) && end + 6 < len && self.s[end] == '\\' && self.s[end + 1] == 'u' {
                let Some(u2) = self.hex(end + 2) else { return Err(self.err("Invalid \\uXXXX escape", end + 1)) };
                if (0xdc00..0xe000).contains(&u2) {
                    u = 0x10000 + ((u - 0xd800) << 10) + (u2 - 0xdc00);
                    lone = false;
                    end += 6;
                }
            }
            if lone {
                out.lone = true;
                out.s.push('\u{fffd}');
            } else {
                out.s.push(char::from_u32(u).unwrap_or('\u{fffd}'));
            }
        }
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while self.at(i).is_some_and(is_ws) {
            i += 1;
        }
        i
    }

    fn object(&mut self, idx: usize) -> Result<(V, usize), Fail> {
        let mut m: Vec<(PStr, V)> = Vec::new();
        let mut idx = self.skip_ws(idx);
        if self.at(idx) != Some('}') {
            loop {
                if self.at(idx) != Some('"') {
                    return Err(self.err("Expecting property name enclosed in double quotes", idx));
                }
                let (key, next) = self.scanstring(idx + 1)?;
                idx = self.skip_ws(next);
                if self.at(idx) != Some(':') {
                    return Err(self.err("Expecting ':' delimiter", idx));
                }
                idx = self.skip_ws(idx + 1);
                let (val, next) = self.scan_once(idx)?;
                match m.iter_mut().find(|(k, _)| k.s == key.s) {
                    Some(e) => e.1 = val,
                    None => m.push((key, val)),
                }
                idx = self.skip_ws(next);
                if self.at(idx) == Some('}') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return Err(self.err("Expecting ',' delimiter", idx));
                }
                let comma = idx;
                idx = self.skip_ws(idx + 1);
                if self.at(idx) == Some('}') {
                    return Err(self.err("Illegal trailing comma before end of object", comma));
                }
            }
        }
        Ok((V::Obj(m), idx + 1))
    }

    fn array(&mut self, idx: usize) -> Result<(V, usize), Fail> {
        let mut a = Vec::new();
        let mut idx = self.skip_ws(idx);
        if self.at(idx) != Some(']') {
            loop {
                let (val, next) = self.scan_once(idx)?;
                a.push(val);
                idx = self.skip_ws(next);
                if self.at(idx) == Some(']') {
                    break;
                }
                if self.at(idx) != Some(',') {
                    return Err(self.err("Expecting ',' delimiter", idx));
                }
                let comma = idx;
                idx = self.skip_ws(idx + 1);
                if self.at(idx) == Some(']') {
                    return Err(self.err("Illegal trailing comma before end of array", comma));
                }
            }
        }
        Ok((V::Arr(a), idx + 1))
    }
}

/// `json.dumps(v, separators=(',', ':'))`
pub fn dumps(v: &V) -> String {
    let mut out = String::new();
    dump_into(v, &mut out);
    out
}

pub fn dump_float(f: f64, out: &mut String) {
    if f.is_nan() {
        out.push_str("NaN")
    } else if f.is_infinite() {
        out.push_str(if f > 0.0 { "Infinity" } else { "-Infinity" })
    } else {
        out.push_str(&py::float_repr(f))
    }
}

fn dump_into(v: &V, out: &mut String) {
    match v {
        V::Null => out.push_str("null"),
        V::Bool(true) => out.push_str("true"),
        V::Bool(false) => out.push_str("false"),
        V::Int(s) => out.push_str(s),
        V::Float(f) => dump_float(*f, out),
        V::Str(s) => sarina_voice::json::dump_str(&s.s, out),
        V::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump_into(x, out);
            }
            out.push(']');
        }
        V::Obj(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                sarina_voice::json::dump_str(&k.s, out);
                out.push(':');
                dump_into(x, out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: &str) -> String {
        loads(s).unwrap_err().0
    }

    #[test]
    fn errors_as_python_words_them() {
        assert_eq!(e("\"abc"), "Unterminated string starting at: line 1 column 1 (char 0)");
        assert_eq!(e("\"a\\x\""), "Invalid \\escape: line 1 column 3 (char 2)");
        assert_eq!(e("\"\\u12\""), "Invalid \\uXXXX escape: line 1 column 3 (char 2)");
        assert_eq!(e("\"\\u12g4\""), "Invalid \\uXXXX escape: line 1 column 3 (char 2)");
        assert_eq!(e("\"\\ud800\\u12g4\""), "Invalid \\uXXXX escape: line 1 column 9 (char 8)");
        assert_eq!(e("\"\\ud800\\udc0\""), "Invalid \\uXXXX escape: line 1 column 9 (char 8)");
        assert_eq!(e("\"\x01\""), "Invalid control character at: line 1 column 2 (char 1)");
        assert_eq!(e("[1,]"), "Illegal trailing comma before end of array: line 1 column 3 (char 2)");
        assert_eq!(e("[1 2]"), "Expecting ',' delimiter: line 1 column 4 (char 3)");
        assert_eq!(e("{\"a\" 1}"), "Expecting ':' delimiter: line 1 column 6 (char 5)");
        assert_eq!(e("{1:2}"), "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)");
        assert_eq!(e("["), "Expecting value: line 1 column 2 (char 1)");
        assert_eq!(e("{\"a\":}"), "Expecting value: line 1 column 6 (char 5)");
        assert_eq!(e("-"), "Expecting value: line 1 column 1 (char 0)");
        assert_eq!(e("01"), "Extra data: line 1 column 2 (char 1)");
        assert_eq!(e("1e+"), "Extra data: line 1 column 2 (char 1)");
        assert_eq!(e("tru"), "Expecting value: line 1 column 1 (char 0)");
        assert_eq!(e("-Inf"), "Expecting value: line 1 column 1 (char 0)");
        assert_eq!(e("{\"a\":1"), "Expecting ',' delimiter: line 1 column 7 (char 6)");
        assert_eq!(e("\u{feff}{}"), "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)");
        assert_eq!(e("\"abc\\"), "Unterminated string starting at: line 1 column 1 (char 0)");
        assert_eq!(e(" "), "Expecting value: line 1 column 2 (char 1)");
        assert!(e(&"9".repeat(4301)).starts_with("Exceeds the limit (4300 digits)"));
    }

    #[test]
    fn values_and_output() {
        let v = loads("{\"a\":[1.5e-7,1e400,true,-0,\"\\ud83d\\ude00\",null],\"a\":2,\"b\":\"\\ud800\"}").unwrap();
        assert_eq!(dumps(&v), "{\"a\":2,\"b\":\"\\ufffd\"}");
        let v = loads("[1.5e-7,1e400,true,-0,\"\\ud83d\\ude00\",null,NaN,-0.0]").unwrap();
        assert_eq!(dumps(&v), "[1.5e-07,Infinity,true,0,\"\\ud83d\\ude00\",null,NaN,-0.0]");
        assert_eq!(v.py_repr(), "[1.5e-07, inf, True, 0, '\u{1f600}', None, nan, -0.0]");
        // as deep as the binary's thread allows (main.rs)
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(|| {
                let deep = "[".repeat(MAX_DEPTH + 1);
                assert!(e(&deep).starts_with("maximum recursion depth exceeded while decoding a JSON array"));
                let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
                assert_eq!(dumps(&loads(&ok).unwrap()), ok);
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
