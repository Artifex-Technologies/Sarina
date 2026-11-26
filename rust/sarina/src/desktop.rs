//! Go: `desktop`: the application menu's categories. `sarina desktop-audit` repairs .desktop entries that carry no
//! registered Main Category and proposes entries for installed tools that ship none; everything is a proposal until
//! `--apply`. Signals, cheapest and most reliable first: the pacman group, description keyword rules, then the optional
//! semantic classifier (the ML helper's embeddings, with a content-hash cache). Proved by the CLI differential's
//! desktop cases (rust/parity), against a stand-in pacman and helper and made-up application folders.
//!
//! Go's `EmbeddingScorer` (`semantic.go`) is not ported: nothing but Go's own tests uses it.

use crate::goany::Any;
use crate::gotext::{to_lower, trim_space};
use std::collections::{BTreeMap, BTreeSet};

/// Go's `GeneratedMarker`: on every file this generates, so cleanup removes exactly those.
const GENERATED_MARKER: &str = "X-Sarina-Generated";
const DESKTOP_ENTRY_GROUP: &str = "[Desktop Entry]";
const SYSTEM_APPLICATION_DIRS: &[&str] = &["/usr/share/applications", "/usr/local/share/applications"];
/// a .desktop file read is bounded: real ones are a few KiB
const MAX_ENTRY_BYTES: u64 = 1 << 20;

const MAIN_CATEGORIES: &[&str] = &[
    "AudioVideo",
    "Audio",
    "Video",
    "Development",
    "Education",
    "Game",
    "Graphics",
    "Network",
    "Office",
    "Science",
    "Settings",
    "System",
    "Utility",
];

const ADDITIONAL_CATEGORIES: &[&str] = &[
    "Archiving",
    "ArtificialIntelligence",
    "Calculator",
    "Compression",
    "Database",
    "Debugger",
    "Documentation",
    "Electronics",
    "Emulator",
    "FileManager",
    "FileTools",
    "HamRadio",
    "IDE",
    "ImageProcessing",
    "Monitor",
    "Network",
    "Player",
    "Publishing",
    "RemoteAccess",
    "Security",
    "TerminalEmulator",
    "TextEditor",
    "Viewer",
    "WebBrowser",
    "WebDevelopment",
];

/// environment and toolkit tokens: harmless, but not a menu location
const ENVIRONMENT_TOKENS: &[&str] = &["KDE", "GNOME", "XFCE", "DDE", "GTK", "Qt", "Motif", "Java", "ConsoleOnly"];

/// Go's `BlackArchGroupCategories`: the 52 BlackArch pacman groups (with upstream's typo "reocn").
const BLACKARCH_GROUPS: &[(&str, &[&str])] = &[
    ("ai", &["Science", "ArtificialIntelligence"]),
    ("anti-forensic", &["System", "Security"]),
    ("automation", &["Utility", "Security"]),
    ("automobile", &["System", "Security"]),
    ("backdoor", &["Network", "Security"]),
    ("binary", &["Development", "Security"]),
    ("bluetooth", &["Network", "Security"]),
    ("code-audit", &["Development", "Security"]),
    ("config", &["Settings", "Security"]),
    ("cracker", &["System", "Security"]),
    ("crypto", &["Utility", "Security"]),
    ("database", &["Development", "Database"]),
    ("debugger", &["Development", "Debugger"]),
    ("decompiler", &["Development", "Security"]),
    ("defensive", &["System", "Security"]),
    ("disassembler", &["Development", "Security"]),
    ("dos", &["Network", "Security"]),
    ("drone", &["System", "Security"]),
    ("exploitation", &["System", "Security"]),
    ("fingerprint", &["Network", "Security"]),
    ("firmware", &["System", "Security"]),
    ("forensic", &["System", "Security"]),
    ("fuzzer", &["Development", "Security"]),
    ("gpu", &["System", "Security"]),
    ("hardware", &["System", "Security"]),
    ("honeypot", &["Network", "Security"]),
    ("ids", &["Network", "Security"]),
    ("keylogger", &["System", "Security"]),
    ("malware", &["System", "Security"]),
    ("misc", &["Utility", "Security"]),
    ("mobile", &["System", "Security"]),
    ("networking", &["Network", "Security"]),
    ("nfc", &["Network", "Security"]),
    ("packer", &["Development", "Security"]),
    ("proxy", &["Network", "Security"]),
    ("radio", &["Network", "Security"]),
    ("recon", &["Network", "Security"]),
    ("reocn", &["Network", "Security"]),
    ("reversing", &["Development", "Security"]),
    ("scanner", &["Network", "Security"]),
    ("sniffer", &["Network", "Security"]),
    ("social", &["Network", "Security"]),
    ("spoof", &["Network", "Security"]),
    ("stego", &["Utility", "Security"]),
    ("threat-model", &["System", "Security"]),
    ("tunnel", &["Network", "Security"]),
    ("unpacker", &["Development", "Security"]),
    ("voip", &["Network", "Security"]),
    ("webapp", &["Network", "Security"]),
    ("windows", &["System", "Security"]),
    ("wireless", &["Network", "Security"]),
    ("wordlist", &["Utility", "Security"]),
];

