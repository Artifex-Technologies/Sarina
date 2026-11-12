//! The words as the voice should say them: the text side of labyrinth-voiced, with the meaning Python's `re` gave its patterns.
//!
//! The patterns are matched by hand over characters (Python's look-behinds have no equivalent in the regex crate), with
//! Python's `\s` (`str.isspace`, which also counts `\x1c`-`\x1f`), Python's `\w` (a letter, a number or `_`) and Python's
//! `re.IGNORECASE`: two characters match when their simple lower-case forms are equal or are one of the pairs in Python's
//! `re._casefix` table (so "ı", "İ" and "i" match each other, as do "ſ" and "s", and the Kelvin sign and "k").

use crate::npy::Generator;
use sarina_voice::py;

/// How a sentence is delivered: temperature, pause before (ms), pause after (ms).
pub fn delivery(name: &str) -> (f64, i64, i64) {
    match name {
        "joking" => (0.92, 0, 150),
        "unsure" => (0.85, 0, 120),
        "urgent" => (0.72, 0, 0),
        "gentle" => (0.62, 100, 500),
        "serious" => (0.55, 350, 250),
        _ => (0.8, 0, 0),
    }
}

const TAGS: [&str; 5] = ["joking", "unsure", "urgent", "gentle", "serious"];

/// Names the model reads letter by letter unless they are spelled the way they sound.
const PRONUNCIATIONS: [(&str, &str); 4] =
    [("sarina", "Suhreenuh"), ("fylgja", "Filyah"), ("chymaera", "Kymeeruh"), ("heimdall", "Haymdawl")];

fn is_space(c: char) -> bool {
    py::is_space(c)
}

/// Python's \w
fn is_word(c: char) -> bool {
    c == '_' || py::is_alnum(c)
}

/// Python's re._casefix._EXTRA_CASES (3.14): lower-case characters that match each other under re.IGNORECASE although
/// neither is the other's lower-case form.
const EXTRA_CASES: [(u32, &[u32]); 50] = [
    (105, &[305]),
    (115, &[383]),
    (181, &[956]),
    (305, &[105]),
    (383, &[115]),
    (837, &[953, 8126]),
    (912, &[8147]),
    (944, &[8163]),
    (946, &[976]),
    (949, &[1013]),
    (952, &[977]),
    (953, &[837, 8126]),
    (954, &[1008]),
    (956, &[181]),
    (960, &[982]),
    (961, &[1009]),
    (962, &[963]),
    (963, &[962]),
    (966, &[981]),
    (976, &[946]),
    (977, &[952]),
    (981, &[966]),
    (982, &[960]),
    (1008, &[954]),
    (1009, &[961]),
    (1013, &[949]),
    (1074, &[7296]),
    (1076, &[7297]),
    (1086, &[7298]),
    (1089, &[7299]),
    (1090, &[7300, 7301]),
    (1098, &[7302]),
    (1123, &[7303]),
    (7296, &[1074]),
    (7297, &[1076]),
    (7298, &[1086]),
    (7299, &[1089]),
    (7300, &[1090, 7301]),
    (7301, &[1090, 7300]),
    (7302, &[1098]),
    (7303, &[1123]),
    (7304, &[42571]),
    (7777, &[7835]),
    (7835, &[7777]),
    (8126, &[837, 953]),
    (8147, &[912]),
    (8163, &[944]),
    (42571, &[7304]),
    (64261, &[64262]),
    (64262, &[64261]),
];

/// The simple lower-case mapping (what re's case-insensitive matching compares): one character for one.
fn simple_lower(c: char) -> char {
    if c == '\u{130}' {
        return 'i';
    }
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// Does text character `c` match pattern character `p` under re.IGNORECASE?
fn ic_eq(c: char, p: char) -> bool {
    let (lc, lp) = (simple_lower(c), simple_lower(p));
    if lc == lp {
        return true;
    }
    EXTRA_CASES.iter().find(|(k, _)| *k == lp as u32).is_some_and(|(_, v)| v.contains(&(lc as u32)))
}

/// [A-Za-z] under re.IGNORECASE
fn is_letter_ic(c: char) -> bool {
    let l = simple_lower(c);
    l.is_ascii_lowercase() || l == '\u{131}' || l == '\u{17f}'
}

/// The pronunciation table: the built-in names, then `~/.config/labyrinth/pronunciations` (lines "word = how to spell it").
/// A word given again keeps its place and takes the new spelling, as a Python dict does. Err: the file is not UTF-8.
pub fn pronunciations(home_file: &str) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = PRONUNCIATIONS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let bytes = match std::fs::read(home_file) {
        Ok(b) => b,
        Err(_) => return Ok(out),
    };
    let text = String::from_utf8(bytes).map_err(|e| format!("'utf-8' codec can't decode byte in {}: {}", home_file, e))?;
    let text = py::universal_newlines(text);
    for line in text.split_inclusive('\n') {
        if line.contains('=') && !py::lstrip(line).starts_with('#') {
            let (k, v) = line.split_once('=').unwrap();
            let (k, v) = (py::strip(k), py::strip(v));
            if !k.is_empty() && !v.is_empty() {
                let k = py::lower(k);
                match out.iter_mut().find(|(kk, _)| *kk == k) {
                    Some(e) => e.1 = v.to_string(),
                    None => out.push((k, v.to_string())),
                }
            }
        }
    }
    Ok(out)
}

