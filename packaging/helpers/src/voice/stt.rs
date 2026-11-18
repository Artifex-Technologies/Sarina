//! Speech to text with whisper.cpp (`whisper-cli`, Arch: whisper-cpp) and a ggml model, the same pair Labyrinth's Super+S
//! uses (`~/.local/share/labyrinth/stt/ggml-base.en.bin`). This replaces the Python's faster-whisper (CTranslate2), which
//! has no Rust equivalent and whose models are another format: a CTranslate2 folder is refused with a message saying so.
//! Like the Python, nothing is downloaded: the model is a file already on disk.
//!
//! `--stt-model-dir` names the model file, or a folder holding exactly one `ggml-*.bin`. `--stt-device cpu` (the default)
//! keeps whisper.cpp off the graphics card (`-ng`); `cuda` lets it use one. `--stt-compute-type` has no whisper.cpp
//! counterpart (a ggml model's quantisation is in the file) and is accepted and ignored.

use std::path::Path;
use std::process::{Command, Stdio};

/// The program (`SARINA_WHISPER` names another).
pub fn program() -> String {
    std::env::var("SARINA_WHISPER").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "whisper-cli".into())
}

pub struct SpeechToText {
    pub whisper: String,
    pub model: String,
    pub gpu: bool,
}

/// The ggml model `--stt-model-dir` names. Err is printed as the Python printed its VoiceUnavailable.
pub fn resolve_model(arg: &str) -> Result<String, String> {
    let p = Path::new(arg);
    if p.is_file() {
        return Ok(arg.to_string());
    }
    if !p.is_dir() {
        return Err(format!("no such STT model directory: {}", crate::embed::pathlib_str(arg)));
    }
    let mut ggml: Vec<String> = std::fs::read_dir(p)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("ggml-") && n.ends_with(".bin"))
        .collect();
    ggml.sort();
    match ggml.len() {
        1 => Ok(crate::embed::path_join(arg, &ggml[0])),
        0 if p.join("model.bin").is_file() => Err(format!(
            "{} is a CTranslate2 (faster-whisper) model; the Rust helper runs whisper.cpp and needs a ggml model (ggml-*.bin), such as the one Super+S uses",
            crate::embed::pathlib_str(arg)
        )),
        0 => Err(format!("no whisper.cpp model (ggml-*.bin) in {}", crate::embed::pathlib_str(arg))),
        _ => Err(format!("{} holds several whisper.cpp models ({}): name the file", crate::embed::pathlib_str(arg), ggml.join(", "))),
    }
}

/// whisper.cpp's notes for what is not speech ("[BLANK_AUDIO]", "(music)"): not words.
fn is_note(s: &str) -> bool {
    (s.starts_with('[') && s.ends_with(']')) || (s.starts_with('(') && s.ends_with(')'))
}

impl SpeechToText {
    /// The transcript of a 16 kHz WAV file: whisper's lines, each stripped, joined by spaces (as the Python joined
    /// faster-whisper's segments).
    pub fn transcribe(&self, wav: &Path) -> Result<String, String> {
        let mut cmd = Command::new(&self.whisper);
        cmd.args(["-m", &self.model, "-f"]).arg(wav).args(["-nt", "-np", "-l", "auto"]);
        if !self.gpu {
            cmd.arg("-ng");
        }
        let out = cmd.stdin(Stdio::null()).output().map_err(|e| crate::voice::os_error(&e, &self.whisper))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err.lines().rev().map(|l| l.trim()).find(|l| !l.is_empty()).unwrap_or("no output");
            return Err(format!("whisper-cli failed ({}): {last}", out.status));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let parts: Vec<&str> = text.lines().map(sarina_voice::py::strip).filter(|l| !l.is_empty() && !is_note(l)).collect();
        Ok(sarina_voice::py::strip(&parts.join(" ")).to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn notes() {
        assert!(super::is_note("[BLANK_AUDIO]"));
        assert!(super::is_note("(silence)"));
        assert!(!super::is_note("Hello there."));
    }
}
