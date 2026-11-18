//! The helper's command lines, parsed as Python 3.14's argparse parsed them (the parts these three parsers use): long
//! options by unique prefix (`--top` for `--top-k`, "ambiguous option" otherwise), `--opt=value`, `--` before words that look
//! like options, a word that looks like a negative number or has a space in it is a value, `int` and `float` values as Python
//! reads them, choices, the same messages (exit status 2), `-h` (exit 0), usage and help wrapped to the terminal's width
//! (`COLUMNS`, else stdout's terminal, else 80) and coloured when Python 3.14 coloured them (a terminal; `PYTHON_COLORS`,
//! `NO_COLOR`, `FORCE_COLOR`, `TERM=dumb`).

use std::collections::HashMap;
use std::io::IsTerminal;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Help,
    Store,
    StoreTrue,
    Append,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ty {
    Str,
    Int,
    Float,
}

#[derive(Clone, Copy, Debug)]
pub struct Opt {
    pub long: &'static str,
    pub kind: Kind,
    pub ty: Ty,
    pub required: bool,
    pub choices: &'static [&'static str],
    pub help: &'static str,
}

impl Opt {
    pub const fn store(long: &'static str) -> Opt {
        Opt { long, kind: Kind::Store, ty: Ty::Str, required: false, choices: &[], help: "" }
    }
    pub const fn required(mut self) -> Opt {
        self.required = true;
        self
    }
    pub const fn ty(mut self, ty: Ty) -> Opt {
        self.ty = ty;
        self
    }
    pub const fn choices(mut self, c: &'static [&'static str]) -> Opt {
        self.choices = c;
        self
    }
    pub const fn help(mut self, h: &'static str) -> Opt {
        self.help = h;
        self
    }
    pub const fn flag(long: &'static str) -> Opt {
        Opt { long, kind: Kind::StoreTrue, ty: Ty::Str, required: false, choices: &[], help: "" }
    }
    pub const fn append(long: &'static str) -> Opt {
        Opt { long, kind: Kind::Append, ty: Ty::Str, required: false, choices: &[], help: "" }
    }
    fn dest(&self) -> String {
        self.long.trim_start_matches('-').replace('-', "_")
    }
    fn metavar(&self) -> String {
        if self.choices.is_empty() {
            self.dest().to_uppercase()
        } else {
            format!("{{{}}}", self.choices.join(","))
        }
    }
    fn takes_value(&self) -> bool {
        matches!(self.kind, Kind::Store | Kind::Append)
    }
    fn name(&self) -> String {
        if self.kind == Kind::Help {
            "-h/--help".into()
        } else {
            self.long.to_string()
        }
    }
}

const HELP: Opt =
    Opt { long: "--help", kind: Kind::Help, ty: Ty::Str, required: false, choices: &[], help: "show this help message and exit" };

/// A parser: its options (after -h/--help) and at most one positional, `nargs='+'`.
pub struct Parser {
    pub prog: &'static str,
    pub opts: &'static [Opt],
    pub positional: Option<&'static str>,
}

/// What a parse gave: values by option name (`--top-k`), the positional's words, or what to print and the exit status.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub values: HashMap<&'static str, Vec<String>>,
    pub flags: Vec<&'static str>,
    pub words: Vec<String>,
}

