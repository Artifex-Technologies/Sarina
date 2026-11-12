//! Go's `flag` package as Sarina's CLI uses it (`flag.NewFlagSet(name, flag.ContinueOnError)`): the same syntax (`-x`, `--x`,
//! `-x=v`, `-x v`, `--` ending the flags), the same messages, and on a mistake the same "Usage of NAME:" listing on the
//! set's output. From Limbo's port (`rust/limbo/src/cli/flags.rs`); string, bool, int and repeatable string (a `flag.Var`
//! list) flags, a usage header of the command's own (a set whose `Usage` prints a line before `PrintDefaults`), and Go's
//! back-quoted names in a usage text; the other kinds come with the commands that use them.

use crate::gotext::quote;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Str,
    Bool,
    Int,
    /// a `flag.Var` whose `Set` appends (each use adds a value; Go's usage calls it "value")
    List,
    /// float64 (its value kept as Go's `strconv.FormatFloat(v, 'g', -1, 64)`)
    Float,
}

struct Flag {
    name: &'static str,
    kind: Kind,
    usage: &'static str,
    default: String,
    value: String,
}

pub struct FlagSet {
    name: &'static str,
    flags: Vec<Flag>,
    /// a `Usage` of the command's own: this line, then `PrintDefaults` (None: "Usage of NAME:")
    header: Option<String>,
    /// the values a List flag was given
    lists: Vec<(&'static str, Vec<String>)>,
    /// the flags set on the command line (Go's `actual`)
    set: Vec<&'static str>,
}

/// What a parse failed with: Go's error text. The usage has already gone to `stderr`.
pub const HELP: &str = "flag: help requested";

impl FlagSet {
    pub fn new(name: &'static str) -> FlagSet {
        FlagSet { name, flags: vec![], header: None, lists: vec![], set: vec![] }
    }

    /// A `fs.Usage` that prints `line` and then the flags.
    pub fn usage_header(&mut self, line: &str) {
        self.header = Some(line.to_owned());
    }

    /// A repeatable flag (Go's `fs.Var` with a value that appends).
    pub fn list(&mut self, name: &'static str, usage: &'static str) {
        self.flags.push(Flag { name, kind: Kind::List, usage, default: String::new(), value: String::new() });
        self.lists.push((name, vec![]));
    }

    /// What a List flag was given, in order.
    pub fn values(&self, name: &str) -> Vec<String> {
        self.lists.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone()).unwrap_or_default()
    }

    pub fn string(&mut self, name: &'static str, default: &str, usage: &'static str) {
        self.flags.push(Flag { name, kind: Kind::Str, usage, default: default.into(), value: default.into() });
    }

    pub fn bool(&mut self, name: &'static str, default: bool, usage: &'static str) {
        let d = if default { "true" } else { "false" };
        self.flags.push(Flag { name, kind: Kind::Bool, usage, default: d.into(), value: d.into() });
    }

    pub fn int(&mut self, name: &'static str, default: i64, usage: &'static str) {
        let d = default.to_string();
        self.flags.push(Flag { name, kind: Kind::Int, usage, default: d.clone(), value: d });
    }

    pub fn float(&mut self, name: &'static str, default: f64, usage: &'static str) {
        let d = format_g(default);
        self.flags.push(Flag { name, kind: Kind::Float, usage, default: d.clone(), value: d });
    }

    /// A float flag's value.
    pub fn f64(&self, name: &str) -> f64 {
        crate::gotext::parse_float(&self.get(name).value).unwrap_or(0.0)
    }

