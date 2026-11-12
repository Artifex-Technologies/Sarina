//! Go's `encoding/json` as Sarina's Go build has it: Go 1.27, where `GOEXPERIMENT=jsonv2` is on by default, so the v1 API
//! (`Unmarshal`, `Decoder`, `Marshal`, `Encoder`) runs on the v2 engine with v1's options. That is not v1 byte for byte:
//! a bad escape reads "invalid escape sequence `\x` in string" (v1: "invalid character 'x' in string escape code"), a number
//! cut short "invalid character 'x' in numeric literal" (v1 named the place: "after decimal point ..."), the depth limit
//! "exceeded max depth", and a number out of float64's range is read as 0 with the error, the rest of the document still
//! read ("json: cannot unmarshal number 1e400 into .a.0 of type float64", the path a JSON pointer with dots).
//!
//! The reader is Limbo's (`Limbo/rust/limbo/src/gojson.rs`, same owner and licence, itself started from Heimdall's
//! `rust/heimdall/src/gojson.rs`), changed to these texts and rules; the float writer is Heimdall's `write_f64`. Proved by
//! testdata/parity/gojson.json. Go's leniency is kept: a lone UTF-16 surrogate or a byte that is not UTF-8 inside a string
//! reads as U+FFFD, the last of duplicate keys wins.

use crate::goany::Any;
use crate::gotext;
use std::collections::BTreeMap;
use std::fmt;

/// A JSON document as read. Numbers keep their text, so each reader applies Go's rule for its own type.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    /// Members in document order, duplicates kept.
    Object(Vec<(String, Value)>),
}

impl Drop for Value {
    /// Without recursion: a value 10,000 levels deep would need megabytes of stack to drop part by part.
    fn drop(&mut self) {
        let mut parts = match self {
            Value::Array(items) => std::mem::take(items),
            Value::Object(members) => std::mem::take(members).into_iter().map(|(_, v)| v).collect(),
            _ => return,
        };
        while let Some(mut v) = parts.pop() {
            match &mut v {
                Value::Array(items) => parts.append(items),
                Value::Object(members) => parts.extend(std::mem::take(members).into_iter().map(|(_, v)| v)),
                _ => {}
            }
        }
    }
}

/// Why a document did not read or write: Go's error text.
#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Go's nesting limit.
const MAX_DEPTH: usize = 10_000;

const REPLACEMENT: char = char::REPLACEMENT_CHARACTER;

/// Go's `strconv.Quote` of bytes that may not be UTF-8 (each bad byte as `\x..`).
fn quote_bytes(b: &[u8]) -> String {
    let mut out = String::from("\"");
    let mut rest = b;
    while !rest.is_empty() {
        let (c, n) = gotext::decode_rune(rest);
        match c {
            Some(c) => {
                let q = gotext::quote(&c.to_string());
                out.push_str(&q[1..q.len() - 1]);
            }
            None => out.push_str(&format!("\\x{:02x}", rest[0])),
        }
        rest = &rest[n.max(1)..];
    }
    out.push('"');
    out
}

