//! labyrinth-ask-sarina's command line, parsed as Python 3.14's argparse parsed it: unique prefixes of long options
//! (--conv for --conversation), --opt=value, "--" before words that look like options, a word that looks like a negative
//! number or has a space in it is part of the question, the same error messages (exit status 2) and -h.

use crate::py;

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub cwd: String,
    pub write: bool,
    pub shell: bool,
    pub conversation: String,
    pub voice: bool,
    pub question: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Act {
    Help,
    Cwd,
    Write,
    Shell,
    Conversation,
    Voice,
}

// the option strings in the order argparse knows them (the order its "could match" message lists them)
const OPTS: [(&str, Act); 7] = [
    ("-h", Act::Help),
    ("--help", Act::Help),
    ("--cwd", Act::Cwd),
    ("--write", Act::Write),
    ("--shell", Act::Shell),
    ("--conversation", Act::Conversation),
    ("--voice", Act::Voice),
];

fn name(a: Act) -> &'static str {
    match a {
        Act::Help => "-h/--help",
        Act::Cwd => "--cwd",
        Act::Write => "--write",
        Act::Shell => "--shell",
        Act::Conversation => "--conversation",
        Act::Voice => "--voice",
    }
}

fn takes_value(a: Act) -> bool {
    matches!(a, Act::Cwd | Act::Conversation)
}

fn lookup(s: &str) -> Option<Act> {
    OPTS.iter().find(|(o, _)| *o == s).map(|(_, a)| *a)
}

/// (action or None for an unknown option, option string, sep, explicit argument)
type Tuple = (Option<Act>, String, Option<String>, Option<String>);

fn parse_optional(arg: &str) -> Option<Vec<Tuple>> {
    if arg.is_empty() || !arg.starts_with('-') {
        return None;
    }
    if let Some(a) = lookup(arg) {
        return Some(vec![(Some(a), arg.to_string(), None, None)]);
    }
    if py::clen(arg) == 1 {
        return None;
    }
    if let Some((o, e)) = arg.split_once('=') {
        if let Some(a) = lookup(o) {
            return Some(vec![(Some(a), o.to_string(), Some("=".into()), Some(e.to_string()))]);
        }
    }
    let t = option_tuples(arg);
    if !t.is_empty() {
        return Some(t);
    }
    // looks like a negative number (r'-\.?\d'): a word of the question
    let mut cs = arg.chars().skip(1);
    let mut c = cs.next();
    if c == Some('.') {
        c = cs.next();
    }
    if let Some(c) = c {
        if c.is_ascii_digit() || (!c.is_ascii() && crate::rx::is_unicode_decimal(c)) {
            return None;
        }
    }
    if arg.contains(' ') {
        return None;
    }
    Some(vec![(None, arg.to_string(), None, None)])
}

fn option_tuples(arg: &str) -> Vec<Tuple> {
    let mut out = Vec::new();
    let second = arg.chars().nth(1);
    if second == Some('-') {
        let (prefix, sep, explicit) = match arg.split_once('=') {
            Some((p, e)) => (p, Some("=".to_string()), Some(e.to_string())),
            None => (arg, None, None),
        };
        for (o, a) in OPTS {
            if o.starts_with(prefix) {
                out.push((Some(a), o.to_string(), sep.clone(), explicit.clone()));
            }
        }
    } else {
        let (prefix, sep, explicit) = match arg.split_once('=') {
            Some((p, e)) => (p, Some("=".to_string()), Some(e.to_string())),
            None => (arg, None, None),
        };
        let short = py::take(arg, 2);
        let short_explicit = py::skip(arg, 2);
        for (o, a) in OPTS {
            if o == short {
                out.push((Some(a), o.to_string(), Some(String::new()), Some(short_explicit.to_string())));
            } else if o.starts_with(prefix) {
                out.push((Some(a), o.to_string(), sep.clone(), explicit.clone()));
            }
        }
    }
    out
}

pub enum Outcome {
    Args(Args),
    /// print this to stdout, exit 0 (-h)
    Help(String),
    /// print the usage and this message to stderr, exit 2
    Error(String),
}

pub fn parse(prog: &str, argv: &[String], default_cwd: &str) -> Outcome {
    match parse_inner(argv, default_cwd) {
        Ok(a) => Outcome::Args(a),
        Err(Stop::Help) => Outcome::Help(help(prog)),
        Err(Stop::Error(m)) => Outcome::Error(format!("{}{}: error: {}\n", usage(prog), prog, m)),
    }
}

enum Stop {
    Help,
    Error(String),
}