    /// The flags given on the command line (Go's `fs.Visit`), sorted by name.
    pub fn given(&self) -> Vec<&'static str> {
        let mut v = self.set.clone();
        v.sort_unstable();
        v
    }

    /// Whether a flag was given on the command line.
    pub fn is_set(&self, name: &str) -> bool {
        self.set.contains(&name)
    }

    fn get(&self, name: &str) -> &Flag {
        self.flags.iter().find(|f| f.name == name).expect("flag defined")
    }

    pub fn str(&self, name: &str) -> String {
        self.get(name).value.clone()
    }

    pub fn is_true(&self, name: &str) -> bool {
        self.get(name).value == "true"
    }

    pub fn num(&self, name: &str) -> i64 {
        self.get(name).value.parse().unwrap_or(0)
    }

    /// Go's `defaultUsage`: "Usage of NAME:" ("Usage:" for a set without a name) and `PrintDefaults`, flags sorted by name;
    /// or the set's own header line and `PrintDefaults`.
    pub fn usage(&self, out: &mut Vec<u8>) {
        if let Some(h) = &self.header {
            out.extend_from_slice(h.as_bytes());
            out.push(b'\n');
        } else if self.name.is_empty() {
            out.extend_from_slice(b"Usage:\n");
        } else {
            out.extend_from_slice(format!("Usage of {}:\n", self.name).as_bytes());
        }
        let mut fl: Vec<&Flag> = self.flags.iter().collect();
        fl.sort_by(|a, b| a.name.cmp(b.name));
        for f in fl {
            let mut b = format!("  -{}", f.name);
            let (name, usage) = unquote_usage(f);
            if !name.is_empty() {
                b.push(' ');
                b.push_str(&name);
            }
            // a one-letter flag without a type name stays on one line
            if b.len() <= 4 {
                b.push('\t');
            } else {
                b.push_str("\n    \t");
            }
            b.push_str(&usage.replace('\n', "\n    \t"));
            let zero = match f.kind {
                Kind::Str | Kind::List => "",
                Kind::Bool => "false",
                Kind::Int | Kind::Float => "0",
            };
            if f.default != zero {
                match f.kind {
                    Kind::Str => b.push_str(&format!(" (default {})", quote(&f.default))),
                    _ => b.push_str(&format!(" (default {})", f.default)),
                }
            }
            b.push('\n');
            out.extend_from_slice(b.as_bytes());
        }
    }

    fn fail(&self, msg: String, stderr: &mut Vec<u8>) -> String {
        stderr.extend_from_slice(msg.as_bytes());
        stderr.push(b'\n');
        self.usage(stderr);
        msg
    }

    /// `fs.Parse(args)`: the arguments left after the flags (Go's `fs.Args()`).
    pub fn parse(&mut self, mut args: &[String], stderr: &mut Vec<u8>) -> Result<Vec<String>, String> {
        loop {
            let Some(s) = args.first() else { return Ok(vec![]) };
            let b = s.as_bytes();
            if b.len() < 2 || b[0] != b'-' {
                return Ok(args.to_vec());
            }
            let mut minuses = 1;
            if b[1] == b'-' {
                minuses = 2;
                if b.len() == 2 {
                    return Ok(args[1..].to_vec());
                }
            }
            let name = &s[minuses..];
            if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
                return Err(self.fail(format!("bad flag syntax: {s}"), stderr));
            }
            args = &args[1..];
            let (name, mut value) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (name, None),
            };
            let Some(i) = self.flags.iter().position(|f| f.name == name) else {
                if name == "help" || name == "h" {
                    self.usage(stderr);
                    return Err(HELP.into());
                }
                return Err(self.fail(format!("flag provided but not defined: -{name}"), stderr));
            };
            match self.flags[i].kind {
                Kind::Bool => {
                    let v = value.unwrap_or_else(|| "true".into());
                    match parse_bool(&v) {
                        Some(x) => self.flags[i].value = x.to_string(),
                        None => {
                            // Go's boolValue.Set stores what ParseBool returned, false, before it reports the error
                            self.flags[i].value = "false".into();
                            return Err(self.fail(format!("invalid boolean value {} for -{name}: parse error", quote(&v)), stderr));
                        }
                    }
                }
                kind => {
                    if value.is_none() && !args.is_empty() {
                        value = Some(args[0].clone());
                        args = &args[1..];
                    }
                    let Some(v) = value else {
                        return Err(self.fail(format!("flag needs an argument: -{name}"), stderr));
                    };
                    if kind == Kind::List {
                        if let Some((_, vals)) = self.lists.iter_mut().find(|(n, _)| *n == name) {
                            vals.push(v.clone());
                        }
                        self.flags[i].value = v;
                    } else if kind == Kind::Float {
                        // Go's float64Value.Set stores what ParseFloat returned (±Inf on a range error, 0 on a syntax one)
                        match crate::gotext::parse_float(&v) {
                            Ok(x) => self.flags[i].value = format_g(x),
                            Err(e) => {
                                let range = e.message.ends_with(RANGE);
                                self.flags[i].value = if range { format_g(e.value) } else { "0".into() };
                                let why = if range { RANGE } else { "parse error" };
                                return Err(self.fail(format!("invalid value {} for flag -{name}: {why}", quote(&v)), stderr));
                            }
                        }
                    } else if kind == Kind::Int {
                        match parse_int(&v) {
                            Ok(x) => self.flags[i].value = x.to_string(),
                            Err(why) => {
                                // Go's intValue.Set stores what ParseInt returned: the nearest bound on a range error
                                if why == RANGE {
                                    self.flags[i].value = if v.starts_with('-') { i64::MIN } else { i64::MAX }.to_string();
                                } else {
                                    self.flags[i].value = "0".into();
                                }
                                return Err(self.fail(format!("invalid value {} for flag -{name}: {why}", quote(&v)), stderr));
                            }
                        }
                    } else {
                        self.flags[i].value = v;
                    }
                }
            }
            let n = self.flags[i].name;
            if !self.set.contains(&n) {
                self.set.push(n);
            }
        }
    }
}

