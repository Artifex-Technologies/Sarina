//! desktop-audit's semantic classifier (Python's `classify.py`): the nearest category prototype to a description. Only the
//! threshold-independent half, (label, score); Sarina's core applies its threshold and its content-hash cache.
//!
//! The prototype set is the core's too (`rust/sarina/src/desktop.rs`, Go's `desktop/semantic.go`): the core refuses a
//! helper whose fingerprint differs, so cached scores never silently change meaning.

use crate::embed::{rank, EmbeddingModel};
use crate::pyjson::PStr;
use sha2::{Digest, Sha256};

/// One descriptive sentence per category bucket.
pub const CATEGORY_PROTOTYPES: &[(&[&str], &str)] = &[
    (
        &["Network", "Security"],
        "network scanning, port scanning, packet capture, sniffing, wireless and wifi auditing, spoofing, proxies and remote network attacks",
    ),
    (
        &["System", "Security"],
        "password cracking, hash cracking, digital forensics, malware and rootkit analysis, privilege escalation and host exploitation",
    ),
    (
        &["Development", "Security"],
        "disassembly, decompilation, debugging, fuzzing and reverse engineering of binaries and executables",
    ),
    (&["Utility", "Security"], "encryption, decryption, cryptography, hashing, encoding and steganography"),
    (
        &["Development"],
        "software development, programming languages, compilers, build systems, version control and source code editing",
    ),
    (&["Network"], "web browsing, downloading, file transfer, email, chat and general internet client software"),
    (&["Graphics"], "image editing, photo manipulation, drawing, vector graphics and viewing pictures"),
    (&["AudioVideo"], "playing, recording and editing audio, music and video"),
    (&["Office"], "word processing, spreadsheets, presentations, documents and note taking"),
    (&["Science"], "scientific computing, mathematics, statistics, data analysis and plotting"),
    (&["Settings"], "configuring the desktop, appearance, input devices and system preferences"),
    (&["System"], "system administration, managing processes, services, disks, filesystems and monitoring the machine"),
    (&["Utility"], "small general purpose desktop utilities, archiving, clipboard and text tools"),
];

/// sha256 of `"<cat1>;<cat2>|<sentence>"` lines joined by "\n".
pub fn prototypes_fingerprint() -> String {
    let serialized = CATEGORY_PROTOTYPES.iter().map(|(c, s)| format!("{}|{}", c.join(";"), s)).collect::<Vec<_>>().join("\n");
    Sha256::digest(serialized.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The prototypes embedded once (one batch, as the Python did).
pub struct Classifier {
    prototypes: Vec<(String, Vec<f64>)>,
}

impl Classifier {
    pub fn new(model: &mut EmbeddingModel) -> Result<Classifier, String> {
        let sentences: Vec<PStr> = CATEGORY_PROTOTYPES.iter().map(|(_, s)| PStr::new(*s)).collect();
        let vectors = model.embed(&sentences)?;
        Ok(Classifier { prototypes: CATEGORY_PROTOTYPES.iter().map(|(c, _)| c.join(";")).zip(vectors).collect() })
    }

    /// (label, score); None and 0.0 for a blank text (Python's `str.strip()`).
    pub fn classify(&self, model: &mut EmbeddingModel, text: &PStr) -> Result<(Option<String>, f64), String> {
        if sarina_voice::py::strip(&text.s).is_empty() && !text.lone {
            return Ok((None, 0.0));
        }
        let v = model.embed_one(text)?;
        Ok(rank(&v, &self.prototypes, Some(1)).into_iter().next().map(|(l, s)| (Some(l), s)).unwrap_or((None, 0.0)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fingerprint_is_the_pythons() {
        // python3 -c 'from sarina_helpers.classify import prototypes_fingerprint as f; print(f())'
        assert_eq!(super::prototypes_fingerprint(), "13e51a2830ecca5ed6192261cf131caa9a9bb7225a4d11306fa7157419cd4faf");
    }
}
