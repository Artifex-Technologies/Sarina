//! Go: `go/tools/shellparse`. Splits a POSIX-shell command line into the simple commands it would run, so a safety check can
//! look at real program names and arguments instead of pattern-matching the raw string.
//!
//! It is a conservative reader, not an executor. It understands command separators (`; & && || | |&` newline), subshells and
//! brace groups, command substitution (`$(...)` and backticks), process substitution, quoting (single, double, backslash,
//! `$'...'`), here-documents and redirections. Every command it finds is returned in one flat list, including those nested
//! inside substitutions, so a caller cannot miss `echo $(rm x)` or `( rm x )`.
//!
//! Anything whose value is only known at run time (a variable, a substitution, a glob) is marked with [`DYN`] in
//! [`Word::text`] so callers can refuse to guess at it.
//!
//! Texts are bytes, as in Go: `$'\xff'` makes a word that is not UTF-8, and the guards must see exactly those bytes.

/// Replaces any part of a word that is only known at run time.
pub const DYN: u8 = 0;

/// One shell word with quoting removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Word {
    /// unquoted text; run-time parts are [`DYN`]
    pub text: Vec<u8>,
    /// some part was quoted or escaped
    pub quoted: bool,
    /// contains a `<(...)` or `>(...)` process substitution
    pub proc_sub: bool,
}

impl Word {
    /// Whether any part of the word is unknown until run time.
    pub fn dynamic(&self) -> bool {
        self.text.contains(&DYN)
    }
}

/// One redirection on a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    /// `>`, `>>`, `>|`, `&>`, `&>>`, `<>`, `>&`, `<`, `<&`, `<<`, `<<-`, `<<<`
    pub op: String,
    pub target: Word,
}

impl Redirect {
    /// Whether the redirection can create or truncate a file.
    pub fn writes(&self) -> bool {
        match self.op.as_str() {
            ">" | ">>" | ">|" | "&>" | "&>>" | "<>" => true,
            // ">&2" and ">&-" duplicate or close a descriptor; ">&file" writes one.
            ">&" => {
                let t = &self.target.text;
                t != b"-" && crate::gotext::atoi(t).is_none()
            }
            _ => false,
        }
    }
}

/// One simple command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    /// leading NAME=value words
    pub assigns: Vec<Word>,
    /// program and arguments
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
    /// bodies of here-documents attached to it
    pub heredocs: Vec<Vec<u8>>,
    /// non-zero: commands sharing an id form one pipeline, in order
    pub pipe_id: usize,
    /// set when this command is a function definition header
    pub func_name: Vec<u8>,
    /// a for/case word list: the words are data, not a command
    pub data: bool,
}

/// Parses `src`. It fails on unterminated quotes and substitutions, which a shell would reject too. The commands come in
/// source order, nested ones included.
pub fn parse(src: &[u8]) -> Result<Vec<Command>, String> {
    let mut shared = Shared::default();
    let mut cmds = Vec::new();
    let mut p = Parser { src, pos: 0, out: &mut cmds, shared: &mut shared, depth: 0, here: Vec::new(), stash: Vec::new() };
    p.parse_seq(0)?;
    Ok(cmds)
}

const MAX_DEPTH: usize = 40;

/// A here-document waiting for its body. Go holds a pointer to the command; the port names it by index once it is in `out`,
/// and by the serial of the command still being read before that.
struct Pending {
    cmd: Target,
    delim: Vec<u8>,
    strip: bool,
    raw: bool,
}

/// Where a here-document's body goes: a command already in `out`, or the one still being read.
#[derive(Clone, Copy)]
enum Target {
    Out(usize),
    Current(u64),
}

/// What nested parsers share: Go shares the pipeline counter; the serials name the commands being read.
#[derive(Default)]
struct Shared {
    pipes: usize,
    serial: u64,
}

struct Parser<'a, 'o> {
    src: &'a [u8],
    pos: usize,
    out: &'o mut Vec<Command>,
    shared: &'o mut Shared,
    depth: usize,
    here: Vec<Pending>,
    /// bodies read for a command not finished yet (an outer frame's), by its serial, in reading order
    stash: Vec<(u64, Vec<u8>)>,
}