/// Go's `DescriptionRules`, most specific first; the first that matches wins.
const DESCRIPTION_RULES: &[(&[&str], &[&str])] = &[
    (&["terminal emulator"], &["System", "TerminalEmulator"]),
    (&["file manager"], &["System", "FileManager"]),
    (&["text editor"], &["Utility", "TextEditor"]),
    (&["web browser"], &["Network", "WebBrowser"]),
    (&["system monitor", "process monitor"], &["System", "Monitor"]),
    (&["calculator"], &["Utility", "Calculator"]),
    (&["archive", "compression", "compressor"], &["Utility", "Archiving"]),
    (&["emulator"], &["System", "Emulator"]),
    (&["wireless", "802.11", "wifi", "wi-fi", "wpa", "bluetooth"], &["Network", "Security"]),
    (
        &[
            "password crack",
            "password recovery",
            "password cracker",
            "logon",
            "login cracker",
            "cracker",
            "brute force",
            "bruteforce",
            "hash crack",
        ],
        &["System", "Security"],
    ),
    (&["sql injection", "web server scanner", "web application", "web vulnerability"], &["Network", "Security"]),
    (&["exploit", "penetration test", "pentest", "vulnerability", "payload"], &["System", "Security"]),
    (
        &[
            "packet",
            "sniffer",
            "sniffing",
            "network attack",
            "network discovery",
            "port scan",
            "network scanner",
            "spoof",
            "man-in-the-middle",
        ],
        &["Network", "Security"],
    ),
    (&["forensic", "malware", "rootkit", "antivirus", "steganograph"], &["System", "Security"]),
    (
        &[
            "disassem",
            "decompil",
            "reverse engineer",
            "binary analysis",
            "binary image",
            "binary file",
            "firmware",
            "embedded file",
            "debugger",
            "fuzzing",
            "fuzzer",
        ],
        &["Development", "Security"],
    ),
    (&["encrypt", "decrypt", "cryptograph", "cipher"], &["Utility", "Security"]),
    (&["image editor", "image processing", "photo", "graphics", "vector"], &["Graphics"]),
    (&["video player", "media player", "video"], &["AudioVideo", "Video"]),
    (&["audio", "music", "sound"], &["AudioVideo", "Audio"]),
    (&["spreadsheet", "word processor", "presentation"], &["Office"]),
    (&["compiler", "programming language", "sdk", "build system", "version control", "source code"], &["Development"]),
    (&["documentation", "manual page"], &["Utility", "Documentation"]),
    (&["network", "http", "dns", "tcp", "ftp", "ssh"], &["Network"]),
    (&["filesystem", "disk", "partition"], &["System", "FileTools"]),
];

/// Go's `CategoryPrototypes`: the sentences the helper compares a description with (their fingerprint must be the
/// helper's).
const CATEGORY_PROTOTYPES: &[(&[&str], &str)] = &[
    (&["Network", "Security"], "network scanning, port scanning, packet capture, sniffing, wireless and wifi auditing, spoofing, proxies and remote network attacks"),
    (&["System", "Security"], "password cracking, hash cracking, digital forensics, malware and rootkit analysis, privilege escalation and host exploitation"),
    (&["Development", "Security"], "disassembly, decompilation, debugging, fuzzing and reverse engineering of binaries and executables"),
    (&["Utility", "Security"], "encryption, decryption, cryptography, hashing, encoding and steganography"),
    (&["Development"], "software development, programming languages, compilers, build systems, version control and source code editing"),
    (&["Network"], "web browsing, downloading, file transfer, email, chat and general internet client software"),
    (&["Graphics"], "image editing, photo manipulation, drawing, vector graphics and viewing pictures"),
    (&["AudioVideo"], "playing, recording and editing audio, music and video"),
    (&["Office"], "word processing, spreadsheets, presentations, documents and note taking"),
    (&["Science"], "scientific computing, mathematics, statistics, data analysis and plotting"),
    (&["Settings"], "configuring the desktop, appearance, input devices and system preferences"),
    (&["System"], "system administration, managing processes, services, disks, filesystems and monitoring the machine"),
    (&["Utility"], "small general purpose desktop utilities, archiving, clipboard and text tools"),
];

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn has_main_category(categories: &[String]) -> bool {
    categories.iter().any(|t| MAIN_CATEGORIES.contains(&t.as_str()))
}

fn nonstandard_tokens(categories: &[String]) -> Vec<String> {
    categories
        .iter()
        .filter(|t| {
            !MAIN_CATEGORIES.contains(&t.as_str())
                && !ADDITIONAL_CATEGORIES.contains(&t.as_str())
                && !ENVIRONMENT_TOKENS.contains(&t.as_str())
        })
        .cloned()
        .collect()
}

/// Go's `CategoriesFromGroup`: a pacman group (blackarch-scanner) or the desktop token (X-BlackArch-Scanner).
fn categories_from_group(group: &str) -> Option<Vec<String>> {
    let key = to_lower(trim_space(group));
    for prefix in ["x-blackarch-", "blackarch-"] {
        if let Some(rest) = key.strip_prefix(prefix) {
            return BLACKARCH_GROUPS.iter().find(|g| g.0 == rest).map(|g| strs(g.1));
        }
    }
    None
}

fn categories_from_description(description: &str) -> Option<Vec<String>> {
    let text = to_lower(description);
    DESCRIPTION_RULES.iter().find(|(keys, _)| keys.iter().any(|k| text.contains(k))).map(|(_, c)| strs(c))
}

// ---- entries ----

/// One [Desktop Entry] group reduced to what menu placement needs (Go's `DesktopEntry`).
#[derive(Clone, Default)]
struct Entry {
    path: Vec<u8>,
    id: String,
    name: String,
    entry_type: String,
    exec: String,
    icon: String,
    categories: Vec<String>,
    no_display: bool,
    hidden: bool,
    terminal: bool,
    generated_by: String,
}

impl Entry {
    fn is_application(&self) -> bool {
        self.entry_type == "Application"
    }

    fn is_menu_visible(&self) -> bool {
        self.is_application() && !self.no_display && !self.hidden
    }
}