/// Go's `strconv.FormatFloat(v, 'g', -1, 64)` (and `%v` of a float64): the shortest digits that read back as `v`, with an
/// exponent (`1e+06`, `1e-05`) when the decimal exponent is below -4 or at least 6.
pub fn format_g(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    // Rust's {:e} is the shortest round-trip digits: "d.ddde-N"
    let e = format!("{v:e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let sign = if neg { "-" } else { "" };
    if !(-4..6).contains(&exp) {
        let rest = if digits.len() > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        let es = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{}{rest}e{es}{:02}", &digits[..1], exp.abs());
    }
    if exp < 0 {
        return format!("{sign}0.{}{digits}", "0".repeat((-exp - 1) as usize));
    }
    let point = exp as usize + 1;
    if digits.len() <= point {
        format!("{sign}{digits}{}", "0".repeat(point - digits.len()))
    } else {
        format!("{sign}{}.{}", &digits[..point], &digits[point..])
    }
}

/// Go's `UnquoteUsage`: the first back-quoted word of the usage is the value's name (and loses its quotes), else the
/// kind's name ("" for a bool).
fn unquote_usage(f: &Flag) -> (String, String) {
    let u = f.usage;
    if let Some(i) = u.find('`') {
        if let Some(j) = u[i + 1..].find('`') {
            let name = &u[i + 1..i + 1 + j];
            return (name.to_owned(), format!("{}{name}{}", &u[..i], &u[i + 2 + j..]));
        }
    }
    let name = match f.kind {
        Kind::Str => "string",
        Kind::Int => "int",
        Kind::Bool => "",
        Kind::List => "value",
        Kind::Float => "float",
    };
    (name.to_owned(), u.to_owned())
}

/// strconv.ParseBool
pub fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

const SYNTAX: &str = "parse error";
const RANGE: &str = "value out of range";

/// strconv.ParseInt(s, 0, 64), with the flag package's short reasons ("parse error", "value out of range").
pub fn parse_int(s0: &str) -> Result<i64, &'static str> {
    if s0.is_empty() {
        return Err(SYNTAX);
    }
    let (neg, s) = match s0.as_bytes()[0] {
        b'+' => (false, &s0[1..]),
        b'-' => (true, &s0[1..]),
        _ => (false, s0),
    };
    // ParseUint(s, 0, 64)
    if s.is_empty() {
        return Err(SYNTAX);
    }
    let b = s.as_bytes();
    let (base, digits): (u64, &[u8]) = if b[0] == b'0' {
        if b.len() >= 3 && matches!(b[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
            (
                match b[1].to_ascii_lowercase() {
                    b'b' => 2,
                    b'o' => 8,
                    _ => 16,
                },
                &b[2..],
            )
        } else {
            (8, &b[1..])
        }
    } else {
        (10, b)
    };
    let mut n: u64 = 0;
    let mut underscores = false;
    for &c in digits {
        let d = match c {
            b'_' => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => (c - b'0') as u64,
            b'a'..=b'z' => (c - b'a' + 10) as u64,
            b'A'..=b'Z' => (c - b'A' + 10) as u64,
            _ => return Err(SYNTAX),
        };
        if d >= base {
            return Err(SYNTAX);
        }
        // overflow: Go stops at once with a range error
        n = match n.checked_mul(base).and_then(|x| x.checked_add(d)) {
            Some(x) => x,
            None => return Err(RANGE),
        };
    }
    if underscores && !crate::gotext::underscore_ok(s.as_bytes()) {
        return Err(SYNTAX);
    }
    let cutoff = 1u64 << 63;
    if !neg && n >= cutoff {
        return Err(RANGE);
    }
    if neg && n > cutoff {
        return Err(RANGE);
    }
    Ok(if neg { (n as i64).wrapping_neg() } else { n as i64 })
}