impl Parsed {
    pub fn get(&self, long: &str) -> Option<&str> {
        self.values.get(long).and_then(|v| v.last()).map(|s| s.as_str())
    }
    pub fn all(&self, long: &str) -> Vec<String> {
        self.values.get(long).cloned().unwrap_or_default()
    }
    pub fn flag(&self, long: &str) -> bool {
        self.flags.contains(&long)
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Ok(Parsed),
    /// text for stdout, exit 0
    Help(String),
    /// text for stderr, exit 2
    Error(String),
}

/// Python's int(): surrounding whitespace, a sign, digits with single underscores between them.
pub fn py_int(s: &str) -> Option<i64> {
    let t = sarina_voice::py::strip(s);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let clean: String = digits.chars().filter(|&c| c != '_').collect();
    if !clean.chars().all(|c| c.is_ascii_digit()) || clean.len() > 4300 {
        return None;
    }
    // past i64 the slice it bounds behaves the same as at the limit
    let v: i64 = clean.parse().unwrap_or(i64::MAX);
    Some(if neg { v.checked_neg().unwrap_or(i64::MIN) } else { v })
}

/// Python's float(): whitespace, a sign, digits (single underscores between them), a point, an exponent; inf, nan.
pub fn py_float(s: &str) -> Option<f64> {
    let t = sarina_voice::py::strip(s);
    let lower = t.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    if lower.len() - body.len() <= 1 && matches!(body, "inf" | "infinity" | "nan") {
        let v = if body == "nan" { f64::NAN } else { f64::INFINITY };
        return Some(if lower.starts_with('-') { -v } else { v });
    }
    let ok_underscores =
        !t.contains("__") && t.split(|c: char| !c.is_ascii_digit() && c != '_').all(|run| !run.starts_with('_') && !run.ends_with('_'));
    if !ok_underscores || !t.chars().all(|c| c.is_ascii_digit() || matches!(c, '_' | '.' | 'e' | 'E' | '+' | '-')) {
        return None;
    }
    t.replace('_', "").parse().ok()
}

/// A usage part as (plain text, coloured text): the plain one is what the wrapping measures.
type Part = (String, String);

struct Theme {
    on: bool,
}

impl Theme {
    fn c(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

/// `_colorize.can_colorize(file=...)` for the process's fd (1 or 2).
pub fn can_colorize(fd: i32) -> bool {
    let env = |k: &str| std::env::var(k).ok();
    match env("PYTHON_COLORS").as_deref() {
        Some("0") => return false,
        Some("1") => return true,
        _ => {}
    }
    if env("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if env("FORCE_COLOR").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    if env("TERM").as_deref() == Some("dumb") {
        return false;
    }
    if fd == 1 {
        std::io::stdout().is_terminal()
    } else {
        std::io::stderr().is_terminal()
    }
}

/// `shutil.get_terminal_size().columns - 2`
fn width() -> usize {
    let mut cols: i64 = std::env::var("COLUMNS").ok().and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    if cols <= 0 {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        cols = if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 { ws.ws_col as i64 } else { 80 };
    }
    (cols - 2).max(1) as usize
}

impl Parser {
    fn all_opts(&self) -> Vec<Opt> {
        let mut v = vec![HELP];
        v.extend_from_slice(self.opts);
        v
    }

    /// (plain, coloured) usage parts: the options', then the positional's
    fn usage_parts(&self, t: &Theme) -> (Vec<Part>, Vec<Part>) {
        let mut opt_parts = Vec::new();
        for o in self.all_opts() {
            let (plain, col) = match o.kind {
                Kind::Help => ("-h".to_string(), t.c("32", "-h")),
                Kind::StoreTrue => (o.long.to_string(), t.c("36", o.long)),
                _ => {
                    let m = o.metavar();
                    let col = if t.on { format!("\x1b[36m{} \x1b[33m{}\x1b[0m", o.long, m) } else { format!("{} {}", o.long, m) };
                    (format!("{} {}", o.long, m), col)
                }
            };
            opt_parts.push(if o.required { (plain, col) } else { (format!("[{plain}]"), format!("[{col}]")) });
        }
        let pos_parts = self.positional.map(|p| {
            let s = format!("{p} [{p} ...]");
            vec![(s.clone(), t.c("32", &s))]
        });
        (opt_parts, pos_parts.unwrap_or_default())
    }

    pub fn usage(&self, color: bool) -> String {
        let t = Theme { on: color };
        let text_width = width();
        let prefix = "usage: ";
        let (opt_parts, pos_parts) = self.usage_parts(&t);
        let plain_all: Vec<&str> = opt_parts.iter().chain(&pos_parts).map(|p| p.0.as_str()).collect();
        let full = format!("{} {}", self.prog, plain_all.join(" "));
        let prog_c = t.c("1;35", self.prog);
        let body = if prefix.len() + full.len() <= text_width {
            let cols: Vec<&str> = opt_parts.iter().chain(&pos_parts).map(|p| p.1.as_str()).collect();
            format!("{} {}", prog_c, cols.join(" "))
        } else {
            let get_lines = |parts: &[(String, String)], indent: &str, prefix: Option<&str>| -> Vec<String> {
                let mut lines = Vec::new();
                let mut line: Vec<&str> = Vec::new();
                let mut line_len = match prefix {
                    Some(p) => p.len() as i64 - 1,
                    None => indent.len() as i64 - 1,
                };
                for (plain, col) in parts {
                    if line_len + 1 + plain.chars().count() as i64 > text_width as i64 && !line.is_empty() {
                        lines.push(format!("{indent}{}", line.join(" ")));
                        line.clear();
                        line_len = indent.len() as i64 - 1;
                    }
                    line.push(col);
                    line_len += plain.chars().count() as i64 + 1;
                }
                if !line.is_empty() {
                    lines.push(format!("{indent}{}", line.join(" ")));
                }
                if prefix.is_some() && !lines.is_empty() {
                    lines[0] = lines[0][indent.len()..].to_string();
                }
                lines
            };
            let mut lines;
            if (prefix.len() + self.prog.len()) as f64 <= 0.75 * text_width as f64 {
                let indent = " ".repeat(prefix.len() + self.prog.len() + 1);
                let progpart = vec![(self.prog.to_string(), prog_c.clone())];
                if !opt_parts.is_empty() {
                    lines = get_lines(&[progpart, opt_parts.clone()].concat(), &indent, Some(prefix));
                    lines.extend(get_lines(&pos_parts, &indent, None));
                } else if !pos_parts.is_empty() {
                    lines = get_lines(&[progpart, pos_parts.clone()].concat(), &indent, Some(prefix));
                } else {
                    lines = vec![prog_c.clone()];
                }
            } else {
                let indent = " ".repeat(prefix.len());
                let parts = [opt_parts.clone(), pos_parts.clone()].concat();
                lines = get_lines(&parts, &indent, None);
                if lines.len() > 1 {
                    lines = get_lines(&opt_parts, &indent, None);
                    lines.extend(get_lines(&pos_parts, &indent, None));
                }
                lines.insert(0, prog_c.clone());
            }
            lines.join("\n")
        };
        format!("{}{}", t.c("1;34", prefix), body)
    }

    pub fn help(&self, color: bool) -> String {
        let t = Theme { on: color };
        let w = width();
        let max_help_position = 24.min((w as i64 - 20).max(4) as usize);
        let invocation = |o: &Opt| -> (String, String) {
            match o.kind {
                Kind::Help => ("-h, --help".into(), format!("{}, {}", t.c("1;32", "-h"), t.c("1;36", "--help"))),
                Kind::StoreTrue => (o.long.into(), t.c("1;36", o.long)),
                _ => (format!("{} {}", o.long, o.metavar()), format!("{} {}", t.c("1;36", o.long), t.c("1;33", &o.metavar()))),
            }
        };
        let mut items: Vec<(String, String, &str)> = Vec::new();
        if let Some(p) = self.positional {
            items.push((p.into(), t.c("1;32", p), ""));
        }
        let opts: Vec<(String, String, &str)> = self
            .all_opts()
            .iter()
            .map(|o| {
                let (p, c) = invocation(o);
                (p, c, o.help)
            })
            .collect();
        let max_len = items.iter().chain(&opts).map(|i| i.0.chars().count() + 2).max().unwrap_or(0);
        let help_position = (max_len + 2).min(max_help_position);
        let help_width = (w as i64 - help_position as i64).max(11) as usize;
        let action_width = help_position - 2 - 2;
        let fmt = |items: &[(String, String, &str)]| -> String {
            let mut out = String::new();
            for (plain, col, help) in items {
                if help.is_empty() {
                    out.push_str(&format!("  {col}\n"));
                    continue;
                }
                let lines = wrap(help, help_width);
                if plain.chars().count() <= action_width {
                    let pad = " ".repeat(action_width - plain.chars().count());
                    out.push_str(&format!("  {col}{pad}  {}\n", lines[0]));
                } else {
                    out.push_str(&format!("  {col}\n"));
                    out.push_str(&format!("{}{}\n", " ".repeat(help_position), lines[0]));
                }
                for l in &lines[1..] {
                    out.push_str(&format!("{}{}\n", " ".repeat(help_position), l));
                }
            }
            out
        };
        let mut out = format!("{}\n\n", self.usage(color));
        if self.positional.is_some() {
            out.push_str(&format!("{}\n{}\n", t.c("1;34", "positional arguments:"), fmt(&items)));
        }
        out.push_str(&format!("{}\n{}", t.c("1;34", "options:"), fmt(&opts)));
        format!("{}\n", out.trim_end_matches('\n'))
    }

    fn error(&self, msg: &str) -> Outcome {
        Outcome::Error(format!("{}\n{}: error: {}\n", self.usage(can_colorize(2)), self.prog, msg))
    }

    fn lookup(&self, s: &str) -> Option<Opt> {
        if s == "-h" || s == "--help" {
            return Some(HELP);
        }
        self.opts.iter().find(|o| o.long == s).copied()
    }

    /// the option strings in the order argparse knows them
    fn strings(&self) -> Vec<(&'static str, Opt)> {
        let mut v = vec![("-h", HELP), ("--help", HELP)];
        v.extend(self.opts.iter().map(|o| (o.long, *o)));
        v
    }

    /// `_parse_optional`: None for a word (A); Some(tuples) for an option (O), where a tuple's Opt is None for an unknown
    /// option.
    #[allow(clippy::type_complexity)]
    fn parse_optional(&self, arg: &str) -> Result<Option<Vec<(Option<Opt>, String, Option<String>, Option<String>)>>, String> {
        if arg.is_empty() || !arg.starts_with('-') {
            return Ok(None);
        }
        if let Some(o) = self.lookup(arg) {
            return Ok(Some(vec![(Some(o), arg.into(), None, None)]));
        }
        if arg.chars().count() == 1 {
            return Ok(None);
        }
        if let Some((o, e)) = arg.split_once('=') {
            if let Some(op) = self.lookup(o) {
                return Ok(Some(vec![(Some(op), o.into(), Some("=".into()), Some(e.into()))]));
            }
        }
        let mut tuples = Vec::new();
        let second_dash = arg[1..].starts_with('-');
        if second_dash {
            let (prefix, sep, explicit) = match arg.split_once('=') {
                Some((p, e)) => (p, Some("=".to_string()), Some(e.to_string())),
                None => (arg, None, None),
            };
            for (s, o) in self.strings() {
                if s.starts_with(prefix) {
                    tuples.push((Some(o), s.to_string(), sep.clone(), explicit.clone()));
                }
            }
        } else {
            let short: String = arg.chars().take(2).collect();
            let rest: String = arg.chars().skip(2).collect();
            for (s, o) in self.strings() {
                if s == short {
                    tuples.push((Some(o), s.to_string(), Some(String::new()), Some(rest.clone())));
                } else if s.starts_with(arg) {
                    tuples.push((Some(o), s.to_string(), None, None));
                }
            }
        }
        if tuples.len() > 1 {
            let names: Vec<&str> = tuples.iter().map(|t| t.1.as_str()).collect();
            return Err(format!("ambiguous option: {} could match {}", arg, names.join(", ")));
        }
        if tuples.len() == 1 {
            return Ok(Some(tuples));
        }
        // '^-\d+$|^-\d*\.\d+$': a negative number is a value
        let b = &arg[1..];
        let is_num = (!b.is_empty() && b.chars().all(|c| c.is_ascii_digit()))
            || b.split_once('.')
                .is_some_and(|(i, f)| i.chars().all(|c| c.is_ascii_digit()) && !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()));
        if is_num || arg.contains(' ') {
            return Ok(None);
        }
        Ok(Some(vec![(None, arg.into(), None, None)]))
    }

    fn convert(&self, o: &Opt, v: &str) -> Result<String, String> {
        match o.ty {
            Ty::Int if py_int(v).is_none() => {
                return Err(format!("argument {}: invalid int value: {}", o.name(), sarina_voice::py::str_repr(v)))
            }
            Ty::Float if py_float(v).is_none() => {
                return Err(format!("argument {}: invalid float value: {}", o.name(), sarina_voice::py::str_repr(v)))
            }
            _ => {}
        }
        if !o.choices.is_empty() && !o.choices.contains(&v) {
            let choices: Vec<String> = o.choices.iter().map(|c| sarina_voice::py::str_repr(c)).collect();
            return Err(format!(
                "argument {}: invalid choice: {} (choose from {})",
                o.name(),
                sarina_voice::py::str_repr(v),
                choices.join(", ")
            ));
        }
        Ok(v.to_string())
    }

    pub fn parse(&self, args: &[String]) -> Outcome {
        match self.parse_inner(args) {
            Ok(Ok(p)) => Outcome::Ok(p),
            Ok(Err(())) => Outcome::Help(self.help(can_colorize(1))),
            Err(m) => self.error(&m),
        }
    }

    #[allow(clippy::type_complexity)]
    fn parse_inner(&self, args: &[String]) -> Result<Result<Parsed, ()>, String> {
        let mut pattern: Vec<char> = Vec::new();
        let mut opt_at: HashMap<usize, Vec<(Option<Opt>, String, Option<String>, Option<String>)>> = HashMap::new();
        let mut after_dashes = false;
        for (i, a) in args.iter().enumerate() {
            if after_dashes {
                pattern.push('A');
            } else if a == "--" {
                pattern.push('-');
                after_dashes = true;
            } else {
                match self.parse_optional(a)? {
                    None => pattern.push('A'),
                    Some(t) => {
                        opt_at.insert(i, t);
                        pattern.push('O');
                    }
                }
            }
        }
        let mut parsed = Parsed::default();
        let mut seen: Vec<&'static str> = Vec::new();
        let mut extras: Vec<String> = Vec::new();
        let mut positional_left = self.positional.is_some();
        let max_opt = opt_at.keys().max().map(|&m| m as i64).unwrap_or(-1);

        // consume_positionals: the one '+' positional takes '-*A[A-]*' from `start`
        let consume_positionals = |start: usize, positional_left: &mut bool, parsed: &mut Parsed, seen: &mut Vec<&'static str>| -> usize {
            if !*positional_left {
                return start;
            }
            let p = &pattern[start..];
            let mut n = 0;
            while n < p.len() && p[n] == '-' {
                n += 1;
            }
            if n >= p.len() || p[n] != 'A' {
                return start;
            }
            n += 1;
            while n < p.len() && (p[n] == 'A' || p[n] == '-') {
                n += 1;
            }
            let mut words: Vec<String> = args[start..start + n].to_vec();
            if pattern[start..start + n].contains(&'-') {
                if let Some(i) = words.iter().position(|w| w == "--") {
                    words.remove(i);
                }
            }
            parsed.words = words;
            seen.push("positional");
            *positional_left = false;
            start + n
        };

        let mut start = 0usize;
        while (start as i64) <= max_opt {
            let mut next = start;
            while (next as i64) <= max_opt && !opt_at.contains_key(&next) {
                next += 1;
            }
            if start != next {
                let end = consume_positionals(start, &mut positional_left, &mut parsed, &mut seen);
                if end > start {
                    start = end;
                    continue;
                }
                start = end;
            }
            if !opt_at.contains_key(&start) {
                extras.extend_from_slice(&args[start..next]);
                start = next;
            }
            // consume_optional
            let tuples = &opt_at[&start];
            let (opt, option_string, sep, explicit) = tuples[0].clone();
            let Some(o) = opt else {
                extras.push(args[start].clone());
                start += 1;
                continue;
            };
            let mut take: Vec<(Opt, Option<String>)> = Vec::new();
            let ignored =
                |o: &Opt, ex: &str| format!("argument {}: ignored explicit argument {}", o.name(), sarina_voice::py::str_repr(ex));
            let (mut o, mut option_string, mut sep, mut explicit) = (o, option_string, sep, explicit);
            let stop;
            loop {
                if let Some(ex) = explicit.clone() {
                    let short = !option_string[1..].starts_with('-');
                    if !o.takes_value() && short && !ex.is_empty() {
                        if sep.as_deref().is_some_and(|s| !s.is_empty()) || ex.starts_with('-') {
                            return Err(ignored(&o, &ex));
                        }
                        take.push((o, None));
                        let os = format!("-{}", ex.chars().next().unwrap_or(' '));
                        if let Some(o2) = self.lookup(&os) {
                            o = o2;
                            option_string = os;
                            let rest: String = ex.chars().skip(1).collect();
                            if rest.is_empty() {
                                sep = None;
                                explicit = None;
                            } else if let Some(r) = rest.strip_prefix('=') {
                                sep = Some("=".into());
                                explicit = Some(r.to_string());
                            } else {
                                sep = Some(String::new());
                                explicit = Some(rest);
                            }
                        } else {
                            extras.push(format!("-{ex}"));
                            stop = start + 1;
                            break;
                        }
                    } else if o.takes_value() {
                        take.push((o, Some(ex)));
                        stop = start + 1;
                        break;
                    } else {
                        return Err(ignored(&o, &ex));
                    }
                } else {
                    if o.takes_value() {
                        if pattern.get(start + 1) != Some(&'A') {
                            return Err(format!("argument {}: expected one argument", o.name()));
                        }
                        take.push((o, Some(args[start + 1].clone())));
                        stop = start + 2;
                    } else {
                        take.push((o, None));
                        stop = start + 1;
                    }
                    break;
                }
            }
            for (o, v) in take {
                seen.push(o.long);
                match o.kind {
                    Kind::Help => return Ok(Err(())),
                    Kind::StoreTrue => parsed.flags.push(o.long),
                    Kind::Store => {
                        let v = self.convert(&o, &v.unwrap_or_default())?;
                        parsed.values.insert(o.long, vec![v]);
                    }
                    Kind::Append => {
                        let v = self.convert(&o, &v.unwrap_or_default())?;
                        parsed.values.entry(o.long).or_default().push(v);
                    }
                }
            }
            start = stop;
        }
        let stop = consume_positionals(start, &mut positional_left, &mut parsed, &mut seen);
        extras.extend_from_slice(&args[stop..]);
        let mut required: Vec<String> = Vec::new();
        if let Some(p) = self.positional {
            if !seen.contains(&"positional") {
                required.push(p.to_string());
            }
        }
        for o in self.opts {
            if o.required && !seen.contains(&o.long) {
                required.push(o.long.to_string());
            }
        }
        if !required.is_empty() {
            return Err(format!("the following arguments are required: {}", required.join(", ")));
        }
        if !extras.is_empty() {
            return Err(format!("unrecognized arguments: {}", extras.join(" ")));
        }
        Ok(Ok(parsed))
    }
}

/// textwrap.wrap(text, width) after argparse's whitespace normalisation (the help texts here are plain words).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_python_reads_them() {
        assert_eq!(py_int(" 1_000 "), Some(1000));
        assert_eq!(py_int("+7"), Some(7));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("_1"), None);
        assert_eq!(py_int("1.0"), None);
        assert_eq!(py_float("1e3"), Some(1000.0));
        assert_eq!(py_float(" -inf"), Some(f64::NEG_INFINITY));
        assert!(py_float("nan").unwrap().is_nan());
        assert_eq!(py_float("1_0.5"), Some(10.5));
        assert_eq!(py_float("x"), None);
        assert_eq!(py_float("1e"), None);
    }

    #[test]
    fn wraps_like_textwrap() {
        assert_eq!(
            wrap("extra flag passed to every chat/resume call (repeatable), e.g. --agent-arg=--allow-write", 54),
            vec!["extra flag passed to every chat/resume call", "(repeatable), e.g. --agent-arg=--allow-write"]
        );
    }
}
