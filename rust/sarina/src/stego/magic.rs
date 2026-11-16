//! Go: `stego/magic.go`: what a file really is by its first bytes, and the file signatures inside it.

/// A file type and the extensions that usually carry it (Go's `Kind`).
#[derive(Clone, Debug, PartialEq)]
pub struct Kind {
    pub name: &'static str,
    pub exts: &'static [&'static str],
}

const MAGICS: &[(usize, &[u8], Kind)] = &[
    (0, b"\x89PNG\r\n\x1a\n", Kind { name: "png", exts: &["png"] }),
    (0, b"\xff\xd8\xff", Kind { name: "jpeg", exts: &["jpg", "jpeg"] }),
    (0, b"GIF87a", Kind { name: "gif", exts: &["gif"] }),
    (0, b"GIF89a", Kind { name: "gif", exts: &["gif"] }),
    (0, b"BM", Kind { name: "bmp", exts: &["bmp"] }),
    (8, b"WEBP", Kind { name: "webp", exts: &["webp"] }),
    (8, b"WAVE", Kind { name: "wav", exts: &["wav"] }),
    (8, b"AVI ", Kind { name: "avi", exts: &["avi"] }),
    (0, b"fLaC", Kind { name: "flac", exts: &["flac"] }),
    (0, b"OggS", Kind { name: "ogg", exts: &["ogg", "oga", "ogv", "opus"] }),
    (0, b"ID3", Kind { name: "mp3", exts: &["mp3"] }),
    (4, b"ftyp", Kind { name: "mp4", exts: &["mp4", "m4a", "mov", "heic"] }),
    (0, b"II*\x00", Kind { name: "tiff", exts: &["tif", "tiff"] }),
    (0, b"MM\x00*", Kind { name: "tiff", exts: &["tif", "tiff"] }),
    (0, b"PK\x03\x04", Kind { name: "zip", exts: &["zip", "jar", "docx", "xlsx", "pptx", "odt", "apk", "epub"] }),
    (0, b"PK\x05\x06", Kind { name: "zip (empty)", exts: &["zip"] }),
    (0, b"%PDF-", Kind { name: "pdf", exts: &["pdf"] }),
    (0, b"\x1f\x8b", Kind { name: "gzip", exts: &["gz", "tgz"] }),
    (0, b"BZh", Kind { name: "bzip2", exts: &["bz2"] }),
    (0, b"\xfd7zXZ\x00", Kind { name: "xz", exts: &["xz"] }),
    (0, b"7z\xbc\xaf\x27\x1c", Kind { name: "7z", exts: &["7z"] }),
    (0, b"Rar!\x1a\x07", Kind { name: "rar", exts: &["rar"] }),
    (0, b"\x7fELF", Kind { name: "elf", exts: &["", "so", "o"] }),
    (0, b"MZ", Kind { name: "pe (Windows program)", exts: &["exe", "dll"] }),
    (0, b"SQLite format 3\x00", Kind { name: "sqlite", exts: &["db", "sqlite", "sqlite3"] }),
    (257, b"ustar", Kind { name: "tar", exts: &["tar"] }),
    (0, b"-----BEGIN PGP", Kind { name: "pgp armor", exts: &["asc", "gpg"] }),
    (0, b"-----BEGIN", Kind { name: "pem", exts: &["pem", "crt", "cer", "pub"] }),
    (0, b"\xef\xbb\xbf", Kind { name: "text (UTF-8 with BOM)", exts: &["txt"] }),
    (0, b"<?xml", Kind { name: "xml", exts: &["xml", "svg"] }),
    (0, b"{\\rtf", Kind { name: "rtf", exts: &["rtf"] }),
    (0, b"\xd0\xcf\x11\xe0", Kind { name: "ole (old Office)", exts: &["doc", "xls", "ppt"] }),
    (0, b"#!", Kind { name: "script", exts: &["sh", "py"] }),
];

/// Go's `Detect`.
pub fn detect(b: &[u8]) -> Kind {
    for (offset, magic, kind) in MAGICS {
        if b.len() >= offset + magic.len() && &b[*offset..offset + magic.len()] == *magic {
            if kind.name == "bmp" && (b.len() < 14 || u32::from_le_bytes([b[2], b[3], b[4], b[5]]) as usize != b.len()) {
                continue;
            }
            return kind.clone();
        }
    }
    if b.len() >= 2 && b[0] == 0xff && b[1] == 0xfb {
        return Kind { name: "mp3", exts: &["mp3"] };
    }
    if is_text(b) {
        return Kind { name: "text", exts: &["txt", "md", "csv", "json", "html"] };
    }
    Kind { name: "unknown", exts: &[] }
}

/// Go's `isText`: no NUL and few control bytes in the first 4 KiB.
pub fn is_text(b: &[u8]) -> bool {
    if b.is_empty() {
        return true;
    }
    let sample = &b[..b.len().min(4096)];
    let mut bad = 0;
    for &c in sample {
        if c == 0 {
            return false;
        }
        if c < 0x20 && !matches!(c, b'\n' | b'\r' | b'\t' | 0x0c | 0x1b) {
            bad += 1;
        }
    }
    bad * 20 < sample.len()
}

/// A file signature found inside a file (Go's `Embedded`); the kind is Go's bytes (a RIFF's type need not be text).
#[derive(Clone, Debug, PartialEq)]
pub struct Embedded {
    pub offset: usize,
    pub kind: Vec<u8>,
}

fn index(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Go's `FindEmbedded`: the strong (offset 0, 4 bytes or more) signatures and RIFF headers after the first byte, at most
/// `max` of each search, in offset order.
pub fn find_embedded(b: &[u8], max: usize) -> Vec<Embedded> {
    let mut out: Vec<Embedded> = Vec::new();
    for (offset, magic, kind) in MAGICS {
        if *offset != 0 || magic.len() < 4 {
            continue;
        }
        let mut from = 1;
        while out.len() < max && from <= b.len() {
            let Some(i) = index(&b[from..], magic) else { break };
            out.push(Embedded { offset: from + i, kind: kind.name.as_bytes().to_vec() });
            from += i + 1;
        }
    }
    let mut from = 1;
    while out.len() < max && from <= b.len() {
        let Some(i) = index(&b[from..], b"RIFF") else { break };
        let at = from + i;
        if at + 12 <= b.len() {
            let mut kind = b"riff ".to_vec();
            kind.extend_from_slice(crate::gotext::trim_space_bytes(&b[at + 8..at + 12]));
            out.push(Embedded { offset: at, kind });
        }
        from = at + 1;
    }
    for i in 1..out.len() {
        let mut j = i;
        while j > 0 && out[j].offset < out[j - 1].offset {
            out.swap(j, j - 1);
            j -= 1;
        }
    }
    out
}
