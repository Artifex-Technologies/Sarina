//! Go: `stego/stego.go`: the reports the tools and `sarina stego` give (`Inspect`, with the helper programs it runs,
//! `ExtractReport`, `DiffReport`, `Clean`) and the command itself.

use super::{clip_text, lsb, magic, meta, watermark, wav};
use crate::cryptotools::fmt_f;
use crate::goctx::Ctx;
use crate::gopath;
use crate::gotext::{self, lossy, quote, quote_bytes};
use std::time::{Duration, Instant};

/// Go's `MaxFile`: the most of a file the tool reads.
pub const MAX_FILE: u64 = 64 << 20;

/// Go's `Helpers`: the programs Inspect names, of which it runs exiftool and binwalk.
pub const HELPERS: [&str; 7] = ["exiftool", "binwalk", "zsteg", "steghide", "stegseek", "foremost", "outguess"];

/// Go's `filepath.Ext`.
pub fn ext(path: &str) -> &str {
    match path.rfind(['.', '/']) {
        Some(i) if path.as_bytes()[i] == b'.' => &path[i..],
        _ => "",
    }
}

fn base(path: &str) -> String {
    lossy(&gopath::base(path.as_bytes()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|c| format!("{c:02x}")).collect()
}

fn msb_words(msb: bool) -> &'static str {
    if msb {
        "most significant"
    } else {
        "least significant"
    }
}

