//! Text to speech with a Piper voice (the `.onnx` and `.onnx.json` pair every Piper voice ships as), without Python: the
//! port of what the helper's `tts.py` asked piper-tts 1.8 for. Text to phonemes per sentence (espeak-ng, `espeak`), the
//! phonemes to the voice's ids (start, then each known phoneme followed by the pad id, then end), the VITS model through
//! ONNX Runtime on the processor, each sentence's audio scaled to its loudest sample and written as 16-bit PCM.
//!
//! The helper's own layer on top (`tts.py`): a long reply is split into sentences first (after `.`, `!` or `?` and
//! whitespace), each synthesised alone and joined with 0.3 s of silence, because a voice trained on short clips loses
//! intelligibility on long input.

use crate::ortrt;
use crate::pyjson::{self, V};
use crate::voice::espeak;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use sarina_voice::py;
use std::borrow::Cow;
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

pub const DEFAULT_NOISE_SCALE: f64 = 0.667;
pub const DEFAULT_LENGTH_SCALE: f64 = 1.0;
pub const DEFAULT_NOISE_W_SCALE: f64 = 0.8;
pub const SENTENCE_PAUSE_SECONDS: f64 = 0.3;

/// The part of a voice's .onnx.json synthesis needs (piper's `PiperConfig.from_dict`).
#[derive(Debug, Clone)]
pub struct Config {
    pub num_speakers: i64,
    pub sample_rate: u32,
    pub espeak_voice: String,
    pub phoneme_id_map: Vec<(String, Vec<i64>)>,
    pub noise_scale: f64,
    pub length_scale: f64,
    pub noise_w_scale: f64,
    pub vowel_clusters: Option<HashSet<Vec<String>>>,
    pub default_speaker_id: i64,
}

fn num(v: Option<&V>) -> Option<f64> {
    match v? {
        V::Float(f) => Some(*f),
        V::Int(s) => s.parse().ok(),
        V::Bool(b) => Some(*b as i64 as f64),
        _ => None,
    }
}