fn parse_inner(argv: &[String], default_cwd: &str) -> Result<Args, Stop> {
    let mut a = Args { cwd: default_cwd.to_string(), ..Default::default() };
    // the pattern: O for an option, A for an argument, - for the first "--"
    let mut pattern = Vec::new();
    let mut opts: std::collections::BTreeMap<usize, Vec<Tuple>> = Default::default();
    let mut after_dashes = false;
    for (i, s) in argv.iter().enumerate() {
        if after_dashes {
            pattern.push('A');
            continue;
        }
        if s == "--" {
            pattern.push('-');
            after_dashes = true;
            continue;
        }
        match parse_optional(s) {
            None => pattern.push('A'),
            Some(t) => {
                opts.insert(i, t);
                pattern.push('O');
            }
        }
    }
    let mut extras: Vec<String> = Vec::new();
    let mut question_done = false;
    let mut seen_question = false;

    let take = |act: Act, vals: Vec<String>, a: &mut Args| -> Result<(), Stop> {
        match act {
            Act::Help => return Err(Stop::Help),
            Act::Cwd => a.cwd = vals[0].clone(),
            Act::Conversation => a.conversation = vals[0].clone(),
            Act::Write => a.write = true,
            Act::Shell => a.shell = true,
            Act::Voice => a.voice = true,
        }
        Ok(())
    };

    // consume_positionals: '(-*A[A-]*)' at the start of the rest of the pattern
    let consume_positionals = |start: usize, a: &mut Args, done: &mut bool, seen: &mut bool| -> usize {
        if *done {
            return start;
        }
        let p = &pattern[start..];
        let mut i = 0;
        while i < p.len() && p[i] == '-' {
            i += 1;
        }
        if i >= p.len() || p[i] != 'A' {
            return start;
        }
        i += 1;
        while i < p.len() && (p[i] == 'A' || p[i] == '-') {
            i += 1;
        }
        let mut vals: Vec<String> = argv[start..start + i].to_vec();
        if p[..i].contains(&'-') {
            if let Some(k) = vals.iter().position(|v| v == "--") {
                vals.remove(k);
            }
        }
        a.question = vals;
        *done = true;
        *seen = true;
        start + i
    };

    let max_opt = opts.keys().next_back().copied();
    let mut start = 0usize;
    if let Some(max_opt) = max_opt {
        while start <= max_opt {
            let mut next = start;
            while next <= max_opt && !opts.contains_key(&next) {
                next += 1;
            }
            if start != next {
                let end = consume_positionals(start, &mut a, &mut question_done, &mut seen_question);
                if end > start {
                    start = end;
                    continue;
                }
                start = end;
            }
            if !opts.contains_key(&start) {
                extras.extend(argv[start..next].iter().cloned());
                start = next;
            }
            // consume_optional
            let tuples = opts[&start].clone();
            if tuples.len() > 1 {
                let names: Vec<&str> = tuples.iter().map(|t| t.1.as_str()).collect();
                return Err(Stop::Error(format!("ambiguous option: {} could match {}", argv[start], names.join(", "))));
            }
            let (mut act, mut optstr, mut sep, mut explicit) = tuples[0].clone();
            let mut actions: Vec<(Act, Vec<String>)> = Vec::new();
            let stop;
            loop {
                let Some(ac) = act else {
                    extras.push(argv[start].clone());
                    stop = start + 1;
                    break;
                };
                if let Some(ex) = explicit.clone() {
                    let count = if takes_value(ac) { 1 } else { 0 };
                    let second_is_prefix = optstr.chars().nth(1) == Some('-');
                    if count == 0 && !second_is_prefix && !ex.is_empty() {
                        if sep.as_deref().map(|s| !s.is_empty()).unwrap_or(false) || ex.starts_with('-') {
                            return Err(Stop::Error(format!("argument {}: ignored explicit argument {}", name(ac), py::str_repr(&ex))));
                        }
                        actions.push((ac, vec![]));
                        let first = ex.chars().next().unwrap();
                        optstr = format!("-{}", first);
                        if let Some(next_act) = lookup(&optstr) {
                            act = Some(next_act);
                            let rest = py::skip(&ex, 1).to_string();
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
                            continue;
                        } else {
                            extras.push(format!("-{}", ex));
                            stop = start + 1;
                            break;
                        }
                    } else if count == 1 {
                        actions.push((ac, vec![ex]));
                        stop = start + 1;
                        break;
                    } else {
                        return Err(Stop::Error(format!("argument {}: ignored explicit argument {}", name(ac), py::str_repr(&ex))));
                    }
                } else {
                    let s = start + 1;
                    if takes_value(ac) {
                        if pattern.get(s) != Some(&'A') {
                            return Err(Stop::Error(format!("argument {}: expected one argument", name(ac))));
                        }
                        actions.push((ac, vec![argv[s].clone()]));
                        stop = s + 1;
                    } else {
                        actions.push((ac, vec![]));
                        stop = s;
                    }
                    break;
                }
            }
            for (ac, vals) in actions {
                take(ac, vals, &mut a)?;
            }
            start = stop;
        }
    }
    let stop = consume_positionals(start, &mut a, &mut question_done, &mut seen_question);
    extras.extend(argv[stop..].iter().cloned());
    if !seen_question {
        return Err(Stop::Error("the following arguments are required: question".into()));
    }
    if !extras.is_empty() {
        return Err(Stop::Error(format!("unrecognized arguments: {}", extras.join(" "))));
    }
    Ok(a)
}

