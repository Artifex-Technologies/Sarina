//! espeak-ng's library (`libespeak-ng.so.1`, Arch: espeak-ng), loaded at run time with dlopen: nothing is linked at build
//! time. Text to IPA phonemes clause by clause, as piper's `espeakbridge` asks for them.
//!
//! piper 1.8 calls `espeak_TextToPhonemesWithTerminator` from its own copy of espeak-ng, which also says which punctuation
//! ended each clause; the system's espeak-ng 1.52 has only `espeak_TextToPhonemes`, so the terminator is read from the text
//! the clause took (its last punctuation mark, closing quotes and brackets passed over). `tests/differential.py` compares
//! the clauses with piper's own on a corpus.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};

/// The library file (`SARINA_ESPEAK_LIB` names another).
pub const DEFAULT_LIB: &str = "libespeak-ng.so.1";

type Initialize = unsafe extern "C" fn(c_int, c_int, *const c_char, c_int) -> c_int;
type SetVoiceByName = unsafe extern "C" fn(*const c_char) -> c_int;
type TextToPhonemes = unsafe extern "C" fn(*mut *const c_void, c_int, c_int) -> *const c_char;

struct Lib {
    set_voice: SetVoiceByName,
    to_phonemes: TextToPhonemes,
}

// the functions are plain C entry points of a library that stays loaded for the life of the process
unsafe impl Send for Lib {}

static LIB: OnceLock<Result<Mutex<Lib>, String>> = OnceLock::new();

const AUDIO_OUTPUT_SYNCHRONOUS: c_int = 2;
const CHARS_AUTO: c_int = 0;
const PHONEMES_IPA: c_int = 0x02;
/// Put between phonemes (bits 8-23 of the phoneme mode): the invisible separator, which no phoneme contains. It shows where
/// espeak-ng's linking phoneme `;` sits, which the system's espeak-ng writes as nothing and piper's own build as "ʲ".
const SEP: char = '\u{2063}';

fn sym<T>(h: *mut c_void, name: &str) -> Result<T, String> {
    let c = CString::new(name).map_err(|e| e.to_string())?;
    let p = unsafe { libc::dlsym(h, c.as_ptr()) };
    if p.is_null() {
        return Err(format!("espeak-ng has no {name}"));
    }
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

fn dlerror() -> String {
    let e = unsafe { libc::dlerror() };
    if e.is_null() {
        "unknown error".into()
    } else {
        unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
    }
}

/// The library, loaded and initialised once. The data folder is espeak-ng's own (`ESPEAK_DATA_PATH` names another).
fn lib() -> Result<&'static Mutex<Lib>, String> {
    LIB.get_or_init(|| {
        let name = std::env::var("SARINA_ESPEAK_LIB").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_LIB.into());
        let c = CString::new(name.clone()).map_err(|e| e.to_string())?;
        let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            return Err(format!("cannot load {name}: {}", dlerror()));
        }
        let init: Initialize = sym(h, "espeak_Initialize")?;
        let set_voice: SetVoiceByName = sym(h, "espeak_SetVoiceByName")?;
        let to_phonemes: TextToPhonemes = sym(h, "espeak_TextToPhonemes")?;
        // 0x8000: espeakINITIALIZE_DONT_EXIT (a missing data folder is an error here, not the end of the process)
        let rate = unsafe { init(AUDIO_OUTPUT_SYNCHRONOUS, 0, std::ptr::null(), 0x8000) };
        if rate <= 0 {
            return Err("espeak-ng could not start (is its data folder there?)".into());
        }
        Ok(Mutex::new(Lib { set_voice, to_phonemes }))
    })
    .as_ref()
    .map_err(|e| e.clone())
}

pub fn available() -> Result<(), String> {
    lib().map(|_| ())
}

/// The separated phonemes joined. An empty phoneme is one espeak-ng writes as nothing in IPA: a pause, or the linking
/// phoneme `;` that espeak-ng puts between a front vowel and the next vowel ("three open"), which piper's own espeak-ng
/// build writes as "ʲ" (θɹˈiːʲ ˈoʊpən). So "ʲ" goes where an empty phoneme follows i, iː or ɪ and a vowel comes next;
/// other empty phonemes stay nothing. Checked against piper's bridge on Sarina's docs (tests/differential.py).
fn linking(separated: &str) -> String {
    const VOWELS: &str = "aeiouyæɐɑɒɔəɚɛɜɝɞɘɪʊʌʏɤɯøœɶᵻ";
    let tokens: Vec<(usize, &str)> = separated.split(' ').enumerate().flat_map(|(w, word)| word.split(SEP).map(move |t| (w, t))).collect();
    let mut out = String::new();
    for (i, &(w, t)) in tokens.iter().enumerate() {
        if i > 0 && tokens[i - 1].0 != w {
            out.push(' ');
        }
        if !t.is_empty() {
            out.push_str(t);
            continue;
        }
        // one empty phoneme alone (a pause is two), after the vowel i, iː or ɪ (not a diphthong ending in ɪ)
        let lone = tokens.get(i + 1).is_none_or(|x| !x.1.is_empty() || x.0 != w)
            && i.checked_sub(1).is_some_and(|j| !tokens[j].1.is_empty() && tokens[j].0 == w);
        let before = if i > 0 { tokens[i - 1].1 } else { "" };
        let after = tokens[i + 1..].iter().map(|x| x.1).find(|x| !x.is_empty()).unwrap_or("");
        let front = matches!(before.trim_start_matches(['\u{2c8}', '\u{2cc}']), "i" | "i\u{2d0}" | "\u{26a}");
        let vowel = after.trim_start_matches(['\u{2c8}', '\u{2cc}']).starts_with(|c| VOWELS.contains(c));
        if lone && front && vowel {
            out.push('\u{2b2}');
        }
    }
    out
}

