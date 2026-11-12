//! Go: `go/skills/library.go`: long skills. At most 64 KiB of a skill go into the prompt; a longer one is split into
//! sections at its headings (long sections into parts of at most 12 KiB), the prompt gets its introduction and a table
//! of contents, and the sections are read when needed (skill_section) or found by meaning (skill_search). A skill folder
//! may keep its sections as files: `<skill>/sections/*.md` (testdata/parity/skills.json).

use std::sync::LazyLock;

use regex::Regex;

use super::discover::{Problem, Scanner, Trust};
use super::parse::Skill;
use crate::gotext::{self, trim_space};
use crate::{goerr, gopath, goregexp};

/// The most read of one skill or section file.
pub const MAX_FILE_BYTES: u64 = 4 << 20;
/// The most of a section returned at once.
pub const MAX_SECTION_BYTES: usize = 12 << 10;
const INTRO_BYTES: usize = 6 << 10;
const TOC_BYTES: usize = 8 << 10;
const MAX_SECTIONS: usize = 2000;

/// One part of a long skill.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub title: String,
    pub text: String,
}

pub(crate) static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| goregexp::compile(r"(?m)^(#{1,3})[ \t]+(.+?)[ \t]*#*[ \t]*$").expect("a fixed pattern"));

/// Go's `splitSections`: a long body cut at its headings ("##" when it has two or more, else "#", else "###"); the text
/// before the first is the introduction; long sections become parts. Without headings the body is one "Text" section.
pub fn split_sections(body: &str) -> (String, Vec<Section>) {
    let caps: Vec<regex::Captures> = HEADING_RE.captures_iter(body).collect();
    let level = ["##", "#", "###"].into_iter().find(|l| caps.iter().filter(|c| &c[1] == *l).count() >= 2);
    let mut intro = String::new();
    let mut raw: Vec<Section> = Vec::new();
    match level {
        None => raw.push(Section { title: "Text".into(), text: body.to_owned() }),
        Some(level) => {
            let mut starts = Vec::new();
            let mut titles = Vec::new();
            for c in &caps {
                if &c[1] == level {
                    starts.push(c.get(0).map_or(0, |m| m.start()));
                    titles.push(trim_space(&c[2]).to_owned());
                }
            }
            intro = trim_space(&body[..starts[0]]).to_owned();
            for (i, &st) in starts.iter().enumerate() {
                let end = starts.get(i + 1).copied().unwrap_or(body.len());
                raw.push(Section { title: titles[i].clone(), text: trim_space(&body[st..end]).to_owned() });
            }
        }
    }
    let mut out = Vec::new();
    for s in raw {
        let parts = cut_parts(&s.text, MAX_SECTION_BYTES);
        let n = parts.len();
        for (i, p) in parts.into_iter().enumerate() {
            let title = if n > 1 { format!("{} (part {} of {n})", s.title, i + 1) } else { s.title.clone() };
            out.push(Section { title, text: p });
        }
    }
    out.truncate(MAX_SECTIONS);
    (intro, out)
}

fn last_index(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Go's `cutParts`: pieces of at most max bytes, cut at paragraph breaks where it can, else at line breaks, else on a
/// UTF-8 boundary.
pub fn cut_parts(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut text = text;
    while text.len() > max {
        let b = text.as_bytes();
        let half = (max / 2) as i64;
        let mut cut = last_index(&b[..max], b"\n\n").map_or(-1, |i| i as i64);
        if cut < half {
            cut = last_index(&b[..max], b"\n").map_or(-1, |i| i as i64);
        }
        let mut cut = if cut < half { max } else { cut as usize };
        while cut > 0 && b[cut] & 0xC0 == 0x80 {
            cut -= 1;
        }
        if cut == 0 {
            // a piece smaller than one character (never with the sizes used): take that character whole rather than loop
            cut = text.chars().next().map_or(text.len(), char::len_utf8);
        }
        out.push(trim_space(&text[..cut]).to_owned());
        text = trim_space(&text[cut..]);
    }
    if !trim_space(text).is_empty() {
        out.push(trim_space(text).to_owned());
    }
    out
}

/// Go's `readCapped`: a file of at most MAX_FILE_BYTES.
pub fn read_capped(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| goerr::path("open", path.as_bytes(), &e))?;
    let st = f.metadata().map_err(|e| goerr::path("stat", path.as_bytes(), &e))?;
    if st.len() > MAX_FILE_BYTES {
        return Err(format!("file is larger than {MAX_FILE_BYTES} bytes"));
    }
    let mut data = Vec::new();
    f.take(st.len()).read_to_end(&mut data).map_err(|e| goerr::path("read", path.as_bytes(), &e))?;
    Ok(data)
}