fn unescape(value: &str) -> String {
    if !value.contains('\\') {
        return value.to_string();
    }
    let rs: Vec<char> = value.chars().collect();
    let mut b = String::new();
    let mut i = 0;
    while i < rs.len() {
        if rs[i] != '\\' {
            b.push(rs[i]);
            i += 1;
            continue;
        }
        if i + 1 >= rs.len() {
            break;
        }
        i += 1;
        b.push(match rs[i] {
            's' => ' ',
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            c => c,
        });
        i += 1;
    }
    b
}

fn split_list(value: &str) -> Vec<String> {
    value.split(';').map(trim_space).filter(|p| !p.is_empty()).map(str::to_string).collect()
}

/// Go's `splitLines`: \n, \r\n and \r; no trailing empty element.
fn split_lines(text: &str) -> Vec<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.is_empty() {
        return Vec::new();
    }
    let text = text.strip_suffix('\n').unwrap_or(&text);
    text.split('\n').map(str::to_string).collect()
}

/// The file's base name without its extension (Go's `filepath.Ext`: from the last dot of the base).
fn entry_id(path: &[u8]) -> String {
    let base = crate::gopath::base(path);
    let stem = match base.iter().rposition(|&c| c == b'.') {
        Some(i) => &base[..i],
        None => &base[..],
    };
    crate::gotext::lossy(stem)
}

/// Go's `ParseDesktopEntry`: the [Desktop Entry] group, or None when the text has none; locale keys and later groups are
/// left out.
fn parse_entry(text: &str, path: &[u8]) -> Option<Entry> {
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let (mut in_entry, mut seen) = (false, false);
    for raw in split_lines(text) {
        let line = trim_space(&raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if in_entry {
                break;
            }
            in_entry = line == DESKTOP_ENTRY_GROUP;
            seen = seen || in_entry;
            continue;
        }
        let Some(i) = line.find('=') else { continue };
        if !in_entry {
            continue;
        }
        let (key, value) = (trim_space(&line[..i]), trim_space(&line[i + 1..]));
        if key.ends_with(']') {
            continue;
        }
        fields.entry(key.to_string()).or_insert_with(|| value.to_string());
    }
    if !seen {
        return None;
    }
    let get = |k: &str| fields.get(k).cloned().unwrap_or_default();
    let flag = |k: &str| crate::gotext::equal_fold(&get(k), "true");
    Some(Entry {
        path: path.to_vec(),
        id: entry_id(path),
        name: unescape(&get("Name")),
        entry_type: fields.get("Type").cloned().unwrap_or_else(|| "Application".into()),
        exec: unescape(&get("Exec")),
        icon: get("Icon"),
        categories: split_list(&get("Categories")),
        no_display: flag("NoDisplay"),
        hidden: flag("Hidden"),
        terminal: flag("Terminal"),
        generated_by: get(GENERATED_MARKER),
    })
}

/// Go's `UserApplicationDir`: where generated entries belong.
fn user_application_dir(getenv: &dyn Fn(&str) -> String) -> Vec<u8> {
    let d = getenv("XDG_DATA_HOME");
    if !d.is_empty() {
        return crate::gopath::join(&[d.as_bytes(), b"applications"]);
    }
    let home = getenv("HOME");
    let home = if home.is_empty() { ".".to_string() } else { home };
    crate::gopath::join(&[home.as_bytes(), b".local", b"share", b"applications"])
}

/// Go's `ApplicationDirs`: the XDG application folders that exist, most general first.
fn application_dirs(getenv: &dyn Fn(&str) -> String) -> Vec<Vec<u8>> {
    let mut candidates: Vec<Vec<u8>> = Vec::new();
    let dd = getenv("XDG_DATA_DIRS");
    if !dd.is_empty() {
        for p in dd.split(':').filter(|p| !p.is_empty()) {
            candidates.push(crate::gopath::join(&[p.as_bytes(), b"applications"]));
        }
    } else {
        candidates.extend(SYSTEM_APPLICATION_DIRS.iter().map(|d| d.as_bytes().to_vec()));
    }
    candidates.push(user_application_dir(getenv));
    let mut found: Vec<Vec<u8>> = Vec::new();
    for c in candidates {
        if found.contains(&c) {
            continue;
        }
        if std::fs::metadata(crate::gopath::os_path(&c)).is_ok_and(|m| m.is_dir()) {
            found.push(c);
        }
    }
    found
}

/// Go's `readText`: a regular file only, bounded (a named pipe planted under an entry's name would hold the scan for
/// ever), each byte that is not UTF-8 read as U+FFFD.
fn read_text(path: &[u8]) -> Option<String> {
    let raw = crate::session::readfile::read_regular(path, MAX_ENTRY_BYTES).ok()?;
    Some(crate::gotext::lossy(&raw))
}

fn has_meta(p: &[u8]) -> bool {
    p.iter().any(|c| matches!(c, b'*' | b'?' | b'[' | b'\\'))
}

/// Go's `filepath.Glob(filepath.Join(dir, "*.desktop"))`, sorted; nothing on a bad pattern.
fn glob_desktop(dir: &[u8]) -> Vec<Vec<u8>> {
    let pattern = crate::gopath::join(&[dir, b"*.desktop"]);
    let mut m = glob(&pattern, 0).unwrap_or_default();
    m.sort();
    m
}