/// jsonwire's QuoteRune: the first character of `b` as Go's `strconv.QuoteRune` writes it, or `'\xff'` for a bad byte.
fn quote_rune(b: &[u8]) -> String {
    match gotext::decode_rune(b) {
        (Some('\''), _) => "'\\''".into(),
        (Some('"'), _) => "'\"'".into(),
        (Some(c), _) => {
            let q = gotext::quote(&c.to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
        (None, _) => format!("'\\x{:x}'", b.first().copied().unwrap_or(0)),
    }
}

/// jsonwire's InvalidTextError text for "escape sequence" (the raw bytes of a bad escape).
fn escape_error(what: &[u8]) -> String {
    let mut runes = 0;
    let mut needs_quote = false;
    let mut rest = what;
    while !rest.is_empty() {
        let (c, n) = gotext::decode_rune(rest);
        runes += 1;
        match c {
            Some(c) if c != '`' && c != REPLACEMENT && !gotext::is_space(c) && gotext::is_print(c) => {}
            _ => needs_quote = true,
        }
        rest = &rest[n.max(1)..];
    }
    let shown = if runes == 1 {
        quote_rune(what)
    } else if needs_quote {
        quote_bytes(what)
    } else {
        format!("`{}`", String::from_utf8_lossy(what))
    };
    format!("invalid escape sequence {shown} in string")
}

/// Go's `utf8.FullRune`: whether `b` starts with a whole character, or with bytes that can never become one.
fn full_rune(b: &[u8]) -> bool {
    let Some(&c) = b.first() else { return false };
    let (need, lo, hi) = match c {
        0xc2..=0xdf => (2, 0x80, 0xbf),
        0xe0 => (3, 0xa0, 0xbf),
        0xed => (3, 0x80, 0x9f),
        0xe1..=0xef => (3, 0x80, 0xbf),
        0xf0 => (4, 0x90, 0xbf),
        0xf4 => (4, 0x80, 0x8f),
        0xf1..=0xf3 => (4, 0x80, 0xbf),
        _ => return true, // ASCII, or a byte no character starts with
    };
    if b.len() >= need {
        return true;
    }
    if b.len() > 1 && !(lo..=hi).contains(&b[1]) {
        return true;
    }
    b.len() > 2 && !(0x80..=0xbf).contains(&b[2])
}

/// jsonwire's hasEscapedUTF16Prefix: whether `b` could be the start of a `\uXXXX` cut short.
fn escaped_utf16_prefix(b: &[u8]) -> bool {
    b.iter().enumerate().all(|(i, &c)| match i {
        0 => c == b'\\',
        1 => c == b'u',
        2..=5 => c.is_ascii_hexdigit(),
        _ => true,
    })
}

fn hex4(b: &[u8]) -> Option<u32> {
    if b.len() != 4 || !b.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok()
}

/// An array or object being read: its parts so far (and, for an object, the key of the value being read).
enum Frame {
    Array(Vec<Value>),
    Object(Vec<(String, Value)>, String),
}

enum Stop {
    /// A syntax error: Go's text.
    Syntax(String),
    /// The input ended inside the value.
    End,
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

impl<'a> Parser<'a> {
    /// An invalid character at `at`, in v1's words for the place.
    fn bad<T>(&self, at: usize, place: &str) -> Result<T, Stop> {
        Err(Stop::Syntax(format!("invalid character {} {place}", quote_rune(&self.b[at..]))))
    }

    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(|&c| is_ws(c)) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Result<u8, Stop> {
        self.b.get(self.i).copied().ok_or(Stop::End)
    }

    /// One value, without recursion (a document may nest 10,000 deep, as Go allows): the open arrays and objects are a stack.
    fn value(&mut self) -> Result<Value, Stop> {
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let mut done = match self.peek()? {
                b'{' => {
                    self.enter()?;
                    self.i += 1;
                    self.ws();
                    if self.peek()? == b'}' {
                        self.i += 1;
                        self.depth -= 1;
                        Value::Object(Vec::new())
                    } else {
                        let k = self.key()?;
                        stack.push(Frame::Object(Vec::new(), k));
                        continue;
                    }
                }
                b'[' => {
                    self.enter()?;
                    self.i += 1;
                    self.ws();
                    if self.peek()? == b']' {
                        self.i += 1;
                        self.depth -= 1;
                        Value::Array(Vec::new())
                    } else {
                        stack.push(Frame::Array(Vec::new()));
                        continue;
                    }
                }
                b'"' => Value::String(self.string()?),
                b't' => self.literal("true", Value::Bool(true))?,
                b'f' => self.literal("false", Value::Bool(false))?,
                b'n' => self.literal("null", Value::Null)?,
                b'-' | b'0'..=b'9' => self.number()?,
                _ => return self.bad(self.i, "looking for beginning of value"),
            };
            // a value is complete: add it to the innermost open container, closing those that end here
            loop {
                match stack.last_mut() {
                    None => return Ok(done),
                    Some(Frame::Array(items)) => {
                        items.push(done);
                        self.ws();
                        match self.peek()? {
                            b',' => {
                                self.i += 1;
                                self.ws();
                                break;
                            }
                            b']' => {
                                self.i += 1;
                                self.depth -= 1;
                                let Some(Frame::Array(items)) = stack.pop() else { unreachable!() };
                                done = Value::Array(items);
                            }
                            _ => return self.bad(self.i, "after array element"),
                        }
                    }
                    Some(Frame::Object(members, key)) => {
                        members.push((std::mem::take(key), done));
                        self.ws();
                        match self.peek()? {
                            b',' => {
                                self.i += 1;
                                *key = self.key()?;
                                break;
                            }
                            b'}' => {
                                self.i += 1;
                                self.depth -= 1;
                                let Some(Frame::Object(members, _)) = stack.pop() else { unreachable!() };
                                done = Value::Object(members);
                            }
                            _ => return self.bad(self.i, "after object key:value pair"),
                        }
                    }
                }
            }
        }
    }

    /// An object member's key and its colon, and the white space before its value.
    fn key(&mut self) -> Result<String, Stop> {
        self.ws();
        if self.peek()? != b'"' {
            return self.bad(self.i, "looking for beginning of object key string");
        }
        let k = self.string()?;
        self.ws();
        if self.peek()? != b':' {
            return self.bad(self.i, "after object key");
        }
        self.i += 1;
        self.ws();
        Ok(k)
    }

    fn literal(&mut self, lit: &str, v: Value) -> Result<Value, Stop> {
        for (k, &want) in lit.as_bytes().iter().enumerate() {
            match self.b.get(self.i + k) {
                None => return Err(Stop::End),
                Some(&c) if c != want => {
                    return self.bad(self.i + k, &format!("in literal {lit} (expecting '{}')", want as char));
                }
                Some(_) => {}
            }
        }
        self.i += lit.len();
        Ok(v)
    }

    fn enter(&mut self) -> Result<(), Stop> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Stop::Syntax("exceeded max depth".into()));
        }
        Ok(())
    }

    fn digits(&mut self) {
        while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
        }
    }

    /// A digit must be next (jsonwire's "in number (expecting digit)").
    fn digit(&mut self) -> Result<(), Stop> {
        if !self.peek()?.is_ascii_digit() {
            return self.bad(self.i, "in numeric literal");
        }
        self.digits();
        Ok(())
    }

    fn number(&mut self) -> Result<Value, Stop> {
        let start = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        if self.peek()? == b'0' {
            self.i += 1;
        } else {
            self.digit()?;
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            self.digit()?;
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            self.digit()?;
        }
        Ok(Value::Number(String::from_utf8_lossy(&self.b[start..self.i]).into_owned()))
    }

    /// A string at the current quote: checked as jsonwire's ConsumeString does (escapes, control characters, a character
    /// cut off by the end), read as AppendUnquote does (bad bytes and lone surrogates as U+FFFD).
    fn string(&mut self) -> Result<String, Stop> {
        let b = self.b;
        let mut n = self.i + 1;
        let mut out = String::new();
        loop {
            let run = n;
            while b.get(n).is_some_and(|&c| (b' '..0x80).contains(&c) && c != b'\\' && c != b'"') {
                n += 1;
            }
            out.push_str(std::str::from_utf8(&b[run..n]).unwrap_or_default());
            let Some(&c) = b.get(n) else { return Err(Stop::End) };
            if c == b'"' {
                self.i = n + 1;
                return Ok(out);
            }
            match gotext::decode_rune(&b[n..]) {
                (Some(ch), len) if len > 1 => {
                    out.push(ch);
                    n += len;
                }
                (Some('\\'), _) => {
                    let Some(&e) = b.get(n + 1) else { return Err(Stop::End) };
                    let simple = match e {
                        b'/' => Some('/'),
                        b'"' => Some('"'),
                        b'\\' => Some('\\'),
                        b'b' => Some('\u{8}'),
                        b'f' => Some('\u{c}'),
                        b'n' => Some('\n'),
                        b'r' => Some('\r'),
                        b't' => Some('\t'),
                        b'u' => None,
                        _ => return Err(Stop::Syntax(escape_error(&b[n..n + 2]))),
                    };
                    if let Some(ch) = simple {
                        out.push(ch);
                        n += 2;
                        continue;
                    }
                    if b.len() < n + 6 {
                        if escaped_utf16_prefix(&b[n..]) {
                            return Err(Stop::End);
                        }
                        return Err(Stop::Syntax(escape_error(&b[n..])));
                    }
                    let Some(v1) = hex4(&b[n + 2..n + 6]) else {
                        return Err(Stop::Syntax(escape_error(&b[n..n + 6])));
                    };
                    n += 6;
                    let mut ch = char::from_u32(v1);
                    if (0xD800..0xE000).contains(&v1) {
                        // a pair only when a high surrogate is followed by a low one; anything else is U+FFFD and what
                        // follows is read on its own
                        ch = None;
                        if v1 < 0xDC00 && b.get(n) == Some(&b'\\') && b.get(n + 1) == Some(&b'u') {
                            if let Some(v2) = b.get(n + 2..n + 6).and_then(hex4) {
                                if (0xDC00..0xE000).contains(&v2) {
                                    ch = char::from_u32(0x10000 + ((v1 - 0xD800) << 10) + (v2 - 0xDC00));
                                    n += 6;
                                }
                            }
                        }
                    }
                    out.push(ch.unwrap_or(REPLACEMENT));
                }
                (Some(_), _) => return self.bad(n, "in string"), // a control character
                (None, _) => {
                    if !full_rune(&b[n..]) {
                        return Err(Stop::End);
                    }
                    out.push(REPLACEMENT);
                    n += 1;
                }
            }
        }
    }
}