fn reserved(t: &[u8]) -> bool {
    matches!(
        t,
        b"if"
            | b"then"
            | b"else"
            | b"elif"
            | b"fi"
            | b"while"
            | b"until"
            | b"do"
            | b"done"
            | b"{"
            | b"}"
            | b"!"
            | b"time"
            | b"coproc"
            | b"esac"
    )
}

fn is_assign(w: &Word) -> bool {
    let t = &w.text;
    let Some(i) = t.iter().position(|&c| c == b'=') else { return false };
    if i == 0 {
        return false;
    }
    let name = t[..i].strip_suffix(b"+").unwrap_or(&t[..i]);
    if name.is_empty() {
        return false;
    }
    // Go ranges over runes; every allowed character is ASCII, so any other byte fails either way
    name.iter().enumerate().all(|(j, &r)| r == b'_' || r.is_ascii_alphabetic() || j > 0 && r.is_ascii_digit())
}

fn is_meta(c: u8) -> bool {
    b" \t\r\n;&|()<>".contains(&c)
}

/// The command being read in `parse_seq`, with a serial so a here-document pending on it can find it after it is finished.
struct Current {
    cmd: Command,
    serial: u64,
}

impl Parser<'_, '_> {
    fn next_pipe(&mut self) -> usize {
        self.shared.pipes += 1;
        self.shared.pipes
    }

    fn next_serial(&mut self) -> u64 {
        self.shared.serial += 1;
        self.shared.serial
    }

    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> u8 {
        if self.eof() {
            0
        } else {
            self.src[self.pos]
        }
    }

    fn peek_at(&self, n: usize) -> u8 {
        if self.pos + n >= self.src.len() {
            0
        } else {
            self.src[self.pos + n]
        }
    }

    /// parse_seq reads commands until EOF (term == 0) or the matching ')' (term == b')').
    fn parse_seq(&mut self, term: u8) -> Result<(), String> {
        self.depth += 1;
        let r = self.parse_seq_inner(term);
        self.depth -= 1;
        r
    }

    fn finish(&mut self, cur: &mut Current, pipe: &mut usize, sep: u8) {
        let cmd = std::mem::take(&mut cur.cmd);
        if !cmd.words.is_empty() || !cmd.assigns.is_empty() || !cmd.redirects.is_empty() || !cmd.func_name.is_empty() {
            if sep == b'|' && *pipe == 0 {
                *pipe = self.next_pipe();
            }
            let mut cmd = cmd;
            cmd.pipe_id = *pipe;
            let serial = cur.serial;
            let mut i = 0;
            while i < self.stash.len() {
                if self.stash[i].0 == serial {
                    cmd.heredocs.push(self.stash.remove(i).1);
                } else {
                    i += 1;
                }
            }
            let idx = self.out.len();
            self.out.push(cmd);
            // here-documents waiting for this command now point at its place in out
            for h in &mut self.here {
                if let Target::Current(s) = h.cmd {
                    if s == cur.serial {
                        h.cmd = Target::Out(idx);
                    }
                }
            }
        }
        // a command that was dropped (nothing in it) cannot have here-documents: a "<<" adds a redirect
        if sep != b'|' {
            *pipe = 0;
        }
        cur.serial = self.next_serial();
    }