fn glob(pattern: &[u8], depth: usize) -> Result<Vec<Vec<u8>>, ()> {
    use crate::gopath::path_match;
    if path_match(pattern, b"").is_err() || depth == 10_000 {
        return Err(());
    }
    if !has_meta(pattern) {
        return Ok(if std::fs::symlink_metadata(crate::gopath::os_path(pattern)).is_ok() { vec![pattern.to_vec()] } else { vec![] });
    }
    let i = pattern.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    let (dir, file) = (&pattern[..i], &pattern[i..]);
    // Go's cleanGlobPath
    let dir: Vec<u8> = if dir.is_empty() {
        b".".to_vec()
    } else if dir == b"/" {
        dir.to_vec()
    } else {
        dir[..dir.len() - 1].to_vec()
    };
    if !has_meta(&dir) {
        return glob_in(&dir, file, Vec::new());
    }
    if dir == pattern {
        return Err(());
    }
    let mut out = Vec::new();
    for d in glob(&dir, depth + 1)? {
        out = glob_in(&d, file, out)?;
    }
    Ok(out)
}

fn glob_in(dir: &[u8], pattern: &[u8], mut m: Vec<Vec<u8>>) -> Result<Vec<Vec<u8>>, ()> {
    if !std::fs::metadata(crate::gopath::os_path(dir)).is_ok_and(|x| x.is_dir()) {
        return Ok(m);
    }
    let Ok(rd) = std::fs::read_dir(crate::gopath::os_path(dir)) else { return Ok(m) };
    let mut names: Vec<Vec<u8>> = rd.filter_map(|e| e.ok()).map(|e| std::os::unix::ffi::OsStringExt::into_vec(e.file_name())).collect();
    names.sort();
    for n in names {
        match crate::gopath::path_match(pattern, &n) {
            Ok(true) => m.push(crate::gopath::join(&[dir, &n])),
            Ok(false) => {}
            Err(_) => return Err(()),
        }
    }
    Ok(m)
}

/// Go's `ScanDesktopEntries`: every .desktop file in the folders; later folders shadow earlier ones by entry id.
fn scan_entries(dirs: &[Vec<u8>]) -> Vec<Entry> {
    let mut by_id: BTreeMap<String, Entry> = BTreeMap::new();
    let mut order = Vec::new();
    for dir in dirs {
        for p in glob_desktop(dir) {
            let Some(text) = read_text(&p) else { continue };
            let Some(e) = parse_entry(&text, &p) else { continue };
            if !by_id.contains_key(&e.id) {
                order.push(e.id.clone());
            }
            by_id.insert(e.id.clone(), e);
        }
    }
    order.iter().map(|id| by_id[id].clone()).collect()
}

/// Go's `RewriteCategories`: the text with its [Desktop Entry] Categories key replaced and the marker added, nothing else
/// changed (a user-level file replaces the system one outright).
fn rewrite_categories(text: &str, categories: &[String], generated_by: &str) -> String {
    let replacement = format!("Categories={};", categories.join(";"));
    let marker = format!("{GENERATED_MARKER}={generated_by}");
    let mut out: Vec<String> = Vec::new();
    let (mut in_entry, mut written, mut inserted_marker) = (false, false, false);
    let close_group = |out: &mut Vec<String>, written: &mut bool, inserted_marker: &mut bool| {
        let mut trailing = Vec::new();
        while out.last().is_some_and(|l| trim_space(l).is_empty()) {
            trailing.push(out.pop().unwrap_or_default());
        }
        if !*written {
            out.push(replacement.clone());
            *written = true;
        }
        if !*inserted_marker {
            out.push(marker.clone());
            *inserted_marker = true;
        }
        out.extend(trailing.into_iter().rev());
    };
    for line in split_lines(text) {
        let stripped = trim_space(&line).to_string();
        if stripped.starts_with('[') {
            if in_entry {
                close_group(&mut out, &mut written, &mut inserted_marker);
            }
            in_entry = stripped == DESKTOP_ENTRY_GROUP;
            out.push(line);
            continue;
        }
        if in_entry {
            let key = trim_space(stripped.split('=').next().unwrap_or_default());
            if key == "Categories" {
                if !written {
                    out.push(replacement.clone());
                    written = true;
                }
                continue;
            }
            if key == GENERATED_MARKER {
                continue;
            }
        }
        out.push(line);
    }
    if in_entry {
        close_group(&mut out, &mut written, &mut inserted_marker);
    }
    out.join("\n") + "\n"
}

/// Go's `RenderDesktopEntry` (generated by "sarina").
fn render_entry(name: &str, exec: &str, comment: &str, icon: &str, categories: &[String], terminal: bool) -> String {
    let mut lines = vec![DESKTOP_ENTRY_GROUP.to_string(), "Type=Application".into(), format!("Name={name}")];
    if !comment.is_empty() {
        lines.push(format!("Comment={comment}"));
    }
    lines.push(format!("Exec={exec}"));
    if !icon.is_empty() {
        lines.push(format!("Icon={icon}"));
    }
    lines.push(format!("Terminal={terminal}"));
    lines.push(format!("Categories={};", categories.join(";")));
    lines.push(format!("{GENERATED_MARKER}=sarina"));
    lines.join("\n") + "\n"
}

// ---- packages ----

/// One installed package reduced to what categorisation uses (Go's `InstalledPackage`).
#[derive(Clone, Default)]
struct Package {
    name: String,
    description: String,
    groups: Vec<String>,
    binaries: Vec<String>,
    explicit: bool,
}

impl Package {
    /// Go's `PrimaryBinary`: an exact name match, else the shortest (ties alphabetical).
    fn primary_binary(&self) -> String {
        let Some(first) = self.binaries.first() else { return String::new() };
        let mut best = first;
        for b in &self.binaries {
            if *b == self.name {
                return b.clone();
            }
            if b.len() < best.len() || (b.len() == best.len() && b < best) {
                best = b;
            }
        }
        best.clone()
    }
}

