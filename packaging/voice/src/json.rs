//! JSON read and written the way Python's json module does it: objects keep their order (a repeated key keeps its first place
//! and its last value), integers stay integers, floats are written as repr() writes them, and json.dumps escapes everything
//! outside printable ASCII (ensure_ascii) with ", " and ": " between items.

use crate::py;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// an integer, as its canonical decimal digits ("-0" is "0")
    Int(String),
    Float(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, k: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(kk, _)| kk == k).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Python truthiness
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(s) => s != "0",
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Arr(a) => !a.is_empty(),
            Value::Obj(o) => !o.is_empty(),
        }
    }

    /// dict[k] = v
    pub fn set(&mut self, k: &str, v: Value) {
        if let Value::Obj(m) = self {
            if let Some(e) = m.iter_mut().find(|(kk, _)| kk == k) {
                e.1 = v;
            } else {
                m.push((k.to_string(), v));
            }
        }
    }

    /// str(value), as "%s" % value formats it
    pub fn py_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            other => other.py_repr(),
        }
    }

    /// repr(value)
    pub fn py_repr(&self) -> String {
        match self {
            Value::Null => "None".into(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Int(s) => s.clone(),
            Value::Float(f) => py::float_repr(*f),
            Value::Str(s) => py::str_repr(s),
            Value::Arr(a) => format!("[{}]", a.iter().map(|v| v.py_repr()).collect::<Vec<_>>().join(", ")),
            Value::Obj(o) => {
                format!("{{{}}}", o.iter().map(|(k, v)| format!("{}: {}", py::str_repr(k), v.py_repr())).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// json.loads failed: Python's ValueError (JSONDecodeError), without the detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError;

/// json.loads(text); Err is a ValueError (JSONDecodeError).
pub fn loads(text: &str) -> Result<Value, DecodeError> {
    let b: Vec<char> = text.chars().collect();
    let mut p = Parser { s: &b, i: 0, depth: 0 };
    p.ws();
    let v = p.value().map_err(|()| DecodeError)?;
    p.ws();
    if p.i != b.len() {
        return Err(DecodeError);
    }
    Ok(v)
}

/// json.loads(bytes): UTF-8 (a leading byte-order mark is skipped).
pub fn loads_bytes(b: &[u8]) -> Result<Value, DecodeError> {
    let b = b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(b);
    let s = std::str::from_utf8(b).map_err(|_| DecodeError)?;
    loads(s)
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], ' ' | '\t' | '\n' | '\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn lit(&mut self, w: &str) -> bool {
        let n = w.chars().count();
        if self.i + n <= self.s.len() && self.s[self.i..self.i + n].iter().copied().eq(w.chars()) {
            self.i += n;
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Value, ()> {
        self.depth += 1;
        if self.depth > 900 {
            return Err(());
        }
        let r = match self.peek().ok_or(())? {
            '{' => self.object(),
            '[' => self.array(),
            '"' => {
                self.i += 1;
                self.string().map(Value::Str)
            }
            'n' if self.lit("null") => Ok(Value::Null),
            't' if self.lit("true") => Ok(Value::Bool(true)),
            'f' if self.lit("false") => Ok(Value::Bool(false)),
            'N' if self.lit("NaN") => Ok(Value::Float(f64::NAN)),
            'I' if self.lit("Infinity") => Ok(Value::Float(f64::INFINITY)),
            '-' if self.lit("-Infinity") => Ok(Value::Float(f64::NEG_INFINITY)),
            _ => self.number(),
        };
        self.depth -= 1;
        r
    }

    fn object(&mut self) -> Result<Value, ()> {
        self.i += 1;
        let mut m: Vec<(String, Value)> = Vec::new();
        self.ws();
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok(Value::Obj(m));
        }
        loop {
            self.ws();
            if self.peek() != Some('"') {
                return Err(());
            }
            self.i += 1;
            let k = self.string()?;
            self.ws();
            if self.peek() != Some(':') {
                return Err(());
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            if let Some(e) = m.iter_mut().find(|(kk, _)| *kk == k) {
                e.1 = v;
            } else {
                m.push((k, v));
            }
            self.ws();
            match self.peek() {
                Some(',') => {
                    self.i += 1;
                }
                Some('}') => {
                    self.i += 1;
                    return Ok(Value::Obj(m));
                }
                _ => return Err(()),
            }
        }
    }

    fn array(&mut self) -> Result<Value, ()> {
        self.i += 1;
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(']') {
            self.i += 1;
            return Ok(Value::Arr(a));
        }
        loop {
            self.ws();
            a.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(',') => {
                    self.i += 1;
                }
                Some(']') => {
                    self.i += 1;
                    return Ok(Value::Arr(a));
                }
                _ => return Err(()),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ()> {
        if self.i + 4 > self.s.len() {
            return Err(());
        }
        let h: String = self.s[self.i..self.i + 4].iter().collect();
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(());
        }
        self.i += 4;
        u32::from_str_radix(&h, 16).map_err(|_| ())
    }

    fn string(&mut self) -> Result<String, ()> {
        let mut out = String::new();
        loop {
            let c = self.peek().ok_or(())?;
            self.i += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let e = self.peek().ok_or(())?;
                    self.i += 1;
                    match e {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\x08'),
                        'f' => out.push('\x0c'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let u = self.hex4()?;
                            if (0xd800..0xdc00).contains(&u) && self.s.get(self.i) == Some(&'\\') && self.s.get(self.i + 1) == Some(&'u') {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    let cp = 0x10000 + ((u - 0xd800) << 10) + (lo - 0xdc00);
                                    out.push(char::from_u32(cp).ok_or(())?);
                                    continue;
                                }
                                self.i = save;
                            }
                            // a lone surrogate (Python keeps it; Rust strings cannot hold one)
                            out.push(char::from_u32(u).unwrap_or('\u{fffd}'));
                        }
                        _ => return Err(()),
                    }
                }
                c if (c as u32) < 0x20 => return Err(()),
                c => out.push(c),
            }
        }
    }

    fn number(&mut self) -> Result<Value, ()> {
        let start = self.i;
        let digit = |c: Option<char>| c.map(|c| c.is_ascii_digit()).unwrap_or(false);
        if self.peek() == Some('-') {
            self.i += 1;
        }
        if self.peek() == Some('0') {
            self.i += 1;
        } else if digit(self.peek()) {
            while digit(self.peek()) {
                self.i += 1;
            }
        } else {
            return Err(());
        }
        let int_end = self.i;
        let mut is_float = false;
        if self.peek() == Some('.') && digit(self.s.get(self.i + 1).copied()) {
            self.i += 1;
            while digit(self.peek()) {
                self.i += 1;
            }
            is_float = true;
        }
        if matches!(self.peek(), Some('e') | Some('E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.peek(), Some('+') | Some('-')) {
                self.i += 1;
            }
            if digit(self.peek()) {
                while digit(self.peek()) {
                    self.i += 1;
                }
                is_float = true;
            } else {
                self.i = save;
            }
        }
        let text: String = self.s[start..self.i].iter().collect();
        if is_float {
            return text.parse::<f64>().map(Value::Float).map_err(|_| ());
        }
        let _ = int_end;
        let neg = text.starts_with('-');
        let digits = text.trim_start_matches('-');
        if digits == "0" {
            return Ok(Value::Int("0".into()));
        }
        Ok(Value::Int(if neg { format!("-{}", digits) } else { digits.to_string() }))
    }
}

/// json.dumps(value)
pub fn dumps(v: &Value) -> String {
    let mut out = String::new();
    dump_into(v, &mut out);
    out
}

fn dump_into(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(s) => out.push_str(s),
        Value::Float(f) => {
            if f.is_nan() {
                out.push_str("NaN")
            } else if f.is_infinite() {
                out.push_str(if *f > 0.0 { "Infinity" } else { "-Infinity" })
            } else {
                out.push_str(&py::float_repr(*f))
            }
        }
        Value::Str(s) => dump_str(s, out),
        Value::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_into(x, out);
            }
            out.push(']');
        }
        Value::Obj(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                dump_str(k, out);
                out.push_str(": ");
                dump_into(x, out);
            }
            out.push('}');
        }
    }
}

pub fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
        }
    }
    out.push('"');
}

pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}