/// Go's `Inspect`: everything a read-only look at the file finds, then the helper programs' report.
pub fn inspect(ctx: &Ctx, b: &[u8], name: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut put = |s: &[u8]| out.extend_from_slice(s);
    let kind = magic::detect(b);
    let ext = gotext::to_lower(ext(name));
    let ext = ext.strip_prefix('.').unwrap_or(&ext);
    put(format!("File {}: {} bytes, really a {} file", base(name), b.len(), kind.name).as_bytes());
    if !ext.is_empty() && kind.name != "unknown" && kind.name != "text" && !kind.exts.contains(&ext) {
        put(format!(" but named .{ext}: THE EXTENSION LIES about the type").as_bytes());
    }
    put(b".\n");
    let c = match kind.name {
        "png" => meta::parse_png(b),
        "jpeg" => meta::parse_jpeg(b),
        "gif" => meta::parse_gif(b),
        "wav" | "webp" | "avi" => meta::parse_riff(b),
        _ => meta::Container { end: -1, ..Default::default() },
    };
    if !c.parts.is_empty() {
        put(b"\nStructure: ");
        put(&c.parts.join(&b"; "[..]));
        put(b".\n");
    }
    for p in &c.problems {
        put(b"Problem: ");
        put(p);
        put(b".\n");
    }
    if !c.text.is_empty() {
        put(b"Metadata text:\n");
        for t in &c.text {
            put(b"  ");
            put(t);
            put(b"\n");
        }
    } else if matches!(kind.name, "png" | "jpeg" | "gif" | "wav") {
        put(b"Metadata text: none (no comments, EXIF text or text chunks).\n");
    }
    for u in &c.unusual {
        put(b"Unusual part: ");
        put(u);
        put(b".\n");
    }
    let t = c.trailing(b);
    if !t.is_empty() {
        let k = magic::detect(t);
        put(format!(
            "DATA AFTER THE END OF THE {} ({} bytes at offset {}): looks like {}",
            kind.name.to_uppercase(),
            t.len(),
            c.end,
            k.name
        )
        .as_bytes());
        if k.name == "text" {
            put(format!(": {}", quote_bytes(&clip_text(t, 300))).as_bytes());
        } else {
            put(format!("; first bytes {}", hex(&t[..t.len().min(16)])).as_bytes());
        }
        put(b". Viewers ignore this, so it is a classic hiding place: carve it out (dd or tail -c) and look at it as its own file.\n");
    } else if c.end > 0 {
        put(b"Nothing after the end of the format.\n");
    }
    let emb = magic::find_embedded(b, 12);
    if !emb.is_empty() {
        put(b"File signatures inside the file:");
        for e in &emb {
            put(b" ");
            put(&e.kind);
            put(format!(" at offset {};", e.offset).as_bytes());
        }
        put(b" (a signature inside compressed data can be a coincidence; one after the format's end, or a zip/pdf/elf, is a real embedded file).\n");
    } else {
        put(b"No other file signatures inside it.\n");
    }
    let (worth, total) = meta::strings(b, 8, 15);
    put(format!("Strings: {total} printable runs of 8+ characters").as_bytes());
    if !worth.is_empty() {
        put(b"; worth reading:\n");
        for s in &worth {
            put(b"  ");
            put(s);
            put(b"\n");
        }
    } else {
        put(b"; none look like URLs, flags, keys or long base64.\n");
    }
    match kind.name {
        "png" | "jpeg" | "gif" | "bmp" => match lsb::decode_image(b) {
            Ok(p) => {
                let a = lsb::analyse(&p);
                let paletted = if p.indices.is_some() { ", paletted" } else { "" };
                put(format!("\nImage {}x{} ({}{paletted}). LSB measurement: {}\n", p.w, p.h, p.format, lsb::describe(&a)).as_bytes());
                if kind.name == "jpeg" {
                    put(b"This is a JPEG: its pixels are decoded from DCT coefficients, so pixel-LSB hiding does not survive in it; JPEG steganography (steghide, outguess, F5) lives in the coefficients and needs those tools and the password.\n");
                }
                let found = lsb::search_lsb(&p, 4096);
                if found.is_empty() {
                    put(b"LSB extraction (orders rgb, r, g, b, rgba, bgr; both bit orders; 1 bit): nothing that starts like text or a known file.\n");
                    put(b"Verdict: NO readable LSB payload found. ");
                    put(if a.verdict.contains("equalised in") {
                        &b"The equalised LSB statistic is then most likely noise (or a payload that is encrypted or placed with a key, which only its own tool and password can show)."[..]
                    } else {
                        &b"Nothing points at LSB hiding in this image."[..]
                    });
                    put(b"\n");
                } else {
                    put(b"Verdict: an LSB payload IS in this image (extracted below).\n");
                }
                for f in &found {
                    put(format!("LSB EXTRACTION FOUND something: order {}, {} bit first, {}", f.order, msb_words(f.msb_first), f.why)
                        .as_bytes());
                    if f.length > 0 {
                        put(format!(" ({} bytes)", f.length).as_bytes());
                    }
                    if wav::text_like(&f.data) {
                        put(format!(":\n  {}\n", quote_bytes(&clip_text(&f.data, 600))).as_bytes());
                    } else {
                        put(format!("; first bytes {}\n", hex(&f.data[..f.data.len().min(32)])).as_bytes());
                    }
                }
            }
            Err(e) => put(format!("\nCould not decode the image for an LSB look: {e}\n").as_bytes()),
        },
        "wav" => match wav::parse_wav(b) {
            Ok(w) => {
                let (ones, n) = w.lsb_stats();
                put(format!(
                    "\nWAV: {} channel(s), {} Hz, {} bits, {n} samples; sample LSB ones {}%.\n",
                    w.channels,
                    w.rate,
                    w.bits,
                    fmt_f(100.0 * ones, 1)
                )
                .as_bytes());
                let found = wav::search_wav(&w, 4096);
                if found.is_empty() {
                    put(b"Sample-LSB extraction (both bit orders): nothing that starts like text or a known file.\n");
                }
                for f in &found {
                    put(format!(
                        "SAMPLE-LSB EXTRACTION FOUND something: {} bit first, {}:\n  {}\n",
                        msb_words(f.msb_first),
                        f.why,
                        quote_bytes(&clip_text(&f.data, 600))
                    )
                    .as_bytes());
                }
            }
            Err(e) => put(format!("\nWAV: {e}\n").as_bytes()),
        },
        "text" => {
            put(b"\nInvisible characters and other marks: ");
            put(&watermark::watermarks(b).describe());
            put(b"\n");
        }
        _ => {}
    }
    put(b"\n");
    out.extend_from_slice(&helpers_report(ctx, name));
    out
}