/// One clause: its IPA, the punctuation that ended it ("." "?" "!" "," ":" ";" or ""), whether it ended a sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    pub phonemes: String,
    pub terminator: &'static str,
    pub end_of_sentence: bool,
}

/// The punctuation ending a clause, read from the text it took (without the one character espeak-ng reads ahead), as
/// espeak-ng classifies it (checked against piper's bridge, tests/differential.py): the first mark of a run decides ("?!"
/// is a question), an ellipsis ends nothing, a dash is a semicolon, closing quotes and brackets are passed over.
pub fn terminator(segment: &str, last: bool) -> (&'static str, bool) {
    let closing = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '}' | '\u{201d}' | '\u{2019}' | '\u{bb}');
    let t = segment.trim_end_matches(closing);
    let marks = |c: char| matches!(c, '.' | '?' | '!' | ',' | ':' | ';');
    let body = t.trim_end_matches(marks);
    let run = &t[body.len()..];
    if run.ends_with("..") || t.ends_with('\u{2026}') {
        return ("", false);
    }
    if t.ends_with('\u{2014}') || t.ends_with('\u{2013}') || (t.ends_with('-') && body.trim_end_matches('-').ends_with(char::is_whitespace))
    {
        return (";", false);
    }
    match run.chars().next() {
        Some('.') => (".", true),
        Some('?') => ("?", true),
        Some('!') => ("!", true),
        Some(',') => (",", false),
        Some(':') => (":", false),
        Some(';') => (";", false),
        // the end of the text, or a paragraph break, ends a sentence without a mark
        _ => ("", last || segment.contains("\n\n")),
    }
}

/// `espeakbridge.set_voice(voice)` then `get_phonemes(text)`.
pub fn phonemize(voice: &str, text: &str) -> Result<Vec<Clause>, String> {
    let lib = lib()?.lock().unwrap_or_else(|e| e.into_inner());
    let v = CString::new(voice).map_err(|e| e.to_string())?;
    if unsafe { (lib.set_voice)(v.as_ptr()) } != 0 {
        return Err(format!("espeak-ng has no voice {voice:?}"));
    }
    // a NUL ends the text for espeak-ng, as it did for the Python's C string
    let cut = text.split('\0').next().unwrap_or("");
    let c = CString::new(cut).map_err(|e| e.to_string())?;
    let base = c.as_ptr() as usize;
    let end = base + cut.len();
    let mut ptr: *const c_void = c.as_ptr() as *const c_void;
    let mut out = Vec::new();
    while !ptr.is_null() {
        let start = ptr as usize;
        let ph = unsafe { (lib.to_phonemes)(&mut ptr, CHARS_AUTO, PHONEMES_IPA | ((SEP as c_int) << 8)) };
        let phonemes = if ph.is_null() { String::new() } else { linking(&unsafe { CStr::from_ptr(ph) }.to_string_lossy()) };
        let stop = if ptr.is_null() { end } else { (ptr as usize).clamp(start, end) };
        let mut segment = String::from_utf8_lossy(&cut.as_bytes()[start - base..stop - base]).into_owned();
        if !ptr.is_null() {
            // the first character of the next clause, already read
            segment.pop();
        }
        let (terminator, end_of_sentence) = terminator(&segment, ptr.is_null());
        out.push(Clause { phonemes, terminator, end_of_sentence });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::terminator;

    #[test]
    fn terminators() {
        assert_eq!(terminator("Hello, ", false), (",", false));
        assert_eq!(terminator("He said \"hi.\" ", false), (".", true));
        assert_eq!(terminator("why?", true), ("?", true));
        assert_eq!(terminator("no mark", true), ("", true));
        assert_eq!(terminator("a dash - ", false), (";", false));
        assert_eq!(terminator("Question?! ", false), ("?", true));
        assert_eq!(terminator("Wait... ", false), ("", false));
        assert_eq!(terminator("(Parenthetical.) ", false), (".", true));
        assert_eq!(terminator("Item one\n\n", false), ("", true));
    }
}