fn ext_of(name: &str) -> &str {
    name.rfind('.').map_or("", |i| &name[i..])
}

/// Go's `sectionFiles`: `<dir>/sections/*.md` sorted by name, each titled by its first heading (else its file name).
pub fn section_files(dir: &str, sc: &Scanner, tr: Option<&Trust>) -> (Vec<Section>, Vec<Problem>) {
    let sdir = format!("{dir}/sections");
    let Ok(entries) = gopath::read_dir(sdir.as_bytes()) else { return (Vec::new(), Vec::new()) };
    let mut out = Vec::new();
    let mut probs = Vec::new();
    for e in &entries {
        let name = gotext::lossy(&e.name);
        let p = format!("{sdir}/{name}");
        if e.kind != gopath::Kind::Regular || !ext_of(&name).eq_ignore_ascii_case(".md") {
            continue;
        }
        if let Some(tr) = tr {
            let why = tr(&p);
            if !why.is_empty() {
                probs.push(Problem { path: p, message: why });
                continue;
            }
        }
        let data = match read_capped(&p) {
            Ok(d) => d,
            Err(err) => {
                probs.push(Problem { path: p, message: err });
                continue;
            }
        };
        let (blocked, reason) = sc(&data);
        if blocked {
            probs.push(Problem { path: p, message: format!("blocked by the security scan: {reason}") });
            continue;
        }
        let text = trim_space(&gotext::lossy(&data)).to_owned();
        let mut title = name[..name.len() - ext_of(&name).len()].to_owned();
        if let Some(m) = HEADING_RE.captures(&text) {
            title = trim_space(&m[2]).to_owned();
        }
        let parts = cut_parts(&text, MAX_SECTION_BYTES);
        let n = parts.len();
        for (i, pt) in parts.into_iter().enumerate() {
            let t = if n > 1 { format!("{title} (part {} of {n})", i + 1) } else { title.clone() };
            out.push(Section { title: t, text: pt });
        }
        if out.len() >= MAX_SECTIONS {
            break;
        }
    }
    (out, probs)
}

impl Skill {
    /// Go's `makeLibrary`: the skill made a long one; authored: the introduction is the skill's own SKILL.md text (kept
    /// whole), else it was cut from an oversized file and is shortened.
    pub fn make_library(&mut self, intro: &str, sections: Vec<Section>, authored: bool) {
        let mut b = String::new();
        if !intro.is_empty() {
            let mut intro = intro.to_owned();
            if !authored && intro.len() > INTRO_BYTES {
                intro = format!("{}\n[…]", cut_parts(&intro, INTRO_BYTES)[0]);
            }
            b.push_str(&intro);
            b.push_str("\n\n");
        }
        b.push_str(&format!(
            "This skill is long: its {} sections are not in this message. Open one with the skill_section tool (skill {} and the \
             section's number or title), or find passages by meaning with skill_search. Read only what the task needs.\n\nSections:\n",
            sections.len(),
            gotext::quote(&self.name)
        ));
        let mut toc = 0;
        for (i, sec) in sections.iter().enumerate() {
            let line = format!("{}. {}\n", i + 1, sec.title);
            if toc + line.len() > TOC_BYTES {
                b.push_str(&format!("… and {} more (skill_search finds them)\n", sections.len() - i));
                break;
            }
            b.push_str(&line);
            toc += line.len();
        }
        self.sections = sections;
        self.body = trim_space(&b).to_owned();
        if !self.body.contains("$ARGUMENTS") {
            self.body.push_str("\n\nThe task: $ARGUMENTS");
        }
    }

    /// Go's `Section`: a section by its number (1 first) or its title (whole, else the first that holds it), ignoring case.
    pub fn section(&self, which: &str) -> Option<(usize, &Section)> {
        let which = trim_space(which);
        if let Some(n) = gotext::atoi(which.strip_suffix('.').unwrap_or(which).as_bytes()) {
            if n >= 1 && (n as usize) <= self.sections.len() {
                return Some((n as usize, &self.sections[n as usize - 1]));
            }
            return None;
        }
        let low = gotext::to_lower(which);
        if let Some(i) = self.sections.iter().position(|s| gotext::to_lower(&s.title) == low) {
            return Some((i + 1, &self.sections[i]));
        }
        if !low.is_empty() {
            if let Some(i) = self.sections.iter().position(|s| gotext::to_lower(&s.title).contains(&low)) {
                return Some((i + 1, &self.sections[i]));
            }
        }
        None
    }

    /// Whether the skill has sections to read on demand.
    pub fn is_library(&self) -> bool {
        !self.sections.is_empty()
    }
}
