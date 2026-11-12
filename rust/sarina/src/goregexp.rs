//! Go's `regexp` syntax with Go's meaning, on Rust's regex crate (from Heimdall's port, `rust/heimdall/src/rules.rs`, where the
//! decision is written down: Go's `\b \w \d \s` are ASCII-only and Rust's are Unicode-aware, so they are written out as
//! Go's ASCII classes; `(?i)` folds case with Unicode's simple folding in both).

use regex::Regex;

/// Translates a pattern in Go's syntax to one Rust's regex reads with Go's meaning. Only what the Go code's patterns use is
/// accepted; anything else is refused by name, so no construct is ever read with a different meaning.
pub fn translate(go: &str) -> Result<String, String> {
    const SPACE: &str = r"\t\n\x0C\r ";
    const WORD: &str = "0-9A-Za-z_";
    const DIGIT: &str = "0-9";
    let mut out = String::with_capacity(go.len() + 16);
    let mut chars = go.chars().peekable();
    let mut in_class = false;
    let mut class_start = false; // just after '[' or '[^': a ']' here is a literal
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(e) = chars.next() else { return Err("the pattern ends with a lone backslash".into()) };
                let rep: String = match (e, in_class) {
                    ('s', false) => format!("[{SPACE}]"),
                    ('S', false) => format!("[^{SPACE}]"),
                    ('d', false) => format!("[{DIGIT}]"),
                    ('D', false) => format!("[^{DIGIT}]"),
                    ('w', false) => format!("[{WORD}]"),
                    ('W', false) => format!("[^{WORD}]"),
                    ('s', true) => SPACE.into(),
                    ('d', true) => DIGIT.into(),
                    ('w', true) => WORD.into(),
                    ('b', false) => r"(?-u:\b)".into(),
                    ('B', false) => r"(?-u:\B)".into(),
                    ('A' | 'z', false) => format!("\\{e}"),
                    ('n' | 't' | 'r' | 'f' | 'v' | 'a', _) => format!("\\{e}"),
                    // Unicode classes by their one-letter or braced name (\pL, \p{N}): the same general categories
                    ('p' | 'P', _) => {
                        let name: String = if chars.peek() == Some(&'{') {
                            chars.next();
                            let mut n = String::new();
                            loop {
                                match chars.next() {
                                    Some('}') => break,
                                    Some(c) if c.is_ascii_alphabetic() && n.len() < 32 => n.push(c),
                                    _ => return Err("a Unicode class name is not closed or not a name".into()),
                                }
                            }
                            n
                        } else {
                            match chars.next() {
                                Some(c) if c.is_ascii_alphabetic() => c.to_string(),
                                _ => return Err("a Unicode class needs a name".into()),
                            }
                        };
                        if !matches!(name.as_str(), "L" | "Lu" | "Ll" | "N" | "Nd") {
                            return Err(format!("the Unicode class {name} is not supported"));
                        }
                        format!("\\{e}{{{name}}}")
                    }
                    // Go reads \< and \> as the characters; Rust as word-start and word-end assertions
                    ('<' | '>', _) => e.to_string(),
                    (e, _) if e.is_ascii_punctuation() => format!("\\{e}"),
                    (e, true) if matches!(e, 'S' | 'D' | 'W') => {
                        return Err(format!("\\{e} inside a character class is not supported (write the class out)"))
                    }
                    (e, _) => return Err(format!("the escape \\{e} is not supported")),
                };
                out.push_str(&rep);
                class_start = false;
            }
            '[' if !in_class => {
                in_class = true;
                class_start = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    chars.next();
                    out.push('^');
                }
            }
            '[' => {
                if chars.peek() == Some(&':') {
                    return Err("POSIX classes ([[:alpha:]]) are not supported".into());
                }
                // Go reads '[' inside a class as itself; Rust would open a nested class
                out.push_str(r"\[");
                class_start = false;
            }
            ']' if in_class && !class_start => {
                in_class = false;
                out.push(']');
            }
            '&' | '~' if in_class => {
                // Rust reads && and ~~ in a class as set operations; Go as the characters
                out.push('\\');
                out.push(c);
                class_start = false;
            }
            '(' if !in_class && chars.peek() == Some(&'?') => {
                // flags and non-capturing groups: Go's i, m, s, U only; named groups as Go writes them
                out.push('(');
                out.push(chars.next().unwrap_or('?'));
                let mut flags = String::new();
                while let Some(&f) = chars.peek() {
                    if f == ')' || f == ':' {
                        break;
                    }
                    flags.push(f);
                    chars.next();
                }
                if flags.starts_with('P') || flags.starts_with('<') {
                    return Err("named groups are not supported".into());
                }
                if !flags.chars().all(|f| matches!(f, 'i' | 'm' | 's' | 'U' | '-')) {
                    return Err(format!("the flags (?{flags}) are not supported (Go has i, m, s, U)"));
                }
                out.push_str(&flags);
            }
            c => {
                out.push(c);
                class_start = false;
            }
        }
    }
    if in_class {
        return Err("a character class is not closed".into());
    }
    Ok(out)
}

/// Compiles a Go-syntax pattern with Go's meaning.
pub fn compile(go: &str) -> Result<Regex, String> {
    let rust = translate(go)?;
    regex::RegexBuilder::new(&rust).size_limit(4 << 20).dfa_size_limit(1 << 20).build().map_err(|e| format!("does not compile: {e}"))
}
