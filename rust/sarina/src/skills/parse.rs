//! Go: `skills/parse.go`: a skill file: front matter between `---` lines (a small subset of YAML: flat `key: value`
//! lines, `#` comments, single or double quotes as Go's `strconv.Unquote` reads them, booleans, lists inline `[a, b]`, as
//! `- item` lines or comma separated), then the prompt body.
//!
//! The skill's texts are kept as strings (a byte that is not UTF-8 becomes U+FFFD, as Go's JSON encoder writes it when the
//! text reaches the model); error texts quote the original bytes as Go does.

use crate::gojson::quote_bytes;
use crate::gotext;
use std::collections::BTreeMap;

/// One parsed skill (Go's `Skill`, the parts the file gives).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    /// meaningful only with `has_allowed`
    pub allowed_tools: Vec<String>,
    /// allowed_tools was present (an empty list then means no tools)
    pub has_allowed: bool,
    /// default true
    pub user_invocable: bool,
    /// default false
    pub model_invocable: bool,
    pub body: String,
    pub path: String,
    /// "operator" or "workspace"
    pub source: String,
    /// context looked up before the model starts (front matter "prefetch"); None when absent
    pub prefetch: Option<Vec<String>>,
}

impl Skill {
    /// Go's `Expand`: the body with every `$ARGUMENTS` replaced (the arguments are not expanded again).
    pub fn expand(&self, args: &str) -> String {
        self.body.replace("$ARGUMENTS", args)
    }
}

/// Go's `ValidName`: `^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$`.
pub fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name[0].is_ascii_alphanumeric()
        && name[1..].iter().all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'))
}

/// Go's `strconv.UnquoteChar` for a double-quoted string: the bytes of one character and the rest.
fn unquote_char(s: &[u8]) -> Option<(Vec<u8>, &[u8])> {
    let c = *s.first()?;
    if c == b'"' {
        return None;
    }
    if c >= 0x80 {
        // a byte that is not UTF-8 is U+FFFD (written as a rune)
        let (r, n) = gotext::decode_rune(s);
        return Some((r.unwrap_or('\u{FFFD}').to_string().into_bytes(), &s[n.max(1)..]));
    }
    if c != b'\\' {
        return Some((vec![c], &s[1..]));
    }
    let e = *s.get(1)?;
    let s = &s[2..];
    let simple = |b: u8| Some((vec![b], s));
    match e {
        b'a' => simple(7),
        b'b' => simple(8),
        b'f' => simple(12),
        b'n' => simple(b'\n'),
        b'r' => simple(b'\r'),
        b't' => simple(b'\t'),
        b'v' => simple(11),
        b'x' | b'u' | b'U' => {
            let n = match e {
                b'x' => 2,
                b'u' => 4,
                _ => 8,
            };
            let digits = s.get(..n)?;
            let mut v: u32 = 0;
            for &d in digits {
                v = v << 4 | (d as char).to_digit(16)?;
            }
            let rest = &s[n..];
            if e == b'x' {
                // one byte, possibly not UTF-8
                return Some((vec![v as u8], rest));
            }
            let ch = char::from_u32(v)?;
            Some((ch.to_string().into_bytes(), rest))
        }
        b'0'..=b'7' => {
            let mut v = (e - b'0') as u32;
            let digits = s.get(..2)?;
            for &d in digits {
                if !(b'0'..=b'7').contains(&d) {
                    return None;
                }
                v = v << 3 | (d - b'0') as u32;
            }
            if v > 255 {
                return None;
            }
            Some((vec![v as u8], &s[2..]))
        }
        b'\\' => simple(b'\\'),
        b'"' => simple(b'"'),
        _ => None,
    }
}

/// Go's `strconv.Unquote` of a double-quoted string (the only kind skills hand it).
fn go_unquote_double(v: &[u8]) -> Option<Vec<u8>> {
    if v.len() < 2 || v[0] != b'"' {
        return None;
    }
    let end = v[1..].iter().position(|&c| c == b'"')? + 2;
    let head = &v[..end];
    if !head.contains(&b'\\') && !head.contains(&b'\n') && std::str::from_utf8(&head[1..end - 1]).is_ok() {
        // no escapes: the text itself, and nothing may follow the closing quote
        return (end == v.len()).then(|| head[1..end - 1].to_vec());
    }
    let mut out = Vec::new();
    let mut s = &v[1..];
    while let Some(&c) = s.first() {
        if c == b'"' {
            break;
        }
        if c == b'\n' {
            return None;
        }
        let (bytes, rest) = unquote_char(s)?;
        out.extend_from_slice(&bytes);
        s = rest;
    }
    if s.first() != Some(&b'"') || s.len() != 1 {
        return None;
    }
    Some(out)
}

/// Go's `unquote` of the skills package.
fn unquote(v: &[u8]) -> Vec<u8> {
    if v.len() >= 2 && v[0] == b'"' && v[v.len() - 1] == b'"' {
        return go_unquote_double(v).unwrap_or_else(|| v[1..v.len() - 1].to_vec());
    }
    if v.len() >= 2 && v[0] == b'\'' && v[v.len() - 1] == b'\'' {
        let inner = &v[1..v.len() - 1];
        let mut out = Vec::with_capacity(inner.len());
        let mut i = 0;
        while i < inner.len() {
            if inner[i] == b'\'' && inner.get(i + 1) == Some(&b'\'') {
                out.push(b'\'');
                i += 2;
            } else {
                out.push(inner[i]);
                i += 1;
            }
        }
        return out;
    }
    v.to_vec()
}

#[derive(Clone, Default)]
struct FmValue {
    text: Vec<u8>,
    list: Vec<Vec<u8>>,
    is_list: bool,
}