/// What re.sub makes of a replacement string: backslash escapes are read (the common ones; any other is kept as written).
fn replacement(spoken: &str) -> String {
    if !spoken.contains('\\') {
        return spoken.to_string();
    }
    let mut out = String::new();
    let mut it = spoken.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            Some(e @ ('\\' | 'n' | 't' | 'r' | 'f' | 'v' | 'a' | 'b')) => {
                it.next();
                out.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'f' => '\x0c',
                    'v' => '\x0b',
                    'a' => '\x07',
                    'b' => '\x08',
                    _ => '\\',
                });
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// re.sub(r"(?<![A-Za-z])WORD(?![A-Za-z])", spoken, text, flags=re.IGNORECASE) for each word of the table, in order.
pub fn respell(text: &str, table: &[(String, String)]) -> String {
    let mut text: Vec<char> = text.chars().collect();
    for (word, spoken) in table {
        let w: Vec<char> = word.chars().collect();
        if w.is_empty() {
            continue;
        }
        let spoken: Vec<char> = replacement(spoken).chars().collect();
        let mut out = Vec::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let fits = i + w.len() <= text.len()
                && (i == 0 || !is_letter_ic(text[i - 1]))
                && w.iter().enumerate().all(|(j, &p)| ic_eq(text[i + j], p))
                && (i + w.len() == text.len() || !is_letter_ic(text[i + w.len()]));
            if fits {
                out.extend_from_slice(&spoken);
                i += w.len();
            } else {
                out.push(text[i]);
                i += 1;
            }
        }
        text = out;
    }
    text.into_iter().collect()
}

/// Emoji and pictographs (the voice would try to read them out).
fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE0F | 0x200D)
}

/// [joking] -> «joking» (any case), so the tag survives cleaning and sentence splitting.
pub fn mark_delivery(text: &str) -> String {
    let t: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    'outer: while i < t.len() {
        if t[i] == '[' {
            for tag in TAGS {
                let n = tag.len();
                if i + 1 + n < t.len() && t[i + 1 + n] == ']' && tag.chars().enumerate().all(|(j, p)| ic_eq(t[i + 1 + j], p)) {
                    let word: String = t[i + 1..i + 1 + n].iter().collect();
                    out.push_str(" «");
                    out.push_str(&py::lower(&word));
                    out.push_str("» ");
                    i += n + 2;
                    continue 'outer;
                }
            }
        }
        out.push(t[i]);
        i += 1;
    }
    out
}

/// MARK_RE.match(s): ^\s*«(\w+)»\s*  -> (the word, the end of the match), in characters.
fn mark_at_start(t: &[char]) -> Option<(String, usize)> {
    let mut i = 0;
    while i < t.len() && is_space(t[i]) {
        i += 1;
    }
    let (word, end) = mark_at(t, i)?;
    let mut e = end;
    while e < t.len() && is_space(t[e]) {
        e += 1;
    }
    Some((word, e))
}

/// «\w+» starting at i: (the word, the index after »)
fn mark_at(t: &[char], i: usize) -> Option<(String, usize)> {
    if t.get(i) != Some(&'«') {
        return None;
    }
    let mut j = i + 1;
    while j < t.len() && is_word(t[j]) {
        j += 1;
    }
    if j == i + 1 || t.get(j) != Some(&'»') {
        return None;
    }
    Some((t[i + 1..j].iter().collect(), j + 1))
}

