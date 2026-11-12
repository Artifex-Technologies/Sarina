//! Go: `go/chymaera/internal/pyfmt`. The few Python rendering and truthiness rules the Chymaera ports keep, so report text
//! stays byte-compatible with the original Python (str(True) is "True", repr of a string uses single quotes, ...). A float64
//! is written as Go writes it (`1`, not Python's `1.0`): the Go code does that, and the port follows the Go code.

use crate::goany::Any;
use crate::gotext::{float_v, parse_float};

/// Python's bool(v) for JSON-decoded values.
pub fn truthy(v: &Any) -> bool {
    match v {
        Any::Null => false,
        Any::Bool(b) => *b,
        Any::String(s) => !s.is_empty(),
        // Go: json.Number.Float64 failing (out of range) counts as true
        Any::Num(n) => !parse_float(n).is_ok_and(|f| f == 0.0),
        Any::Number(f) => *f != 0.0,
        Any::Array(a) => !a.is_empty(),
        Any::Object(m) => !m.is_empty(),
    }
}

/// Python's str(v).
pub fn str(v: &Any) -> String {
    match v {
        Any::String(s) => s.clone(),
        _ => repr(v),
    }
}

/// Python's repr(v) for JSON-decoded values.
pub fn repr(v: &Any) -> String {
    match v {
        Any::Null => "None".into(),
        Any::Bool(true) => "True".into(),
        Any::Bool(false) => "False".into(),
        Any::String(s) => repr_string(s),
        Any::Num(n) => n.clone(),
        Any::Number(f) => float_v(*f),
        Any::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Any::Object(m) => {
            format!("{{{}}}", m.iter().map(|(k, x)| format!("{}: {}", repr_string(k), repr(x))).collect::<Vec<_>>().join(", "))
        }
    }
}

/// Python's type(v).__name__ for JSON-decoded values. A float64 is "object", as the Go code has no case for it.
pub fn type_name(v: &Any) -> &'static str {
    match v {
        Any::Null => "NoneType",
        Any::Bool(_) => "bool",
        Any::String(_) => "str",
        Any::Num(n) if n.contains(['.', 'e', 'E']) => "float",
        Any::Num(_) => "int",
        Any::Array(_) => "list",
        Any::Object(_) => "dict",
        Any::Number(_) => "object",
    }
}

/// Python's repr(s) for a string: single quotes unless the text holds one and no double quote.
pub fn repr_string(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut b = String::with_capacity(s.len() + 2);
    b.push(quote);
    for r in s.chars() {
        match r {
            '\\' => b.push_str("\\\\"),
            c if c == quote => {
                b.push('\\');
                b.push(c);
            }
            '\n' => b.push_str("\\n"),
            '\r' => b.push_str("\\r"),
            '\t' => b.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => b.push_str(&format!("\\x{:02x}", c as u32)),
            c => b.push(c),
        }
    }
    b.push(quote);
    b
}