fn int(v: Option<&V>) -> Option<i64> {
    match v? {
        V::Int(s) => s.parse().ok(),
        V::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

impl Config {
    pub fn from_json(text: &str) -> Result<Config, String> {
        let c = pyjson::loads(text).map_err(|e| e.0)?;
        let missing = |k: &str| format!("the voice config has no usable {k}");
        let phoneme_type = match c.get("phoneme_type") {
            None => "espeak".to_string(),
            Some(V::Str(s)) => s.s.clone(),
            Some(_) => return Err(missing("phoneme_type")),
        };
        if phoneme_type != "espeak" {
            return Err(format!("phoneme type {phoneme_type:?}: only espeak voices are supported"));
        }
        let audio = c.get("audio").ok_or_else(|| missing("audio"))?;
        let espeak_voice = match c.get("espeak").and_then(|e| e.get("voice")) {
            Some(V::Str(s)) => s.s.clone(),
            _ => return Err(missing("espeak.voice")),
        };
        let mut phoneme_id_map = Vec::new();
        match c.get("phoneme_id_map") {
            Some(V::Obj(m)) => {
                for (k, v) in m {
                    let V::Arr(ids) = v else { return Err(missing("phoneme_id_map")) };
                    let ids: Option<Vec<i64>> = ids.iter().map(|i| int(Some(i))).collect();
                    phoneme_id_map.push((k.s.clone(), ids.ok_or_else(|| missing("phoneme_id_map"))?));
                }
            }
            _ => return Err(missing("phoneme_id_map")),
        }
        let inference = c.get("inference");
        let vowel_clusters = match c.get("vowel_clusters") {
            Some(V::Arr(a)) if !a.is_empty() => Some(
                a.iter()
                    .map(|vc| match vc {
                        V::Arr(x) => x.iter().map(|p| p.py_str().s).collect(),
                        V::Str(s) => s.s.chars().map(|c| c.to_string()).collect(),
                        _ => Vec::new(),
                    })
                    .collect(),
            ),
            _ => None,
        };
        let rate = int(audio.get("sample_rate")).filter(|r| *r > 0 && *r <= u32::MAX as i64).ok_or_else(|| missing("audio.sample_rate"))?;
        Ok(Config {
            num_speakers: int(c.get("num_speakers")).ok_or_else(|| missing("num_speakers"))?,
            sample_rate: rate as u32,
            espeak_voice,
            phoneme_id_map,
            noise_scale: num(inference.and_then(|i| i.get("noise_scale"))).unwrap_or(DEFAULT_NOISE_SCALE),
            length_scale: num(inference.and_then(|i| i.get("length_scale"))).unwrap_or(DEFAULT_LENGTH_SCALE),
            noise_w_scale: num(inference.and_then(|i| i.get("noise_w"))).unwrap_or(DEFAULT_NOISE_W_SCALE),
            vowel_clusters,
            default_speaker_id: int(c.get("default_speaker_id")).unwrap_or(0),
        })
    }

    fn ids_of(&self, p: &str) -> Option<&[i64]> {
        self.phoneme_id_map.iter().find(|(k, _)| k == p).map(|(_, v)| v.as_slice())
    }

    /// piper's `phonemes_to_ids`: start, pad, then each phoneme and a pad, then end. A phoneme the voice does not know is
    /// left out with piper's warning on stderr.
    pub fn ids(&self, phonemes: &[String], warn: &mut dyn FnMut(&str)) -> Vec<i64> {
        let get = |p: &str| self.ids_of(p).map(|v| v.to_vec()).unwrap_or_default();
        let (bos, pad, eos) = (get("^"), get("_"), get("$"));
        let mut ids = bos;
        ids.extend(&pad);
        for p in phonemes {
            match self.ids_of(p) {
                Some(v) => {
                    ids.extend(v);
                    ids.extend(&pad);
                }
                None => warn(&format!("Missing phoneme from id map: {p}")),
            }
        }
        ids.extend(eos);
        ids
    }
}

/// `_merge_known_vowel_clusters`
fn merge_clusters(phones: Vec<String>, clusters: &HashSet<Vec<String>>) -> Vec<String> {
    let max_len = clusters.iter().map(|c| c.len()).max().unwrap_or(0);
    let mut out = Vec::new();
    let mut i = 0;
    while i < phones.len() {
        let mut matched = None;
        let mut n = max_len.min(phones.len() - i);
        while n > 1 {
            if clusters.contains(&phones[i..i + n].to_vec()) {
                matched = Some(n);
                break;
            }
            n -= 1;
        }
        match matched {
            Some(n) => {
                out.push(phones[i..i + n].concat());
                i += n;
            }
            None => {
                out.push(phones[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// `re.sub(r"\([^)]+\)", "", s)`: espeak's language-switch flags taken out
fn drop_flags(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('(') {
        match rest[i + 1..].find(')') {
            Some(j) if j > 0 => {
                out.push_str(&rest[..i]);
                rest = &rest[i + 1 + j + 1..];
            }
            _ => {
                out.push_str(&rest[..i + 1]);
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `EspeakPhonemizer.phonemize`: phonemes per sentence, punctuation kept, decomposed (NFD) into code points.
pub fn espeak_phonemes(voice: &str, text: &str, clusters: Option<&HashSet<Vec<String>>>) -> Result<Vec<Vec<String>>, String> {
    let mut all = Vec::new();
    let mut sentence: Vec<String> = Vec::new();
    let merge = |s: Vec<String>| match clusters {
        Some(c) if !c.is_empty() => merge_clusters(s, c),
        _ => s,
    };
    for clause in espeak::phonemize(voice, text)? {
        let mut p = drop_flags(&clause.phonemes);
        p.push_str(clause.terminator);
        if matches!(clause.terminator, "," | ":" | ";") {
            p.push(' ');
        }
        sentence.extend(p.nfd().map(|c| c.to_string()));
        if clause.end_of_sentence {
            all.push(merge(std::mem::take(&mut sentence)));
        }
    }
    if !sentence.is_empty() {
        all.push(merge(sentence));
    }
    Ok(all)
}

/// `re.split(r"(\[\[.*?\]\])", text)`: text and `[[ raw phonemes ]]` blocks (not across a newline), blocks kept.
fn split_blocks(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = text;
    loop {
        let found = rest.match_indices("[[").find_map(|(i, _)| {
            let after = &rest[i + 2..];
            let close = after.find("]]")?;
            (!after[..close].contains('\n')).then_some((i, i + 2 + close + 2))
        });
        match found {
            Some((a, b)) => {
                parts.push(rest[..a].to_string());
                parts.push(rest[a..b].to_string());
                rest = &rest[b..];
            }
            None => {
                parts.push(rest.to_string());
                return parts;
            }
        }
    }
}

pub struct Voice {
    pub config: Config,
    session: Session,
    has_sid: bool,
}

impl Voice {
    /// `PiperVoice.load(model, config_path)`: the config defaults to `<model>.json`.
    pub fn load(model: &str, config_path: Option<&str>) -> Result<Voice, String> {
        let cfg_path = config_path.map(|s| s.to_string()).unwrap_or_else(|| format!("{model}.json"));
        let text = std::fs::read(&cfg_path).map_err(|e| {
            format!("[Errno {}] {}: {}", e.raw_os_error().unwrap_or(0), crate::voice::os_error_text(&e), py::str_repr(&cfg_path))
        })?;
        let text = String::from_utf8(text).map_err(|_| format!("{cfg_path}: not UTF-8"))?;
        let config = Config::from_json(&text)?;
        let session = ortrt::cpu_session(model)?;
        let has_sid = session.inputs().iter().any(|i| i.name() == "sid");
        Ok(Voice { config, session, has_sid })
    }

    /// `PiperVoice.phonemize`
    pub fn phonemize(&self, text: &str) -> Result<Vec<Vec<String>>, String> {
        let parts = split_blocks(text);
        let mut phonemes: Vec<Vec<String>> = Vec::new();
        let mut prev_raw = false;
        for (i, part) in parts.iter().enumerate() {
            if part.starts_with("[[") {
                prev_raw = true;
                if phonemes.is_empty() {
                    phonemes.push(Vec::new());
                }
                let last = phonemes.last_mut().unwrap();
                if i > 0 && parts[i - 1].ends_with(' ') {
                    last.push(" ".into());
                }
                last.extend(py::strip(&part[2..part.len() - 2]).chars().map(|c| c.to_string()));
                if i + 1 < parts.len() && parts[i + 1].starts_with(' ') {
                    last.push(" ".into());
                }
                continue;
            }
            let mut tp = espeak_phonemes(&self.config.espeak_voice, part, self.config.vowel_clusters.as_ref())?;
            if prev_raw && !tp.is_empty() {
                let first = tp.remove(0);
                phonemes.last_mut().unwrap().extend(first);
            }
            phonemes.extend(tp);
            prev_raw = false;
        }
        if phonemes.last().is_some_and(|p| p.is_empty()) {
            phonemes.pop();
        }
        Ok(phonemes)
    }

    fn run(&mut self, ids: Vec<i64>) -> Result<Vec<f32>, String> {
        let c = &self.config;
        let n = ids.len() as i64;
        let scales = vec![c.noise_scale as f32, c.length_scale as f32, c.noise_w_scale as f32];
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![
            ("input".into(), Tensor::from_array((vec![1i64, n], ids)).map_err(|e| e.to_string())?.into_dyn().into()),
            ("input_lengths".into(), Tensor::from_array((vec![1i64], vec![n])).map_err(|e| e.to_string())?.into_dyn().into()),
            ("scales".into(), Tensor::from_array((vec![3i64], scales)).map_err(|e| e.to_string())?.into_dyn().into()),
        ];
        if c.num_speakers > 1 {
            if !self.has_sid {
                return Err("the voice has several speakers but no sid input".into());
            }
            inputs.push((
                "sid".into(),
                Tensor::from_array((vec![1i64], vec![c.default_speaker_id])).map_err(|e| e.to_string())?.into_dyn().into(),
            ));
        }
        let out = self.session.run(inputs).map_err(ortrt::python_error)?;
        let (_, data) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        Ok(data.to_vec())
    }

    /// `PiperVoice.synthesize` then `AudioChunk.audio_int16_array`, all sentences one after another (as `synthesize_wav`
    /// writes them). None: no sentence had a phoneme (the Python's wave module then failed: "# channels not specified").
    pub fn synthesize_pcm(&mut self, text: &str, warn: &mut dyn FnMut(&str)) -> Result<Option<Vec<i16>>, String> {
        let mut pcm: Option<Vec<i16>> = None;
        for phonemes in self.phonemize(text)? {
            if phonemes.is_empty() {
                continue;
            }
            let ids = self.config.ids(&phonemes, warn);
            let audio = self.run(ids)?;
            pcm.get_or_insert_with(Vec::new).extend(to_int16(audio));
        }
        Ok(pcm)
    }
}

/// normalize_audio (scaled to the loudest sample; silence below 1e-8 stays silence), clip, then int16 as NumPy makes it
/// (float32 times 32767, clipped, truncated toward zero).
pub fn to_int16(audio: Vec<f32>) -> Vec<i16> {
    let max = audio.iter().fold(0f32, |m, x| if x.abs() > m || x.is_nan() { x.abs() } else { m });
    let scaled: Vec<f32> = if max < 1e-8 { vec![0.0; audio.len()] } else { audio.iter().map(|x| x / max).collect() };
    scaled
        .into_iter()
        .map(|x| {
            let x = x.clamp(-1.0, 1.0) * 32767.0f32;
            x.clamp(-32767.0, 32767.0) as i16
        })
        .collect()
}

/// A 16-bit mono WAV file as Python's wave module writes it.
pub fn wav(pcm: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

/// `_split_sentences`: after `.`, `!` or `?`, a run of whitespace ends a sentence; parts stripped, empty ones dropped.
pub fn split_sentences(text: &str) -> Vec<String> {
    let t = py::strip(text);
    let chars: Vec<char> = t.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        if py::is_space(chars[i]) && i > 0 && matches!(chars[i - 1], '.' | '!' | '?') {
            while i < chars.len() && py::is_space(chars[i]) {
                i += 1;
            }
            parts.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    parts.push(cur);
    parts.into_iter().map(|p| py::strip(&p).to_string()).filter(|p| !p.is_empty()).collect()
}

/// The helper's `TextToSpeech.synthesize`: a complete WAV file for the text.
pub fn synthesize(voice: &mut Voice, text: &str, warn: &mut dyn FnMut(&str)) -> Result<Vec<u8>, String> {
    let rate = voice.config.sample_rate;
    let sentences = split_sentences(text);
    let one = |voice: &mut Voice, t: &str, warn: &mut dyn FnMut(&str)| -> Result<Vec<i16>, String> {
        voice.synthesize_pcm(t, warn)?.ok_or_else(|| "# channels not specified".to_string())
    };
    if sentences.len() <= 1 {
        return Ok(wav(&one(voice, text, warn)?, rate));
    }
    let silence = vec![0i16; (rate as f64 * SENTENCE_PAUSE_SECONDS) as usize];
    let mut all = Vec::new();
    let n = sentences.len();
    for (i, s) in sentences.iter().enumerate() {
        all.extend(one(voice, s, warn)?);
        if i < n - 1 {
            all.extend(&silence);
        }
    }
    Ok(wav(&all, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences() {
        assert_eq!(split_sentences("  Hi there.  How are you?Fine! ok "), vec!["Hi there.", "How are you?Fine!", "ok"]);
        assert_eq!(split_sentences("   "), Vec::<String>::new());
        assert_eq!(split_sentences("3.5 seconds. Next"), vec!["3.5 seconds.", "Next"]);
    }

    #[test]
    fn blocks_and_flags() {
        assert_eq!(split_blocks("say [[ hɛloʊ ]] now"), vec!["say ", "[[ hɛloʊ ]]", " now"]);
        assert_eq!(split_blocks("[[a\n]] x"), vec!["[[a\n]] x"]);
        assert_eq!(drop_flags("(en)hɛloʊ(fr)x()y"), "hɛloʊx()y");
    }

    #[test]
    fn ids_with_pads() {
        let cfg = Config::from_json(r#"{"num_speakers":1,"audio":{"sample_rate":22050},"espeak":{"voice":"en-us"},"phoneme_id_map":{"_":[0],"^":[1],"$":[2],"h":[20],"ə":[59]}}"#).unwrap();
        let mut warned = Vec::new();
        let ids = cfg.ids(&["h".into(), "ə".into(), "ˈ".into()], &mut |w| warned.push(w.to_string()));
        assert_eq!(ids, vec![1, 0, 20, 0, 59, 0, 2]);
        assert_eq!(warned, vec!["Missing phoneme from id map: ˈ"]);
    }

    #[test]
    fn pcm_as_numpy() {
        assert_eq!(to_int16(vec![0.0, 0.5, -1.0, 0.25]), vec![0, 16383, -32767, 8191]);
        assert_eq!(to_int16(vec![1e-9, -1e-9]), vec![0, 0]);
        let w = wav(&[1, -2, 3], 22050);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 50);
    }
}