const NON_APPLICATION_PREFIXES: &[&str] = &[
    "lib",
    "python-",
    "python2-",
    "python313-",
    "perl-",
    "ruby-",
    "php-",
    "nodejs-",
    "haskell-",
    "rust-",
    "lua-",
    "lua51-",
    "lua52-",
    "lua53-",
    "ocaml-",
    "texlive-",
    "ttf-",
    "otf-",
    "woff-",
    "woff2-",
    "font-",
    "fonts-",
    "noto-fonts",
    "adobe-source",
    "qt5-",
    "qt6-",
    "kf5-",
    "kf6-",
    "gtk2",
    "gtk3",
    "gtk4",
    "glibc",
    "gcc-",
    "llvm",
    "clang",
    "linux-api-headers",
    "xorg-font",
    "vulkan-",
    "mesa",
    "opencl-",
    "cuda",
];

const NON_APPLICATION_SUFFIXES: &[&str] = &[
    "-libs",
    "-lib",
    "-doc",
    "-docs",
    "-man",
    "-headers",
    "-dev",
    "-devel",
    "-dkms",
    "-keyring",
    "-mirrorlist",
    "-fonts",
    "-font",
    "-icon-theme",
    "-themes",
    "-theme",
    "-wordlist",
    "-wordlists",
    "-data",
    "-common",
    "-i18n",
    "-l10n",
    "-locale",
    "-runtime",
    "-sdk",
    "-src",
    "-static",
];

const NON_APPLICATION_NAMES: &[&str] = &[
    "base",
    "base-devel",
    "archiso",
    "arch-install-scripts",
    "filesystem",
    "pacman",
    "pacman-contrib",
    "pacman-mirrorlist",
    "systemd",
    "systemd-libs",
    "systemd-sysvcompat",
    "coreutils",
    "util-linux",
    "bash",
    "sh",
    "grep",
    "sed",
    "gawk",
    "findutils",
    "diffutils",
    "which",
    "less",
    "tar",
    "gzip",
    "bzip2",
    "xz",
    "zstd",
    "licenses",
    "iana-etc",
    "tzdata",
    "ca-certificates",
];

/// Go's `LooksLikeApplication`: not a library, font, documentation or metapackage by Arch's naming, and with binaries.
fn looks_like_application(p: &Package) -> bool {
    let name = to_lower(&p.name);
    if NON_APPLICATION_NAMES.contains(&name.as_str())
        || NON_APPLICATION_PREFIXES.iter().any(|x| name.starts_with(x))
        || NON_APPLICATION_SUFFIXES.iter().any(|x| name.ends_with(x))
    {
        return false;
    }
    !p.binaries.is_empty()
}

/// Go's `ParsePacmanInfo`: `pacman -Qi` (one package or many), wrapped lines and "None" handled.
fn parse_pacman_info(text: &str) -> Vec<Package> {
    let mut packages = Vec::new();
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut last_key = String::new();
    let flush = |fields: &mut BTreeMap<String, String>, last_key: &mut String, packages: &mut Vec<Package>| {
        let get = |k: &str| fields.get(k).map(|v| trim_space(v).to_string()).unwrap_or_default();
        let name = get("Name");
        if !name.is_empty() {
            let raw = get("Groups");
            let groups = if !raw.is_empty() && raw != "None" {
                crate::gotext::fields(&raw).into_iter().map(str::to_string).collect()
            } else {
                vec![]
            };
            packages.push(Package {
                name,
                description: get("Description"),
                groups,
                binaries: vec![],
                explicit: to_lower(&fields.get("Install Reason").cloned().unwrap_or_default()).contains("explicit"),
            });
        }
        fields.clear();
        last_key.clear();
    };
    for raw in split_lines(text) {
        if trim_space(&raw).is_empty() {
            flush(&mut fields, &mut last_key, &mut packages);
            continue;
        }
        if raw.chars().next().is_some_and(crate::gotext::is_space) {
            if !last_key.is_empty() {
                let joined = format!("{} {}", fields.get(&last_key).cloned().unwrap_or_default(), trim_space(&raw));
                fields.insert(last_key.clone(), trim_space(&joined).to_string());
            }
            continue;
        }
        let Some(i) = raw.find(':') else { continue };
        last_key = trim_space(&raw[..i]).to_string();
        fields.insert(last_key.clone(), trim_space(&raw[i + 1..]).to_string());
    }
    flush(&mut fields, &mut last_key, &mut packages);
    packages
}

/// Go's `ParsePacmanFileList`: `pacman -Ql` as {package: binaries}; only files directly in a binary folder count.
fn parse_pacman_file_list(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in split_lines(text) {
        let Some(i) = line.find(' ') else { continue };
        let (name, path) = (&line[..i], trim_space(&line[i + 1..]));
        for dir in ["/usr/bin/", "/usr/local/bin/"] {
            if let Some(rest) = path.strip_prefix(dir) {
                if !rest.is_empty() && !rest.contains('/') {
                    found.entry(name.to_string()).or_default().push(rest.to_string());
                }
                break;
            }
        }
    }
    for v in found.values_mut() {
        v.sort();
    }
    found
}

/// Go's `ExecRunner` (120 s, a non-zero exit is no answer): the program's standard output.
fn run_pacman(path: &str, arg: &str) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut out = Vec::new();
    let mut sink = |stream: &'static str, b: &[u8]| {
        if stream == "stdout" {
            out.extend_from_slice(b);
        }
    };
    let env = crate::tools::exec::parent_env();
    let ctx = crate::goctx::Ctx::background();
    let end = crate::tools::exec::run_group(
        path,
        &[arg.to_string()],
        &env,
        None,
        deadline,
        &ctx,
        crate::tools::exec::GroupKill::Inherit,
        &mut sink,
    )
    .ok()?;
    if !end.status.is_some_and(|s| s.success()) {
        return None;
    }
    Some(crate::gotext::lossy(&out))
}