/// `json.Unmarshal`'s reading of a whole document (white space around it allowed, nothing else).
pub fn parse(b: &[u8]) -> Result<Value, Error> {
    let mut p = Parser { b, i: 0, depth: 0 };
    p.ws();
    let v = match p.value() {
        Ok(v) => v,
        Err(Stop::End) => return Err(Error("unexpected end of JSON input".into())),
        Err(Stop::Syntax(s)) => return Err(Error(s)),
    };
    p.ws();
    if p.i < b.len() {
        return Err(Error(format!("invalid character {} after top-level value", quote_rune(&b[p.i..]))));
    }
    Ok(v)
}

/// One `json.Decoder.Decode` over `b`: the first value (what follows it is not read) and `Decoder.More` afterwards (more
/// than white space follows that is not a closing bracket). An empty stream is io.EOF ("EOF"), one cut short
/// io.ErrUnexpectedEOF ("unexpected EOF"); after any other error More is true.
pub fn parse_first(b: &[u8]) -> (Result<Value, Error>, bool) {
    let mut p = Parser { b, i: 0, depth: 0 };
    p.ws();
    if p.i >= b.len() {
        return (Err(Error("EOF".into())), false);
    }
    let start = b[p.i];
    match p.value() {
        Ok(v) => {
            p.ws();
            let more = p.b.get(p.i).is_some_and(|&c| c != b']' && c != b'}');
            (Ok(v), more)
        }
        // after a failed Decode, More looks at the same first byte: a closing bracket there is "no more"
        Err(Stop::End) => (Err(Error("unexpected EOF".into())), true),
        Err(Stop::Syntax(s)) => (Err(Error(s)), !matches!(start, b']' | b'}')),
    }
}