/// The usage lines, wrapped as argparse wraps them (78 columns: an 80-column terminal less two).
pub fn usage(prog: &str) -> String {
    let width: usize = 78;
    let opt_parts = ["[-h]", "[--cwd CWD]", "[--write]", "[--shell]", "[--conversation CONVERSATION]", "[--voice]"];
    let pos_parts = ["question [question ...]"];
    let prefix = "usage: ";
    let full = format!("{} {} {}", prog, opt_parts.join(" "), pos_parts.join(" "));
    if prefix.len() + full.chars().count() <= width {
        return format!("{}{}\n", prefix, full);
    }
    let get_lines = |parts: &[&str], indent: &str, prefix: Option<&str>| -> Vec<String> {
        let mut lines = Vec::new();
        let mut line: Vec<&str> = Vec::new();
        let mut line_len = match prefix {
            Some(p) => p.chars().count() as isize - 1,
            None => indent.len() as isize - 1,
        };
        for part in parts {
            if line_len + 1 + part.chars().count() as isize > width as isize && !line.is_empty() {
                lines.push(format!("{}{}", indent, line.join(" ")));
                line.clear();
                line_len = indent.len() as isize - 1;
            }
            line.push(part);
            line_len += part.chars().count() as isize + 1;
        }
        if !line.is_empty() {
            lines.push(format!("{}{}", indent, line.join(" ")));
        }
        if prefix.is_some() {
            lines[0] = lines[0][indent.len()..].to_string();
        }
        lines
    };
    let plen = prog.chars().count();
    let lines = if (prefix.len() + plen) as f64 <= 0.75 * width as f64 {
        let indent = " ".repeat(prefix.len() + plen + 1);
        let mut parts = vec![prog];
        parts.extend(opt_parts);
        let mut l = get_lines(&parts, &indent, Some(prefix));
        l.extend(get_lines(&pos_parts, &indent, None));
        l
    } else {
        let indent = " ".repeat(prefix.len());
        let mut parts: Vec<&str> = opt_parts.to_vec();
        parts.extend(pos_parts);
        let mut l = get_lines(&parts, &indent, None);
        if l.len() > 1 {
            l = get_lines(&opt_parts, &indent, None);
            l.extend(get_lines(&pos_parts, &indent, None));
        }
        let mut all = vec![prog.to_string()];
        all.extend(l);
        all
    };
    format!("{}{}\n", prefix, lines.join("\n"))
}

pub const DOC: &str = "labyrinth-ask-sarina [--cwd DIR] [--write] [--shell] QUESTION...

Ask Sarina (the local agent) one thing, from a script or from Claude Code, and print her answer. She runs as you, in DIR
(default: the current folder), and can only read unless you pass --write or --shell for that one question.

Her sessions go to /var/lib/sarina/sessions, the folder Heimdall watches, so what she does here is watched like everything
else. The model and server come from ~/.config/labyrinth/sarina.env (KEY=value lines), then the environment.
";

/// -h: argparse's help at 80 columns.
pub fn help(prog: &str) -> String {
    format!(
        "{}\n{}\npositional arguments:\n  question\n\noptions:\n  -h, --help            show this help message and exit\n  --cwd CWD\n  --write               let her write files for this question\n  --shell               let her run shell commands for this question\n  --conversation CONVERSATION\n                        a conversation id: its turns share one session (kept\n                        in $XDG_RUNTIME_DIR/labyrinth-sarina)\n  --voice               a spoken question: reading tools and her vault only\n                        (the vault is the one place it may write)\n",
        usage(prog),
        DOC
    )
}
