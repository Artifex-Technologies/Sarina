//! Go: `json.Unmarshal` into a struct (Go 1.27, the v1 API on the v2 engine), the types described once as data (`Ty`):
//! unknown keys dropped, keys matched exactly first and else without regard to case (with Go's two special folds), null
//! leaving a value as it was and making a slice, map or pointer nil, a duplicate key read again into what is there, a map
//! element read into a fresh zero value, and the first type error kept while the rest is still read. Adapted from Heimdall's
//! `gostruct.rs` (the same Go behaviour), read side only, with maps and `any` added.

use crate::goany::Any;
use crate::gojson::{self, Error, Value};
use crate::gotime::Time;
use std::collections::BTreeMap;

/// A Go type, as far as JSON sees it.
pub enum Ty {
    Str,
    Bool,
    /// int / int64
    Int,
    Float,
    Time,
    /// `any`: what Go puts in an interface
    Any,
    Slice(&'static Ty),
    Ptr(&'static Ty),
    /// `map[string]T`
    Map(&'static Ty),
    /// The type as Go's `%v` names it (for an error about the whole value), the name the field paths start with ("" for
    /// an anonymous struct), and the fields.
    Struct(&'static str, &'static str, &'static [Field]),
}

/// One field of a struct: its JSON name and type.
pub struct Field(pub &'static str, pub &'static Ty);

/// A value of a `Ty`.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Str(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    Time(Time),
    Any(Any),
    /// None: nil
    Slice(Option<Vec<Val>>),
    Ptr(Option<Box<Val>>),
    Map(Option<BTreeMap<String, Val>>),
    Struct(Vec<Val>),
}

/// The zero value.
pub fn zero(t: &Ty) -> Val {
    match t {
        Ty::Str => Val::Str(String::new()),
        Ty::Bool => Val::Bool(false),
        Ty::Int => Val::Int(0),
        Ty::Float => Val::Float(0.0),
        Ty::Time => Val::Time(Time::default()),
        Ty::Any => Val::Any(Any::Null),
        Ty::Slice(_) => Val::Slice(None),
        Ty::Ptr(_) => Val::Ptr(None),
        Ty::Map(_) => Val::Map(None),
        Ty::Struct(_, _, fs) => Val::Struct(fs.iter().map(|f| zero(f.1)).collect()),
    }
}

fn type_name(t: &Ty) -> String {
    match t {
        Ty::Str => "string".into(),
        Ty::Bool => "bool".into(),
        Ty::Int => "int".into(),
        Ty::Float => "float64".into(),
        Ty::Time => "time.Time".into(),
        Ty::Any => "interface {}".into(),
        Ty::Slice(e) => format!("[]{}", type_name(e)),
        Ty::Ptr(e) => format!("*{}", type_name(e)),
        Ty::Map(e) => format!("map[string]{}", type_name(e)),
        Ty::Struct(n, _, _) => n.to_string(),
    }
}

/// The kind of a JSON value as Go's errors name it.
pub fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Whether an object's key names a struct field the way Go matches it without regard to case (ASCII folding plus Go's
/// two special folds: U+017F long s is s, U+212A Kelvin is k).
pub fn key_matches(key: &str, field: &str) -> bool {
    if key == field {
        return true;
    }
    let mut k = key.chars().map(|c| match c {
        '\u{17F}' => 's',
        '\u{212A}' => 'k',
        c => c.to_ascii_lowercase(),
    });
    let mut f = field.chars();
    loop {
        match (k.next(), f.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if a == b.to_ascii_lowercase() => {}
            _ => return false,
        }
    }
}

struct Ctx {
    top: &'static str,
    first: Option<Error>,
}

fn mismatch(v: &Value, t: &Ty, field: &str, top: &str) -> Error {
    let what = match v {
        // Go names the number only when it failed to read into a number type ("number 0.5" into int; "number" into string)
        Value::Number(n) if matches!(t, Ty::Int | Ty::Float) => format!("number {n}"),
        other => kind(other).to_string(),
    };
    let ty = type_name(t);
    if field.is_empty() {
        Error(format!("json: cannot unmarshal {what} into Go value of type {ty}"))
    } else if field.rsplit('.').next().is_some_and(|last| !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit())) {
        // a list's element: Go 1.27 leaves out "Go struct field" when the path ends at an index
        Error(format!("json: cannot unmarshal {what} into {top}.{field} of type {ty}"))
    } else {
        Error(format!("json: cannot unmarshal {what} into Go struct field {top}.{field} of type {ty}"))
    }
}

fn path(field: &str, part: &str) -> String {
    if field.is_empty() {
        part.to_string()
    } else {
        format!("{field}.{part}")
    }
}

fn read(t: &Ty, v: &Value, into: &mut Val, field: &str, cx: &mut Ctx) {
    let top = cx.top;
    macro_rules! fail {
        ($e:expr) => {{
            let e = $e;
            cx.first.get_or_insert(e);
        }};
    }
    match (t, v) {
        (Ty::Slice(_), Value::Null) => *into = Val::Slice(None),
        (Ty::Ptr(_), Value::Null) => *into = Val::Ptr(None),
        (Ty::Map(_), Value::Null) => *into = Val::Map(None),
        (Ty::Any, _) => {
            let (a, err) = gojson::to_any(v);
            if let Some(e) = err {
                fail!(e);
            }
            *into = Val::Any(a);
        }
        (_, Value::Null) => {}
        (Ty::Str, Value::String(s)) => *into = Val::Str(s.clone()),
        (Ty::Bool, Value::Bool(b)) => *into = Val::Bool(*b),
        (Ty::Int, Value::Number(n)) => match n.parse::<i64>() {
            Ok(x) => *into = Val::Int(x),
            Err(_) => fail!(mismatch(v, t, field, top)),
        },
        (Ty::Float, Value::Number(n)) => match crate::gotext::parse_float(n) {
            Ok(x) => *into = Val::Float(x),
            Err(_) => fail!(mismatch(v, t, field, top)),
        },
        (Ty::Time, Value::String(s)) => match Time::parse_rfc3339(s) {
            Ok(x) => *into = Val::Time(x),
            Err(e) => fail!(Error(e)),
        },
        (Ty::Slice(e), Value::Array(items)) => {
            // Go decodes into the elements already there and zeroes none of them
            let mut old = match std::mem::replace(into, Val::Slice(None)) {
                Val::Slice(Some(o)) => o,
                _ => Vec::new(),
            };
            old.truncate(items.len());
            let mut out = Vec::with_capacity(items.len());
            let mut old = old.into_iter();
            for (i, it) in items.iter().enumerate() {
                let mut x = old.next().unwrap_or_else(|| zero(e));
                read(e, it, &mut x, &path(field, &i.to_string()), cx);
                out.push(x);
            }
            *into = Val::Slice(Some(out));
        }
        (Ty::Ptr(e), _) => {
            let mut x = match std::mem::replace(into, Val::Ptr(None)) {
                Val::Ptr(Some(b)) => *b,
                _ => zero(e),
            };
            read(e, v, &mut x, field, cx);
            *into = Val::Ptr(Some(Box::new(x)));
        }
        (Ty::Map(e), Value::Object(members)) => {
            let mut m = match std::mem::replace(into, Val::Map(None)) {
                Val::Map(Some(m)) => m,
                _ => BTreeMap::new(),
            };
            for (k, val) in members {
                let mut x = zero(e);
                read(e, val, &mut x, &path(field, k), cx);
                m.insert(k.clone(), x);
            }
            *into = Val::Map(Some(m));
        }
        (Ty::Struct(_, _, fs), Value::Object(members)) => {
            let Val::Struct(vals) = into else { return };
            for (k, val) in members {
                let idx = fs.iter().position(|f| f.0 == k).or_else(|| fs.iter().position(|f| key_matches(k, f.0)));
                if let Some(i) = idx {
                    read(fs[i].1, val, &mut vals[i], &path(field, fs[i].0), cx);
                }
            }
        }
        _ => fail!(mismatch(v, t, field, top)),
    }
}

/// Reads a whole document into a zero value of `t` (Go's `json.Unmarshal`): the value read (as far as it read) and the
/// first error.
pub fn unmarshal(t: &Ty, b: &[u8]) -> (Val, Option<Error>) {
    let mut out = zero(t);
    match gojson::parse(b) {
        Err(e) => (out, Some(e)),
        Ok(v) => {
            let top = match t {
                Ty::Struct(_, short, _) => short,
                _ => "",
            };
            let mut cx = Ctx { top, first: None };
            read(t, &v, &mut out, "", &mut cx);
            (out, cx.first)
        }
    }
}

/// A struct field's value by its JSON name.
pub fn field<'a>(t: &Ty, v: &'a Val, name: &str) -> Option<&'a Val> {
    match (t, v) {
        (Ty::Struct(_, _, fs), Val::Struct(vals)) => fs.iter().position(|f| f.0 == name).map(|i| &vals[i]),
        _ => None,
    }
}

/// A string field's text ("" when absent).
pub fn str_field<'a>(t: &Ty, v: &'a Val, name: &str) -> &'a str {
    match field(t, v, name) {
        Some(Val::Str(s)) => s,
        _ => "",
    }
}