// ---- reading into Go's `any` ----

/// The document as Go puts it in an `any`: float64 numbers, maps. A number out of float64's range reads as 0, and the first
/// such number is the error (the rest is still read, as Go does). Without recursion, as the parser.
pub fn to_any(v: &Value) -> (Any, Option<Error>) {
    enum Build<'v> {
        Array(std::slice::Iter<'v, Value>, Vec<Any>),
        Object(std::slice::Iter<'v, (String, Value)>, BTreeMap<String, Any>, String),
    }
    let mut first = None;
    let mut path: Vec<String> = Vec::new();
    let mut stack: Vec<Build> = Vec::new();
    let mut next = Some(v);
    loop {
        let mut done = None;
        match next.take() {
            Some(Value::Array(items)) => stack.push(Build::Array(items.iter(), Vec::with_capacity(items.len()))),
            Some(Value::Object(members)) => stack.push(Build::Object(members.iter(), BTreeMap::new(), String::new())),
            Some(scalar) => done = Some(scalar_any(scalar, &path, &mut first)),
            None => {}
        }
        loop {
            if let Some(a) = done.take() {
                match stack.last_mut() {
                    None => return (a, first),
                    Some(Build::Array(_, out)) => out.push(a),
                    Some(Build::Object(_, out, key)) => {
                        out.insert(std::mem::take(key), a);
                    }
                }
                path.pop();
            }
            match stack.last_mut() {
                None => unreachable!("a container was pushed or a value finished"),
                Some(Build::Array(items, out)) => match items.next() {
                    Some(c) => {
                        path.push(out.len().to_string());
                        next = Some(c);
                        break;
                    }
                    None => {
                        let Some(Build::Array(_, out)) = stack.pop() else { unreachable!() };
                        done = Some(Any::Array(out));
                    }
                },
                Some(Build::Object(members, _, key)) => match members.next() {
                    Some((k, c)) => {
                        key.clone_from(k);
                        path.push(k.replace('~', "~0").replace('/', "~1"));
                        next = Some(c);
                        break;
                    }
                    None => {
                        let Some(Build::Object(_, out, _)) = stack.pop() else { unreachable!() };
                        done = Some(Any::Object(out));
                    }
                },
            }
            // the root container is finished when the stack empties: `done` goes out at the top of this loop
            if stack.is_empty() {
                if let Some(a) = done.take() {
                    return (a, first);
                }
            }
        }
    }
}

/// `json.Unmarshal(b, &v)` with `v` an `any`: the value (Null when the document does not read) and the error.
pub fn unmarshal_any(b: &[u8]) -> (Any, Option<Error>) {
    match parse(b) {
        Ok(v) => to_any(&v),
        Err(e) => (Any::Null, Some(e)),
    }
}

fn scalar_any(v: &Value, path: &[String], first: &mut Option<Error>) -> Any {
    match v {
        Value::Bool(b) => Any::Bool(*b),
        Value::String(s) => Any::String(s.clone()),
        Value::Number(t) => match gotext::parse_float(t) {
            Ok(f) => Any::Number(f),
            Err(_) => {
                if first.is_none() {
                    *first = Some(Error(type_error(&format!("number {t}"), path, "float64")));
                }
                Any::Number(0.0)
            }
        },
        _ => Any::Null,
    }
}

/// Go 1.27's UnmarshalTypeError text: the path is the JSON pointer with dots for slashes; a path whose last part is all
/// digits is taken for a slice index and loses "Go struct field ".
fn type_error(what: &str, path: &[String], ty: &str) -> String {
    let field: String = path.iter().map(|p| format!("/{p}")).collect::<String>();
    let field = field.strip_prefix('/').unwrap_or(&field).replace('/', ".");
    if field.is_empty() {
        return format!("json: cannot unmarshal {what} into Go value of type {ty}");
    }
    let last = &field[field.rfind('.').map_or(0, |i| i + 1)..];
    let into = if !last.is_empty() && last.bytes().all(|c| c.is_ascii_digit()) { "" } else { "Go struct field " };
    format!("json: cannot unmarshal {what} into {into}.{field} of type {ty}")
}

// ---- writing, as Go's encoder ----

/// Appends `s` as Go's encoder writes a string: `\"`, `\\`, `\b \f \n \r \t`, other control characters as `\u00XX`,
/// U+2028 and U+2029 always escaped, `<`, `>` and `&` escaped only with HTML escaping on (Marshal; an Encoder after
/// `SetEscapeHTML(false)` leaves them).
pub fn write_str(out: &mut String, s: &str, html: bool) {
    use std::fmt::Write;
    out.push('"');
    let mut start = 0;
    for (i, c) in s.char_indices() {
        let esc: Option<&str> = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{8}' => Some("\\b"),
            '\u{c}' => Some("\\f"),
            _ => None,
        };
        let needs_u = (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' || (html && matches!(c, '<' | '>' | '&'));
        if esc.is_none() && !needs_u {
            continue;
        }
        out.push_str(&s[start..i]);
        match esc {
            Some(e) => out.push_str(e),
            None => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
        }
        start = i + c.len_utf8();
    }
    out.push_str(&s[start..]);
    out.push('"');
}

/// A float64 as Go's encoder writes it: the shortest text that reads back, in plain notation from 1e-6 up to (not
/// including) 1e21, else with an exponent (`1e+21`, `1e-7`); NaN and the infinities are Go's UnsupportedValueError.
pub fn write_f64(out: &mut String, f: f64) -> Result<(), Error> {
    if f.is_nan() || f.is_infinite() {
        return Err(Error(format!("json: unsupported value: {}", gotext::float_v(f))));
    }
    if f == 0.0 {
        out.push_str(if f.is_sign_negative() { "-0" } else { "0" });
        return Ok(());
    }
    let abs = f.abs();
    if (1e-6..1e21).contains(&abs) {
        out.push_str(&format!("{f}"));
        return Ok(());
    }
    let s = format!("{f:e}"); // "1e21", "1.5e-7"
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    out.push_str(mant);
    out.push('e');
    match exp.strip_prefix('-') {
        Some(d) => {
            out.push('-');
            out.push_str(d);
        }
        None => {
            out.push('+');
            if exp.len() < 2 {
                out.push('0');
            }
            out.push_str(exp);
        }
    }
    Ok(())
}

/// Writes an `any` compactly as Go does (an object's keys sorted, as Go sorts a map's), without recursion.
pub fn write_any(out: &mut String, v: &Any, html: bool) -> Result<(), Error> {
    enum Open<'a> {
        Array(std::slice::Iter<'a, Any>, bool),
        Object(std::collections::btree_map::Iter<'a, String, Any>, bool),
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut next = Some(v);
    loop {
        match next.take() {
            Some(Any::Null) => out.push_str("null"),
            Some(Any::Bool(b)) => out.push_str(if *b { "true" } else { "false" }),
            Some(Any::Number(f)) => write_f64(out, *f)?,
            Some(Any::Num(t)) => out.push_str(t),
            Some(Any::String(s)) => write_str(out, s, html),
            Some(Any::Array(items)) => {
                out.push('[');
                stack.push(Open::Array(items.iter(), true));
            }
            Some(Any::Object(m)) => {
                out.push('{');
                stack.push(Open::Object(m.iter(), true));
            }
            None => {}
        }
        loop {
            match stack.last_mut() {
                None => return Ok(()),
                Some(Open::Array(items, first)) => match items.next() {
                    Some(it) => {
                        if !std::mem::replace(first, false) {
                            out.push(',');
                        }
                        next = Some(it);
                        break;
                    }
                    None => {
                        out.push(']');
                        stack.pop();
                    }
                },
                Some(Open::Object(members, first)) => match members.next() {
                    Some((k, it)) => {
                        if !std::mem::replace(first, false) {
                            out.push(',');
                        }
                        write_str(out, k, html);
                        out.push(':');
                        next = Some(it);
                        break;
                    }
                    None => {
                        out.push('}');
                        stack.pop();
                    }
                },
            }
        }
    }
}

/// `json.Marshal(v)` of an `any`.
pub fn marshal_any(v: &Any) -> Result<String, Error> {
    let mut out = String::new();
    write_any(&mut out, v, true)?;
    Ok(out)
}

/// The layout an Encoder with `SetIndent("", indent)` gives compact JSON (Go's appendIndent): one member or element per
/// line, `": "` after a key, an empty object or array kept as `{}` or `[]`.
pub fn indent(compact: &str, indent: &str) -> String {
    let mut out = String::with_capacity(compact.len() * 2);
    let mut depth = 0usize;
    let mut need_indent = false;
    let mut in_str = false;
    let mut escaped = false;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(indent);
        }
    };
    for c in compact.chars() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if need_indent && c != '}' && c != ']' {
            need_indent = false;
            depth += 1;
            newline(&mut out, depth);
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '{' | '[' => {
                need_indent = true;
                out.push(c);
            }
            ',' => {
                out.push(c);
                newline(&mut out, depth);
            }
            ':' => out.push_str(": "),
            '}' | ']' => {
                if need_indent {
                    need_indent = false;
                } else {
                    depth = depth.saturating_sub(1);
                    newline(&mut out, depth);
                }
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// Every character past ASCII as a `\uXXXX` escape (a pair of them past U+FFFF), lowercase hex: what Sarina's
/// `session.asciiOnly` and `llm.escapeNonASCII` do to Go's encoder output.
pub fn ascii_only(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        let mut units = [0u16; 2];
        for u in c.encode_utf16(&mut units) {
            let _ = write!(out, "\\u{:04x}", u);
        }
    }
    out
}