/// The most of a helper program's output that is read (Go's `headBuffer{max: 1 << 20}`).
const MAX_HELPER_OUTPUT: usize = 1 << 20;

/// Go's `helpersReport`: which helpers are on PATH, and what exiftool and binwalk say about the file (20 s each, their
/// output and errors together, 2,500 bytes of it shown).
fn helpers_report(ctx: &Ctx, path: &str) -> Vec<u8> {
    let (mut have, mut missing) = (Vec::new(), Vec::new());
    for h in HELPERS {
        if crate::tools::exec::look_path_go(h).is_ok() {
            have.push(h);
        } else {
            missing.push(h);
        }
    }
    let or_none = |xs: &[&str]| if xs.is_empty() { "none".to_string() } else { xs.join(", ") };
    let mut b = format!("Helper programs: installed: {}; not installed: {}.", or_none(&have), or_none(&missing)).into_bytes();
    let path = match gopath::abs(path.as_bytes()) {
        Ok(p) => lossy(&p),
        Err(_) => path.to_string(),
    };
    for h in &have {
        let args: Vec<String> = match *h {
            "exiftool" => vec!["-S".into(), "-q".into(), "-q".into(), path.clone()],
            "binwalk" => vec![path.clone()],
            _ => continue,
        };
        let (out, err) = run_helper(ctx, h, &args);
        let mut text = gotext::trim_space_bytes(&out).to_vec();
        if text.len() > 2500 {
            text.truncate(2500);
            text.extend_from_slice("\n[…]".as_bytes());
        }
        if let (Some(e), true) = (&err, text.is_empty()) {
            text = format!("({e})").into_bytes();
        }
        b.extend_from_slice(format!("\n{h} was run:\n").as_bytes());
        b.extend_from_slice(&text);
    }
    if have.is_empty() || (have.len() == 1 && have[0] != "exiftool" && have[0] != "binwalk") {
        b.extend_from_slice(b" None of them was run.");
    }
    b
}

/// One helper run as Go's `exec.CommandContext(ctx with 20 s, ...)` with `WaitDelay` 2 s: its first MiB of output and
/// errors together, and Go's `Run` error.
fn run_helper(ctx: &Ctx, h: &str, args: &[String]) -> (Vec<u8>, Option<String>) {
    let mut out = Vec::new();
    // a context already done: Go's Start refuses before starting anything
    if let Some(e) = ctx.err() {
        return (out, Some(e.to_string()));
    }
    let mut deadline = Instant::now() + Duration::from_secs(20);
    if let Some(d) = ctx.deadline() {
        deadline = deadline.min(d);
    }
    let mut sink = |_: &'static str, b: &[u8]| {
        let room = MAX_HELPER_OUTPUT.saturating_sub(out.len());
        out.extend_from_slice(&b[..b.len().min(room)]);
    };
    let env = crate::tools::exec::parent_env();
    let r = crate::tools::exec::run_group(h, args, &env, None, deadline, ctx, crate::tools::exec::GroupKill::Combined, &mut sink);
    let err = match r {
        Err(e) => Some(e),
        Ok(end) => match end.status {
            None => Some("signal: killed".to_string()),
            Some(st) => crate::tools::exec::exit_error(st)
                .or_else(|| end.io_incomplete.then(|| "exec: WaitDelay expired before I/O complete".to_string())),
        },
    };
    (out, err)
}