impl FmValue {
    /// The list form, or the scalar split at commas.
    fn items(&self) -> Vec<String> {
        if self.is_list {
            return self.list.iter().map(|x| gotext::lossy(x)).collect();
        }
        self.text.split(|&c| c == b',').map(gotext::trim_space_bytes).filter(|x| !x.is_empty()).map(gotext::lossy).collect()
    }
}

fn trim_right_blank(b: &[u8]) -> &[u8] {
    let mut b = b;
    while let [rest @ .., b' ' | b'\t'] = b {
        b = rest;
    }
    b
}

fn parse_front_matter(lines: &[&[u8]]) -> Result<BTreeMap<Vec<u8>, FmValue>, String> {
    let mut fm: BTreeMap<Vec<u8>, FmValue> = BTreeMap::new();
    let mut open: Option<Vec<u8>> = None;
    for (n, raw) in lines.iter().enumerate() {
        let line = trim_right_blank(raw);
        let trimmed = gotext::trim_space_bytes(line);
        if trimmed.is_empty() || trimmed.starts_with(b"#") {
            continue;
        }
        if trimmed == b"-" || trimmed.starts_with(b"- ") {
            let Some(key) = &open else {
                return Err(format!("front matter line {}: list item with no key", n + 1));
            };
            let item = unquote(gotext::trim_space_bytes(&trimmed[1..]));
            fm.entry(key.clone()).or_default().list.push(item);
            continue;
        }
        if line[0] == b' ' || line[0] == b'\t' {
            return Err(format!("front matter line {}: nested or multi-line values are not supported", n + 1));
        }
        let Some(colon) = line.iter().position(|&c| c == b':') else {
            return Err(format!("front matter line {}: expected 'key: value'", n + 1));
        };
        let key = gotext::trim_space_bytes(&line[..colon]).to_vec();
        if key.is_empty() {
            return Err(format!("front matter line {}: expected 'key: value'", n + 1));
        }
        let val = gotext::trim_space_bytes(&line[colon + 1..]);
        if fm.contains_key(&key) {
            return Err(format!("front matter line {}: duplicate key {}", n + 1, quote_bytes(&key)));
        }
        open = None;
        if val.is_empty() {
            fm.insert(key.clone(), FmValue::default());
            open = Some(key);
        } else if val[0] == b'[' {
            if !val.ends_with(b"]") {
                return Err(format!("front matter line {}: unterminated list", n + 1));
            }
            let mut items = Vec::new();
            let inner = gotext::trim_space_bytes(&val[1..val.len() - 1]);
            if !inner.is_empty() {
                for it in inner.split(|&c| c == b',') {
                    let it = unquote(gotext::trim_space_bytes(it));
                    if !it.is_empty() {
                        items.push(it);
                    }
                }
            }
            fm.insert(key, FmValue { list: items, is_list: true, ..FmValue::default() });
        } else if b"{|>&*!".contains(&val[0]) {
            return Err(format!("front matter line {}: unsupported YAML construct in {}", n + 1, quote_bytes(&key)));
        } else {
            fm.insert(key, FmValue { text: unquote(val), ..FmValue::default() });
        }
    }
    // "key:" followed by "- item" lines is a list; with no items it is empty
    for v in fm.values_mut() {
        if !v.list.is_empty() {
            v.is_list = true;
        }
    }
    Ok(fm)
}

/// Go's `Parse`: a skill file; `fallback_name` when it names none.
pub fn parse(data: &[u8], fallback_name: &str) -> Result<Skill, String> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let mut text = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' && data.get(i + 1) == Some(&b'\n') {
            text.push(b'\n');
            i += 2;
        } else {
            text.push(data[i]);
            i += 1;
        }
    }
    let lines: Vec<&[u8]> = text.split(|&c| c == b'\n').collect();
    if trim_right_blank(lines[0]) != b"---" {
        return Err("no front matter: the file must start with a '---' line".into());
    }
    let Some(end) = (1..lines.len()).find(|&i| trim_right_blank(lines[i]) == b"---") else {
        return Err("front matter is not closed by a '---' line".into());
    };
    let fm = parse_front_matter(&lines[1..end])?;
    let scalar = |k: &str| fm.get(k.as_bytes()).map(|v| v.text.clone()).unwrap_or_default();
    let body = lines[end + 1..].join(&b'\n');
    let mut name = scalar("name");
    if name.is_empty() {
        name = fallback_name.as_bytes().to_vec();
    }
    if !valid_name(&name) {
        return Err(format!("invalid skill name {} (letters, digits, . _ : - ; at most 64 characters)", quote_bytes(&name)));
    }
    let mut s = Skill {
        name: gotext::lossy(&name),
        description: gotext::lossy(&scalar("description")),
        when_to_use: gotext::lossy(&scalar("when_to_use")),
        user_invocable: true,
        body: gotext::lossy(gotext::trim_space_bytes(&body)),
        ..Skill::default()
    };
    if let Some(v) = fm.get(b"prefetch".as_slice()) {
        s.prefetch = Some(v.items());
    }
    if let Some(v) = fm.get(b"allowed_tools".as_slice()) {
        s.has_allowed = true;
        s.allowed_tools = v.items();
    }
    for key in ["user_invocable", "model_invocable"] {
        if let Some(v) = fm.get(key.as_bytes()) {
            let set = match gotext::to_lower(&gotext::lossy(&v.text)).as_str() {
                "true" | "yes" => true,
                "false" | "no" => false,
                _ => return Err(format!("{key} must be true or false, got {}", quote_bytes(&v.text))),
            };
            if key == "user_invocable" {
                s.user_invocable = set;
            } else {
                s.model_invocable = set;
            }
        }
    }
    if s.body.is_empty() {
        return Err("the skill has no body".into());
    }
    Ok(s)
}