/// (delivery name or "", the sentence without its tag(s))
pub fn take_delivery(sentence: &str) -> (String, String) {
    let mut t: Vec<char> = sentence.chars().collect();
    let mut name = String::new();
    if let Some((w, end)) = mark_at_start(&t) {
        name = w;
        t.drain(..end);
    }
    // re.sub(r"\s*«\w+»\s*", " ", sentence)
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        let mut j = i;
        while j < t.len() && is_space(t[j]) {
            j += 1;
        }
        if let Some((_, mut e)) = mark_at(&t, j) {
            while e < t.len() && is_space(t[e]) {
                e += 1;
            }
            out.push(' ');
            i = e;
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    (name, py::strip(&out).to_string())
}

/// The words as the model should say them for this delivery: a chuckle after a joke, a hesitation when unsure (each only
/// sometimes, drawn from the request's generator), an exclamation when urgent.
pub fn dress(sentence: &str, name: &str, rng: &mut Generator) -> String {
    let s = sentence;
    let last = s.chars().last();
    if name == "joking" && matches!(last, Some('.' | '!' | '?')) && rng.random() < 0.6 {
        format!("{} [chuckle]", s)
    } else if name == "unsure" && rng.random() < 0.7 {
        if s.is_empty() {
            return String::new(); // (Python's expression draws the hesitation only for a non-empty sentence)
        }
        let lead = *rng.choice(&["Hmm... ", "Um... ", "Well... "]);
        let first = s.chars().next().unwrap();
        format!("{}{}{}", lead, py::lower(&first.to_string()), &s[first.len_utf8()..])
    } else if name == "urgent" {
        let keep = s.ends_with('!') || s.ends_with('?');
        format!("{}{}", s.trim_end_matches('.'), if keep { "" } else { "!" })
    } else {
        s.to_string()
    }
}

/// Plain words: no emoji, no [tags] or <tags> (the model would act them out), no markup, one space between words, no
/// space before punctuation, and the names respelled.
pub fn clean(text: &str, table: &[(String, String)]) -> String {
    let t: Vec<char> = text.chars().filter(|&c| !is_emoji(c)).collect();
    // \[[^\]]*\]|<[^>]*>  ->  " "
    let mut a: Vec<char> = Vec::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        let close = match t[i] {
            '[' => Some(']'),
            '<' => Some('>'),
            _ => None,
        };
        if let Some(close) = close {
            if let Some(k) = t[i + 1..].iter().position(|&c| c == close) {
                a.push(' ');
                i += k + 2;
                continue;
            }
        }
        a.push(t[i]);
        i += 1;
    }
    // [*`#_]+ -> ""
    a.retain(|c| !matches!(c, '*' | '`' | '#' | '_'));
    // \s+ -> " ", then strip
    let mut b: Vec<char> = Vec::with_capacity(a.len());
    for c in a {
        if is_space(c) {
            if b.last() != Some(&' ') || b.is_empty() {
                b.push(' ');
            }
        } else {
            b.push(c);
        }
    }
    while b.last() == Some(&' ') {
        b.pop();
    }
    let start = b.iter().position(|&c| c != ' ').unwrap_or(b.len());
    // \s+([.,!?;:]) -> \1 (only single spaces are left)
    let mut out = String::with_capacity(b.len());
    for k in start..b.len() {
        if b[k] == ' ' && k + 1 < b.len() && matches!(b[k + 1], '.' | ',' | '!' | '?' | ';' | ':') {
            continue;
        }
        out.push(b[k]);
    }
    respell(&out, table)
}

/// Sentences, with very short ones joined to the next (they sound clipped alone), a delivery tag always starting a new
/// one, and very long ones cut at a comma (or a space).
pub fn split_sentences(text: &str) -> Vec<String> {
    const MIN_LEN: usize = 28;
    const MAX_LEN: usize = 240;
    // re.split(r"(?<=[.!?])\s+", text), each part stripped, empty ones dropped
    let t: Vec<char> = text.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < t.len() {
        if is_space(t[i]) && i > 0 && matches!(t[i - 1], '.' | '!' | '?') {
            let mut j = i;
            while j < t.len() && is_space(t[j]) {
                j += 1;
            }
            parts.push(t[start..i].iter().collect());
            start = j;
            i = j;
        } else {
            i += 1;
        }
    }
    parts.push(t[start..].iter().collect());
    let parts: Vec<String> = parts.iter().map(|p| py::strip(p).to_string()).filter(|p| !p.is_empty()).collect();

    let is_mark = |s: &str| mark_at_start(&s.chars().collect::<Vec<_>>()).is_some();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for p in parts {
        if is_mark(&p) && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur = if cur.is_empty() { p } else { py::strip(&format!("{} {}", cur, p)).to_string() };
        if py::clen(&cur) >= MIN_LEN {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        if !out.is_empty() && py::clen(&cur) < MIN_LEN && !is_mark(&cur) {
            let last = out.last_mut().unwrap();
            last.push(' ');
            last.push_str(&cur);
        } else {
            out.push(cur);
        }
    }
    let mut fin: Vec<String> = Vec::new();
    for s in out {
        let mut s: Vec<char> = s.chars().collect();
        while s.len() > MAX_LEN {
            let rfind = |c: char| s[..MAX_LEN].iter().rposition(|&x| x == c);
            let comma = rfind(',').filter(|&k| k > 40);
            let cut = match comma.or_else(|| rfind(' ')) {
                Some(k) => k,
                // No comma after the 40th character and no space: Python's loop never ended here (the voice stopped
                // answering); this cuts at the limit instead.
                None => MAX_LEN - 1,
            };
            fin.push(py::strip(&s[..cut + 1].iter().collect::<String>()).to_string());
            s = py::strip(&s[cut + 1..].iter().collect::<String>()).chars().collect();
        }
        fin.push(s.into_iter().collect());
    }
    fin.into_iter()
        .filter(|s| !s.is_empty())
        .map(|s| if matches!(s.chars().last(), Some('.' | '!' | '?' | ',')) { s } else { s + "." })
        .collect()
}

/// The sentences of a request with their delivery: (name, plain words), empty ones left out.
pub fn sentences(text: &str, table: &[(String, String)]) -> Vec<(String, String)> {
    split_sentences(&clean(&mark_delivery(text), table)).iter().map(|s| take_delivery(s)).filter(|(_, p)| !p.is_empty()).collect()
}