/// Go's `ExtractReport`: the LSB stream (an image's in `order` with `bits` low bits, a WAV's samples') written to a new
/// temporary folder, and what it starts like.
pub fn extract_report(b: &[u8], name: &str, order: &str, bits: i64) -> Result<Vec<u8>, String> {
    let order = if order.is_empty() { "rgb" } else { order };
    let Some(o) = lsb::ORDERS.iter().find(|o| o.0 == order) else {
        return Err(format!("unknown order {} (rgb, r, g, b, rgba, bgr, a)", quote(order)));
    };
    let (data, how) = if magic::detect(b).name == "wav" {
        let w = wav::parse_wav(b)?;
        (w.lsb_bytes(true, 1 << 22), "sample LSBs, most significant bit first".to_string())
    } else {
        let p = lsb::decode_image(b)?;
        (
            lsb::extract_bits(&p, o.1, bits, true, 1 << 22),
            format!("order {}, {bits} low bit(s) per sample, most significant bit first", o.0),
        )
    };
    let dir = crate::goos::mkdir_temp(b"", "sarina-stego-")?;
    let b_ = base(name);
    let file = format!("{}-lsb-{order}.bin", b_.strip_suffix(ext(name)).unwrap_or(&b_));
    let path = gopath::join(&[&dir, file.as_bytes()]);
    crate::goos::write_file(&path, &data, 0o600)?;
    let mut out = format!("Extracted {} bytes ({how}) to ", data.len()).into_bytes();
    out.extend_from_slice(&path);
    out.extend_from_slice(b" (a temporary folder; it is not kept).\n");
    match wav::judge(&data) {
        Some(f) => {
            out.extend_from_slice(format!("It {}", f.why).as_bytes());
            if f.length > 0 {
                out.extend_from_slice(format!(" ({} bytes)", f.length).as_bytes());
            }
            if wav::text_like(&f.data) {
                out.extend_from_slice(b":\n");
                out.extend_from_slice(&clip_text(&f.data, 2000));
                out.push(b'\n');
            } else {
                out.extend_from_slice(b".\n");
            }
        }
        None => out.extend_from_slice(
            format!("It does not start like text or a known file. First bytes: {}\n", hex(&data[..data.len().min(48)])).as_bytes(),
        ),
    }
    Ok(out)
}

/// Go's `DiffReport`.
pub fn diff_report(a: &[u8], b: &[u8]) -> Result<String, String> {
    let pa = lsb::decode_image(a).map_err(|e| format!("first image: {e}"))?;
    let pb = lsb::decode_image(b).map_err(|e| format!("second image: {e}"))?;
    lsb::diff(&pa, &pb)
}

/// Go's `cleanPath`: NAME.clean.EXT next to the original (`ext` empty: the original's).
pub fn clean_path(orig: &str, ext_: &str) -> String {
    let stem = orig.strip_suffix(ext(orig)).unwrap_or(orig);
    format!("{stem}.clean{}", if ext_.is_empty() { ext(orig) } else { ext_ })
}

/// Go's `Clean`: a cleaned copy of the file written next to it, never over an existing file or through a link
/// planted under its name; (the copy's path, empty when there was nothing to clean; the report).
pub fn clean(b: &[u8], path: &str) -> Result<(String, String), String> {
    let (data, report, ext_) = match gotext::to_lower(ext(path)).as_str() {
        ".png" | ".jpg" | ".jpeg" | ".gif" => {
            let (data, ext_) = watermark::clean_image(b, &mut |n| {
                let mut noise = vec![0u8; n];
                crate::goos::getrandom(&mut noise);
                noise
            })?;
            let report = format!(
                "Cleaned the picture: written again from its pixels only (metadata and anything after the end of the format dropped: {} bytes became {}), the lowest bit of every colour randomised.",
                b.len(),
                data.len()
            );
            (data, report, ext_)
        }
        _ => {
            if b.contains(&0) {
                return Err(format!("{} is binary and not a picture: there is no safe way to clean it here", base(path)));
            }
            let (clean, removed) = watermark::clean_text(b);
            if removed.is_empty() {
                return Ok((
                    String::new(),
                    "Nothing to clean: no invisible characters, stray variation selectors or trailing whitespace.".into(),
                ));
            }
            let mut report = format!("Removed: {}.", removed.join(", "));
            let hs = watermark::watermarks(b).homoglyphs;
            if let Some(h) = hs.first() {
                report += &format!(
                    " Not changed: {} word(s) with look-alike letters from another alphabet (they could be meant; fix them by hand), for example line {}: {}.",
                    hs.len(),
                    h.0,
                    quote(&h.1)
                );
            }
            (clean, report, "")
        }
    };
    let out = clean_path(path, ext_);
    let ob = out.as_bytes();
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(gopath::os_path(ob)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(format!("{} already exists: I do not overwrite it", base(&out)));
            }
            Err(e) => return Err(crate::goerr::path("open", ob, &e)),
        }
    };
    std::io::Write::write_all(&mut f, &data).map_err(|e| crate::goerr::path("write", ob, &e))?;
    Ok((out, report))
}