    fn parse_seq_inner(&mut self, term: u8) -> Result<(), String> {
        if self.depth > MAX_DEPTH {
            return Err("command nesting is too deep".into());
        }
        let serial = self.next_serial();
        let mut cur = Current { cmd: Command::default(), serial };
        let mut pipe = 0usize;
        let mut skip_next = 0; // words to ignore after for/select (the loop variable)
        let mut is_case = false;

        macro_rules! finish {
            ($sep:expr) => {{
                self.finish(&mut cur, &mut pipe, $sep);
                skip_next = 0;
                is_case = false;
            }};
        }
        let at_start = |c: &Command| c.words.is_empty() && c.assigns.is_empty();

        loop {
            // blanks and line continuations
            while !self.eof() {
                let c = self.peek();
                if c == b' ' || c == b'\t' || c == b'\r' {
                    self.pos += 1;
                } else if c == b'\\' && self.peek_at(1) == b'\n' {
                    self.pos += 2;
                } else {
                    break;
                }
            }
            if self.eof() {
                self.finish(&mut cur, &mut pipe, 0);
                if term != 0 {
                    return Err("unterminated '(' in command".into());
                }
                return Ok(());
            }
            let c = self.peek();
            match c {
                b'#' => {
                    while !self.eof() && self.peek() != b'\n' {
                        self.pos += 1;
                    }
                }
                b'\n' => {
                    self.pos += 1;
                    // "a |\n b" is still one pipeline
                    if !(at_start(&cur.cmd) && pipe != 0 && cur.cmd.redirects.is_empty()) {
                        finish!(0);
                    }
                    self.read_heredocs()?;
                }
                b';' => {
                    self.pos += 1;
                    while self.peek() == b';' || self.peek() == b'&' {
                        self.pos += 1;
                    }
                    finish!(0);
                }
                b'&' => {
                    if self.peek_at(1) == b'>' {
                        // &> and &>>
                        self.pos += 1;
                        self.read_redirect(&mut cur, "&")?;
                        continue;
                    }
                    self.pos += 1;
                    if self.peek() == b'&' {
                        self.pos += 1;
                    }
                    finish!(0);
                }
                b'|' => {
                    self.pos += 1;
                    if self.peek() == b'|' {
                        self.pos += 1;
                        finish!(0);
                    } else {
                        if self.peek() == b'&' {
                            self.pos += 1;
                        }
                        finish!(b'|');
                    }
                }
                b')' => {
                    self.pos += 1;
                    finish!(0);
                    if term == b')' {
                        return Ok(());
                    }
                    // A stray ')' (a case pattern terminator) just ends a command.
                }
                b'(' => {
                    if cur.cmd.words.len() == 1 && cur.cmd.assigns.is_empty() && self.peek_at(1) == b')' && !cur.cmd.words[0].dynamic() {
                        // name() function definition
                        self.pos += 2;
                        let w = cur.cmd.words.remove(0);
                        cur.cmd.func_name = w.text;
                        finish!(0);
                        continue;
                    }
                    self.pos += 1;
                    self.parse_seq(b')')?;
                }
                b'<' | b'>' if self.peek_at(1) != b'(' => {
                    self.read_redirect(&mut cur, "")?;
                }
                _ => {
                    // "2>file" style descriptor prefix
                    if c.is_ascii_digit() {
                        let mut j = self.pos;
                        while j < self.src.len() && self.src[j].is_ascii_digit() {
                            j += 1;
                        }
                        if j < self.src.len()
                            && (self.src[j] == b'<' || self.src[j] == b'>')
                            && (j + 1 >= self.src.len() || self.src[j + 1] != b'(')
                        {
                            self.pos = j;
                            continue;
                        }
                    }
                    let w = self.read_word()?;
                    if skip_next > 0 {
                        skip_next -= 1;
                        continue;
                    }
                    if at_start(&cur.cmd) && !w.quoted && reserved(&w.text) {
                        continue;
                    }
                    if at_start(&cur.cmd) && !w.quoted {
                        match w.text.as_slice() {
                            b"for" | b"select" => {
                                skip_next = 1;
                                cur.cmd.data = true;
                                continue;
                            }
                            b"case" => {
                                cur.cmd.data = true;
                                is_case = true;
                                continue;
                            }
                            b"function" => {
                                // "function name": the next word names it
                                let nw = self.read_word_if_any()?;
                                cur.cmd.func_name = nw.text;
                                finish!(0);
                                if self.peek() == b'(' && self.peek_at(1) == b')' {
                                    self.pos += 2;
                                }
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if cur.cmd.data && !w.quoted && w.text == b"in" {
                        if is_case {
                            finish!(0);
                        }
                        continue;
                    }
                    if !cur.cmd.data && cur.cmd.words.is_empty() && is_assign(&w) {
                        cur.cmd.assigns.push(w);
                        continue;
                    }
                    cur.cmd.words.push(w);
                }
            }
        }
    }

    fn read_word_if_any(&mut self) -> Result<Word, String> {
        while !self.eof() && (self.peek() == b' ' || self.peek() == b'\t') {
            self.pos += 1;
        }
        if self.eof() || b";&|()<>\n".contains(&self.peek()) {
            return Ok(Word::default());
        }
        self.read_word()
    }

    /// read_word reads one word starting at the current position.
    fn read_word(&mut self) -> Result<Word, String> {
        let mut b: Vec<u8> = Vec::new();
        let mut w = Word::default();
        while !self.eof() {
            let c = self.peek();
            if (c == b'<' || c == b'>') && self.peek_at(1) == b'(' {
                self.proc_sub()?;
                b.push(DYN);
                w.proc_sub = true;
                continue;
            }
            if is_meta(c) {
                break;
            }
            match c {
                b'\\' => {
                    if self.peek_at(1) == b'\n' {
                        self.pos += 2;
                        continue;
                    }
                    w.quoted = true;
                    if self.pos + 1 < self.src.len() {
                        let size = rune_len(&self.src[self.pos + 1..]);
                        b.extend_from_slice(&self.src[self.pos + 1..self.pos + 1 + size]);
                        self.pos += 1 + size;
                    } else {
                        self.pos += 1;
                    }
                }
                b'\'' => {
                    w.quoted = true;
                    let Some(end) = self.src[self.pos + 1..].iter().position(|&x| x == b'\'') else {
                        return Err("unterminated single quote".into());
                    };
                    b.extend_from_slice(&self.src[self.pos + 1..self.pos + 1 + end]);
                    self.pos += end + 2;
                }
                b'"' => {
                    w.quoted = true;
                    self.pos += 1;
                    self.read_double(&mut b)?;
                }
                b'`' => {
                    self.backtick()?;
                    b.push(DYN);
                }
                b'$' => {
                    self.read_dollar(&mut b, &mut w)?;
                }
                // glob characters stay visible; callers decide
                _ => {
                    b.push(c);
                    self.pos += 1;
                }
            }
        }
        w.text = b;
        Ok(w)
    }

    /// read_dollar handles $ outside of single quotes.
    fn read_dollar(&mut self, b: &mut Vec<u8>, w: &mut Word) -> Result<(), String> {
        let next = self.peek_at(1);
        match next {
            b'\'' => {
                // $'...' ANSI-C quoting
                w.quoted = true;
                self.pos += 2;
                loop {
                    if self.eof() {
                        return Err("unterminated $'...' quote".into());
                    }
                    let c = self.peek();
                    if c == b'\'' {
                        self.pos += 1;
                        return Ok(());
                    }
                    if c == b'\\' {
                        self.pos += 1;
                        self.ansi_escape(b);
                        continue;
                    }
                    b.push(c);
                    self.pos += 1;
                }
            }
            b'"' => {
                w.quoted = true;
                self.pos += 2;
                return self.read_double(b);
            }
            b'(' => {
                self.pos += 2;
                self.parse_seq(b')')?;
                b.push(DYN);
                return Ok(());
            }
            b'{' => {
                self.pos += 2;
                self.skip_brace_param()?;
                b.push(DYN);
                return Ok(());
            }
            c if c == b'_' || c.is_ascii_alphabetic() => {
                self.pos += 1;
                while !self.eof() {
                    let c = self.peek();
                    if c == b'_' || c.is_ascii_alphanumeric() {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                b.push(DYN);
                return Ok(());
            }
            c if c.is_ascii_digit() || b"@*#?-$!".contains(&c) => {
                self.pos += 2;
                b.push(DYN);
                return Ok(());
            }
            _ => {}
        }
        b.push(b'$');
        self.pos += 1;
        Ok(())
    }

    /// ansi_escape decodes one backslash escape inside $'...'. The backslash is consumed.
    fn ansi_escape(&mut self, b: &mut Vec<u8>) {
        if self.eof() {
            return;
        }
        let c = self.peek();
        self.pos += 1;
        match c {
            b'n' => b.push(b'\n'),
            b't' => b.push(b'\t'),
            b'r' => b.push(b'\r'),
            b'a' => b.push(7),
            b'b' => b.push(8),
            b'e' | b'E' => b.push(27),
            b'f' => b.push(12),
            b'v' => b.push(11),
            b'x' | b'u' | b'U' => {
                let max = match c {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let (mut n, mut digits) = (0u64, 0);
                while digits < max && !self.eof() {
                    let Some(d) = hex_val(self.peek()) else { break };
                    n = n * 16 + d;
                    digits += 1;
                    self.pos += 1;
                }
                if c == b'x' {
                    b.push(n as u8);
                } else {
                    // Go's rune(n) is n's low 32 bits as a signed value; WriteRune writes U+FFFD for anything that is not a
                    // valid character (negative, a surrogate, above U+10FFFF)
                    let r = char::from_u32(n as u32).unwrap_or('\u{FFFD}');
                    let mut tmp = [0u8; 4];
                    b.extend_from_slice(r.encode_utf8(&mut tmp).as_bytes());
                }
            }
            b'0'..=b'7' => {
                let mut n = u32::from(c - b'0');
                let mut i = 0;
                while i < 2 && !self.eof() && (b'0'..=b'7').contains(&self.peek()) {
                    n = n * 8 + u32::from(self.peek() - b'0');
                    self.pos += 1;
                    i += 1;
                }
                b.push(n as u8);
            }
            _ => b.push(c),
        }
    }

    /// read_double reads the inside of "..." (the opening quote is consumed) and consumes the closing quote.
    fn read_double(&mut self, b: &mut Vec<u8>) -> Result<(), String> {
        loop {
            if self.eof() {
                return Err("unterminated double quote".into());
            }
            let c = self.peek();
            match c {
                b'"' => {
                    self.pos += 1;
                    return Ok(());
                }
                b'\\' => {
                    let n = self.peek_at(1);
                    if n == b'\n' {
                        self.pos += 2;
                    } else if n == b'$' || n == b'`' || n == b'"' || n == b'\\' {
                        b.push(n);
                        self.pos += 2;
                    } else {
                        b.push(b'\\');
                        self.pos += 1;
                    }
                }
                b'`' => {
                    self.backtick()?;
                    b.push(DYN);
                }
                b'$' => {
                    let next = self.peek_at(1);
                    if next == b'\'' || next == b'"' {
                        // not special inside double quotes
                        b.push(b'$');
                        self.pos += 1;
                        continue;
                    }
                    let mut w = Word::default();
                    self.read_dollar(b, &mut w)?;
                }
                _ => {
                    b.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// skip_brace_param skips ${...}, still parsing substitutions inside it.
    fn skip_brace_param(&mut self) -> Result<(), String> {
        let mut depth = 1;
        while !self.eof() {
            let c = self.peek();
            if c == b'\\' {
                self.pos += 2;
                continue;
            } else if c == b'$' && self.peek_at(1) == b'(' {
                self.pos += 2;
                self.parse_seq(b')')?;
                continue;
            } else if c == b'`' {
                self.backtick()?;
                continue;
            } else if c == b'{' {
                depth += 1;
            } else if c == b'}' {
                depth -= 1;
                if depth == 0 {
                    self.pos += 1;
                    return Ok(());
                }
            }
            self.pos += 1;
        }
        Err("unterminated ${...}".into())
    }

    /// backtick parses `...` (the opening backtick is at pos).
    fn backtick(&mut self) -> Result<(), String> {
        self.pos += 1;
        let mut inner = Vec::new();
        loop {
            if self.eof() {
                return Err("unterminated backtick substitution".into());
            }
            let c = self.peek();
            if c == b'\\' && self.pos + 1 < self.src.len() {
                let n = self.src[self.pos + 1];
                if n == b'`' || n == b'\\' || n == b'$' {
                    inner.push(n);
                    self.pos += 2;
                    continue;
                }
            }
            if c == b'`' {
                self.pos += 1;
                break;
            }
            inner.push(c);
            self.pos += 1;
        }
        self.sub(&inner)
    }

    /// sub parses text as its own script, adding its commands to the output.
    fn sub(&mut self, text: &[u8]) -> Result<(), String> {
        if self.depth >= MAX_DEPTH {
            return Err("command nesting is too deep".into());
        }
        let mut q = Parser {
            src: text,
            pos: 0,
            out: &mut *self.out,
            shared: &mut *self.shared,
            depth: self.depth,
            here: Vec::new(),
            stash: Vec::new(),
        };
        // Go's sub-parser collects into its own list and appends it only on success; an error discards the whole parse, so
        // appending as it goes is the same
        q.parse_seq(0)
    }

    /// proc_sub parses <(...) or >(...).
    fn proc_sub(&mut self) -> Result<(), String> {
        self.pos += 2;
        self.parse_seq(b')')
    }

    /// read_redirect reads a redirection operator and its target. prefix "&" means the leading '&' of &> or &>> was already
    /// consumed.
    fn read_redirect(&mut self, cur: &mut Current, prefix: &str) -> Result<(), String> {
        let mut op = prefix.to_owned();
        let c = self.peek();
        self.pos += 1;
        op.push(c as char);
        let n = self.peek();
        if c == b'>' && (n == b'>' || n == b'|') {
            op.push(n as char);
            self.pos += 1;
        } else if n == b'&' && (c == b'>' && prefix.is_empty() || c == b'<') {
            op.push('&');
            self.pos += 1;
        } else if c == b'<' && n == b'>' {
            op.push('>');
            self.pos += 1;
        } else if c == b'<' && n == b'<' {
            op.push('<');
            self.pos += 1;
            if self.peek() == b'<' {
                op.push('<');
                self.pos += 1;
            } else if self.peek() == b'-' {
                op.push('-');
                self.pos += 1;
            }
        }
        while !self.eof() && (self.peek() == b' ' || self.peek() == b'\t') {
            self.pos += 1;
        }
        if self.eof() || b";&|()\n".contains(&self.peek()) && self.peek() != b'&' {
            return Err(format!("redirection {op} has no target"));
        }
        let target = self.read_word()?;
        if op == "<<" || op == "<<-" {
            self.here.push(Pending {
                cmd: Target::Current(cur.serial),
                delim: target.text.clone(),
                strip: op == "<<-",
                raw: target.quoted,
            });
        }
        cur.cmd.redirects.push(Redirect { op, target });
        Ok(())
    }

    /// read_heredocs consumes the bodies of pending here-documents, which start on the line after the operator. For an
    /// unquoted delimiter the shell expands substitutions in the body, so those are parsed as commands too.
    fn read_heredocs(&mut self) -> Result<(), String> {
        let here = std::mem::take(&mut self.here);
        for h in here {
            let mut body = Vec::new();
            while !self.eof() {
                let rest = &self.src[self.pos..];
                let line = match rest.iter().position(|&x| x == b'\n') {
                    None => {
                        self.pos = self.src.len();
                        rest
                    }
                    Some(end) => {
                        self.pos += end + 1;
                        &rest[..end]
                    }
                };
                let mut cmp = line.strip_suffix(b"\r").unwrap_or(line);
                if h.strip {
                    while let Some(r) = cmp.strip_prefix(b"\t") {
                        cmp = r;
                    }
                }
                if cmp == h.delim.as_slice() {
                    break;
                }
                body.extend_from_slice(line);
                body.push(b'\n');
            }
            match h.cmd {
                Target::Out(i) => self.out[i].heredocs.push(body.clone()),
                // a command of an outer frame, still being read: its bodies join it when it is finished
                Target::Current(s) => self.stash.push((s, body.clone())),
            }
            if !h.raw {
                self.expansions_in(&body)?;
            }
        }
        Ok(())
    }

    /// expansions_in parses the substitutions inside text that the shell would expand.
    fn expansions_in(&mut self, text: &[u8]) -> Result<(), String> {
        let mut q = Parser {
            src: text,
            pos: 0,
            out: &mut *self.out,
            shared: &mut *self.shared,
            depth: self.depth,
            here: Vec::new(),
            stash: Vec::new(),
        };
        let mut sink = Vec::new();
        while !q.eof() {
            match q.peek() {
                b'\\' => q.pos += 2,
                b'`' => q.backtick()?,
                b'$' => {
                    if q.peek_at(1) == b'(' || q.peek_at(1) == b'{' {
                        let mut w = Word::default();
                        q.read_dollar(&mut sink, &mut w)?;
                    } else {
                        q.pos += 1;
                    }
                }
                _ => q.pos += 1,
            }
        }
        Ok(())
    }
}

/// The length of the character at the start of `b` as Go's `utf8.DecodeRuneInString` reads it: a byte that does not start
/// a valid sequence counts as one.
fn rune_len(b: &[u8]) -> usize {
    let n = match b[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return 1,
    };
    if b.len() >= n && std::str::from_utf8(&b[..n]).is_ok() {
        n
    } else {
        1
    }
}

fn hex_val(c: u8) -> Option<u64> {
    (c as char).to_digit(16).map(u64::from)
}