/// Go's `QueryInstalledPackages("pacman", nil)`: every installed package with its description, groups and binaries, from
/// two pacman calls; nothing when pacman is missing or fails.
fn query_installed_packages() -> Vec<Package> {
    let Ok(path) = crate::tools::exec::look_path_go("pacman") else { return Vec::new() };
    let Some(info) = run_pacman(&path, "-Qi") else { return Vec::new() };
    let mut packages = parse_pacman_info(&info);
    if packages.is_empty() {
        return Vec::new();
    }
    let files = run_pacman(&path, "-Ql").map(|ql| parse_pacman_file_list(&ql)).unwrap_or_default();
    for p in packages.iter_mut() {
        p.binaries = files.get(&p.name).cloned().unwrap_or_default();
    }
    packages
}

// ---- the semantic classifier ----

fn prototypes_fingerprint() -> String {
    use sha2::{Digest, Sha256};
    let parts: Vec<String> = CATEGORY_PROTOTYPES.iter().map(|(c, s)| format!("{}|{s}", c.join(";"))).collect();
    Sha256::digest(parts.join("\n").as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Go's `SemanticClassifier` over the helper (`HelperScorer`): the nearest prototype and its score, cached by content;
/// the threshold applied fresh on every call.
struct Semantic {
    srv: crate::helpers::Server,
    threshold: f64,
    cache: Option<crate::analysiscache::Cache>,
}

impl Semantic {
    fn classify(&self, text: &str) -> (Option<Vec<String>>, f64) {
        if trim_space(text).is_empty() {
            return (None, 0.0);
        }
        let mut cached = None;
        if let Some(c) = &self.cache {
            if let Some(Any::Object(m)) = c.get(text.as_bytes()).as_ref() {
                if let Some(Any::Number(score)) = m.get("score") {
                    // a null label is no match
                    let label = match m.get("label") {
                        Some(Any::String(s)) => s.clone(),
                        _ => String::new(),
                    };
                    cached = Some((label, *score));
                }
            }
        }
        let (label, score) = match cached {
            Some(x) => x,
            None => {
                let Ok((label, score)) = self.srv.classify(text) else { return (None, 0.0) };
                if let Some(c) = &self.cache {
                    let v: BTreeMap<String, Any> =
                        [("label".to_string(), Any::String(label.clone())), ("score".to_string(), Any::Number(score))]
                            .into_iter()
                            .collect();
                    let _ = c.set(text.as_bytes(), Any::Object(v));
                }
                (label, score)
            }
        };
        let cats = CATEGORY_PROTOTYPES.iter().find(|(c, _)| c.join(";") == label).map(|(c, _)| strs(c));
        match cats {
            Some(c) if !label.is_empty() && score >= self.threshold => (Some(c), score),
            _ => (None, score),
        }
    }
}

// ---- the audit ----

/// One proposed menu change (Go's `CategoryProposal`).
#[derive(Clone)]
struct Proposal {
    action: &'static str,
    target: String,
    name: String,
    categories: Vec<String>,
    source: &'static str,
    confidence: f64,
    exec: String,
    comment: String,
    terminal: bool,
    icon: String,
    source_path: Vec<u8>,
}

impl Proposal {
    fn as_line(&self) -> String {
        format!("{:<7} {:<28} -> {:<28} [{}, {:.2}]", self.action, self.target, self.categories.join(";"), self.source, self.confidence)
    }

    /// Go's `Render`: a repair rewrites one line of a copy of the original; a creation (or a repair whose original
    /// cannot be read) is a fresh entry.
    fn render(&self) -> String {
        if self.action == "repair" && !self.source_path.is_empty() {
            if let Some(original) = read_text(&self.source_path) {
                if !original.is_empty() {
                    return rewrite_categories(&original, &self.categories, "sarina");
                }
            }
        }
        render_entry(&self.name, &self.exec, &self.comment, &self.icon, &self.categories, self.terminal)
    }
}

struct Audit {
    total: usize,
    visible: usize,
    hidden: usize,
    well_placed: usize,
    stranded: Vec<String>,
    uncovered: Vec<String>,
    proposals: Vec<Proposal>,
    undetermined: Vec<String>,
    unavailable: String,
}

impl Audit {
    fn as_text(&self) -> String {
        let mut lines = vec![
            "# Desktop menu audit".to_string(),
            String::new(),
            format!("desktop entries found:   {}", self.total),
            format!("  menu-visible:          {}", self.visible),
            format!("  hidden (NoDisplay):    {}  (left alone)", self.hidden),
            format!("  correctly placed:      {}", self.well_placed),
            format!("  stranded (no menu):    {}", self.stranded.len()),
            format!("packages with no entry:  {}", self.uncovered.len()),
            String::new(),
        ];
        let repairs: Vec<&Proposal> = self.proposals.iter().filter(|p| p.action == "repair").collect();
        let creations: Vec<&Proposal> = self.proposals.iter().filter(|p| p.action == "create").collect();
        if !repairs.is_empty() {
            lines.push("## Repair -- entry exists but has no menu location".into());
            lines.extend(repairs.iter().map(|p| format!("- {}", p.as_line())));
            lines.push(String::new());
        }
        if !creations.is_empty() {
            lines.push("## Create -- installed tool with no desktop entry".into());
            lines.extend(creations.iter().map(|p| format!("- {}", p.as_line())));
            lines.push(String::new());
        }
        if !self.undetermined.is_empty() {
            lines.push("## Undetermined -- no category signal, left alone".into());
            lines.push(self.undetermined.join(", "));
            lines.push(String::new());
        }
        if self.proposals.is_empty() {
            lines.push("No changes proposed.".into());
            lines.push(String::new());
        }
        if !self.unavailable.is_empty() {
            lines.push(format!("Semantic inference unavailable: {}", self.unavailable));
        }
        lines.join("\n").trim_end_matches([' ', '\t', '\r', '\n']).to_string()
    }
}

fn last_segment(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

const FIELD_CODES: &[&str] = &["%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v", "%m"];

/// Go's `BinaryFromExec`: the bare command name an Exec line runs.
fn binary_from_exec(exec: &str) -> String {
    let tokens: Vec<&str> = crate::gotext::fields(exec).into_iter().filter(|t| !FIELD_CODES.contains(t) && !t.starts_with('%')).collect();
    if tokens.is_empty() {
        return String::new();
    }
    let mut i = 0;
    if last_segment(tokens[0]) == "env" {
        i += 1;
        while i < tokens.len() && tokens[i].contains('=') {
            i += 1;
        }
    }
    if i >= tokens.len() {
        return String::new();
    }
    last_segment(tokens[i]).to_string()
}

fn deterministic(groups: &[String], tokens: &[String], description: &str) -> Option<(Vec<String>, &'static str)> {
    for g in groups.iter().chain(tokens) {
        if let Some(m) = categories_from_group(g) {
            return Some((m, "group"));
        }
    }
    categories_from_description(description).map(|m| (m, "description"))
}

struct Options {
    classifier: Option<Semantic>,
    include_dependencies: bool,
    include_libraries: bool,
    unavailable: String,
}

/// Go's `AuditMenu`.
fn audit_menu(entries: &[Entry], packages: &[Package], opt: &Options) -> Audit {
    let visible: Vec<&Entry> = entries.iter().filter(|e| e.is_menu_visible()).collect();
    let hidden = entries.iter().filter(|e| !e.is_menu_visible() && e.is_application()).count();
    let mut reason = String::new();
    if opt.classifier.is_none() {
        reason = opt.unavailable.clone();
        if reason.is_empty() {
            reason = "no model directory given; deterministic signals only".into();
        }
    }
    let mut descriptions: BTreeMap<&str, &str> = BTreeMap::new();
    for p in packages {
        descriptions.insert(&p.name, &p.description);
    }
    let classify = |text: &str| -> (Option<Vec<String>>, f64, &'static str) {
        match &opt.classifier {
            Some(c) => {
                let (cs, score) = c.classify(text);
                (cs, score, "semantic")
            }
            None => (None, 1.0, ""),
        }
    };
    let (mut proposals, mut stranded, mut undetermined) = (Vec::new(), Vec::new(), Vec::new());
    for e in &visible {
        if has_main_category(&e.categories) {
            continue;
        }
        stranded.push(e.id.clone());
        let text = trim_space(&format!("{} {}", e.name, descriptions.get(e.id.as_str()).copied().unwrap_or(""))).to_string();
        let (cs, source, conf) = match deterministic(&[], &nonstandard_tokens(&e.categories), &text) {
            Some((cs, source)) => (Some(cs), source, 1.0),
            None => {
                let (cs, score, source) = classify(&text);
                (cs, source, score)
            }
        };
        let Some(cs) = cs else {
            undetermined.push(e.id.clone());
            continue;
        };
        proposals.push(Proposal {
            action: "repair",
            target: e.id.clone(),
            name: if e.name.is_empty() { e.id.clone() } else { e.name.clone() },
            categories: cs,
            source,
            confidence: conf,
            exec: e.exec.clone(),
            comment: String::new(),
            terminal: e.terminal,
            icon: e.icon.clone(),
            source_path: e.path.clone(),
        });
    }
    let covered: BTreeSet<String> = visible.iter().map(|e| binary_from_exec(&e.exec)).filter(|b| !b.is_empty()).collect();
    let mut uncovered = Vec::new();
    for p in packages {
        if !p.explicit && !opt.include_dependencies {
            continue;
        }
        if !opt.include_libraries && !looks_like_application(p) {
            continue;
        }
        let binary = p.primary_binary();
        if binary.is_empty() || covered.contains(&binary) {
            continue;
        }
        uncovered.push(p.name.clone());
        let (cs, source, conf) = match deterministic(&p.groups, &[], &p.description) {
            Some((cs, source)) => (Some(cs), source, 1.0),
            None => {
                let (cs, score, source) = classify(&p.description);
                (cs, source, score)
            }
        };
        let Some(cs) = cs else {
            undetermined.push(p.name.clone());
            continue;
        };
        proposals.push(Proposal {
            action: "create",
            target: p.name.clone(),
            name: p.name.clone(),
            categories: cs,
            source,
            confidence: conf,
            exec: binary,
            comment: p.description.clone(),
            terminal: true,
            icon: String::new(),
            source_path: Vec::new(),
        });
    }
    Audit {
        total: entries.len(),
        visible: visible.len(),
        hidden,
        well_placed: visible.len() - stranded.len(),
        stranded,
        uncovered,
        proposals,
        undetermined,
        unavailable: reason,
    }
}

/// Go's `OnlyCategories`: the proposals with one of the comma-separated categories (all when the list is empty).
fn only_categories(proposals: Vec<Proposal>, list: &str) -> Vec<Proposal> {
    let want: BTreeSet<&str> = list.split(',').map(trim_space).filter(|c| !c.is_empty()).collect();
    if want.is_empty() {
        return proposals;
    }
    proposals.into_iter().filter(|p| p.categories.iter().any(|c| want.contains(c.as_str()))).collect()
}

/// Go's `writeEntry`: only as a regular file of its own (a link at its name is not followed, a named pipe is not waited
/// on).
fn write_entry(path: &[u8], body: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(crate::gopath::os_path(path))
        .map_err(|e| crate::goerr::path("open", path, &e))?;
    if !f.metadata().is_ok_and(|m| m.file_type().is_file()) {
        return Err(format!("{} is not a regular file", crate::gotext::lossy(path)));
    }
    f.write_all(body.as_bytes()).map_err(|e| crate::goerr::path("write", path, &e))
}

/// Go's `ApplyProposals`: the entries written into the folder (proposals below the confidence skipped).
fn apply_proposals(proposals: &[Proposal], target: &[u8], min_confidence: f64) -> (Vec<Vec<u8>>, Option<String>) {
    if let Err(e) = crate::goos::mkdir_all(target) {
        return (Vec::new(), Some(e));
    }
    let mut written = Vec::new();
    for p in proposals {
        if p.confidence < min_confidence {
            continue;
        }
        let stem = if p.action == "repair" { p.target.clone() } else { format!("sarina-{}", p.target) };
        let path = crate::gopath::join(&[target, format!("{stem}.desktop").as_bytes()]);
        if let Err(e) = write_entry(&path, &p.render()) {
            return (written, Some(e));
        }
        written.push(path);
    }
    (written, None)
}

/// Go's `RemoveGenerated`: only entries carrying the marker (each file read, its name not trusted).
fn remove_generated(target: &[u8]) -> Vec<Vec<u8>> {
    if !std::fs::metadata(crate::gopath::os_path(target)).is_ok_and(|m| m.is_dir()) {
        return Vec::new();
    }
    let mut removed = Vec::new();
    for p in glob_desktop(target) {
        let Some(text) = read_text(&p) else { continue };
        if parse_entry(&text, &p).is_some_and(|e| !e.generated_by.is_empty()) && std::fs::remove_file(crate::gopath::os_path(&p)).is_ok() {
            removed.push(p);
        }
    }
    removed
}

/// Go's `runAudit` with the default sources: `sarina desktop-audit`.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("desktop-audit");
    fs.string("model-dir", "", "directory of an embedding model for semantic classification");
    fs.float("similarity-threshold", 0.35, "minimum similarity for a semantic match");
    fs.bool("include-dependencies", false, "also consider packages installed as dependencies");
    fs.bool("include-libraries", false, "also consider library-shaped packages");
    fs.bool("apply", false, "write the proposed entries");
    fs.float("min-confidence", 0.0, "with --apply, skip proposals below this confidence");
    fs.string("only-category", "", "with --apply, write only proposals with one of these categories (comma-separated, e.g. Security)");
    fs.string("target-dir", "", "where entries are written (default: the user application directory)");
    fs.bool("remove-generated", false, "delete previously generated entries and exit");
    fs.bool("no-cache", false, "do not cache semantic classification results");
    let r = fs.parse(args, &mut env.stderr);
    env.flush();
    match r {
        Ok(_) => {}
        Err(e) if e == crate::goflag::HELP => return 0,
        Err(_) => return 2,
    }
    let getenv = env.getenv_owned();
    let mut target = fs.str("target-dir").into_bytes();
    if target.is_empty() {
        target = user_application_dir(&*getenv);
    }
    if fs.is_true("remove-generated") {
        let removed = remove_generated(&target);
        for p in &removed {
            env.println(&format!("removed {}", crate::gotext::lossy(p)));
        }
        env.println(&format!("{} generated entries removed.", removed.len()));
        return 0;
    }
    let mut opt = Options {
        classifier: None,
        include_dependencies: fs.is_true("include-dependencies"),
        include_libraries: fs.is_true("include-libraries"),
        unavailable: String::new(),
    };
    let model_dir = fs.str("model-dir");
    if !model_dir.is_empty() {
        let started = crate::helpers::Server::start(&model_dir, &*getenv)
            .map_err(|e| format!("semantic classification needs the ML helper ({}): {e}", crate::helpers::INSTALL_HINT));
        let started = started.and_then(|srv| {
            if srv.prototypes != prototypes_fingerprint() {
                let _ = srv.close();
                return Err("the ML helper's category prototypes differ from this build's; update sarina-helpers".into());
            }
            Ok(srv)
        });
        match started {
            Err(e) => {
                env.println(&format!("semantic inference unavailable: {e}"));
                opt.unavailable = e;
            }
            Ok(srv) => {
                let cache = (!fs.is_true("no-cache")).then(|| {
                    // Go's cachePath with the user's own home: analysiscache.DefaultDir
                    let base = crate::analysiscache::default_dir(&*getenv);
                    let path = crate::gopath::join(&[base.as_bytes(), b"desktop_classify.json"]);
                    crate::analysiscache::Cache::new(&path, &prototypes_fingerprint())
                });
                opt.classifier = Some(Semantic { srv, threshold: fs.f64("similarity-threshold"), cache });
            }
        }
    }
    let audit = audit_menu(&scan_entries(&application_dirs(&*getenv)), &query_installed_packages(), &opt);
    env.println(&audit.as_text());
    let code = if !fs.is_true("apply") {
        if !audit.proposals.is_empty() {
            env.println("");
            env.println("Nothing was written. Re-run with --apply to write these entries.");
        }
        0
    } else {
        let (written, err) =
            apply_proposals(&only_categories(audit.proposals, &fs.str("only-category")), &target, fs.f64("min-confidence"));
        for p in &written {
            env.println(&format!("wrote {}", crate::gotext::lossy(p)));
        }
        if let Some(e) = err {
            env.eprintln(&format!("desktop-audit: {e}"));
        }
        env.println(&format!("{} entries written. Undo with --remove-generated.", written.len()));
        0
    };
    // Go's deferred Close: the helper ends after the report
    if let Some(c) = opt.classifier {
        let _ = c.srv.close();
    }
    code
}