/// Go's `os.ReadFile`, with its error texts.
fn read_file(p: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(gopath::os_path(p.as_bytes())).map_err(|e| crate::goerr::path("open", p.as_bytes(), &e))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|e| crate::goerr::path("read", p.as_bytes(), &e))?;
    Ok(b)
}

/// `sarina stego inspect FILE | lsb FILE [ORDER] | text FILE | watermark FILE|DIR | clean FILE | diff A B | embed IN.png
/// OUT.png TEXT`.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    if args.len() < 2 {
        env.eprintln(
            "usage: sarina stego inspect FILE | lsb FILE [ORDER] | text FILE | watermark FILE|DIR | clean FILE | diff A B | embed IN.png OUT.png TEXT",
        );
        return 2;
    }
    // Go's read: a file that cannot be read ends the command at once
    macro_rules! read {
        ($p:expr) => {
            match read_file($p) {
                Ok(b) => b,
                Err(e) => {
                    env.eprintln(&format!("sarina stego: {e}"));
                    return 2;
                }
            }
        };
    }
    let res: Result<Vec<u8>, String> = match args[0].as_str() {
        "inspect" => Ok(inspect(&Ctx::background(), &read!(&args[1]), &args[1])),
        "lsb" => {
            let order = args.get(2).map_or("rgb", String::as_str);
            extract_report(&read!(&args[1]), &args[1], order, 1)
        }
        "text" => Ok(watermark::watermarks(&read!(&args[1])).describe()),
        "watermark" => {
            if std::fs::metadata(gopath::os_path(args[1].as_bytes())).is_ok_and(|m| m.is_dir()) {
                Ok(watermark::scan_folder(&args[1], 5000))
            } else {
                Ok(watermark::watermarks(&read!(&args[1])).describe())
            }
        }
        "clean" => clean(&read!(&args[1]), &args[1]).map(|(copy, report)| {
            let mut out = report;
            if !copy.is_empty() {
                out += &format!("\nCleaned copy: {copy} (the original is unchanged)");
            }
            out.into_bytes()
        }),
        "diff" => {
            if args.len() < 3 {
                Err("diff needs two files".into())
            } else {
                let a = read!(&args[1]);
                diff_report(&a, &read!(&args[2])).map(String::into_bytes)
            }
        }
        "embed" => {
            if args.len() < 4 {
                Err("embed IN.png OUT.png TEXT".into())
            } else {
                lsb::decode_image(&read!(&args[1]))
                    .and_then(|p| lsb::embed(&p, args[3..].join(" ").as_bytes()))
                    .and_then(|png| crate::goos::write_file(args[2].as_bytes(), &png, 0o644))
                    .map(|()| format!("wrote {}", args[2]).into_bytes())
            }
        }
        other => Err(format!("unknown subcommand {}", quote(other))),
    };
    match res {
        Ok(mut out) => {
            out.push(b'\n');
            env.stdout.extend_from_slice(&out);
            env.flush();
            0
        }
        Err(e) => {
            env.eprintln(&format!("sarina stego: {e}"));
            2
        }
    }
}
