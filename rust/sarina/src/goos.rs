//! Go's `os` functions the ported code writes files with, with Go's error texts: `MkdirAll`, `CreateTemp`, `WriteFile`.

use crate::goerr;
use crate::gopath;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

/// Go's `os.CreateTemp(dir, pattern)`: a new file, mode 0600, named by the pattern with its last `*` replaced by Go's
/// random number; a pattern with a slash is refused as Go refuses it; the error names the last name tried.
pub fn create_temp(dir: &[u8], pattern: &str) -> Result<(std::fs::File, Vec<u8>), String> {
    if pattern.contains('/') {
        return Err(format!("createtemp {pattern}: pattern contains path separator"));
    }
    let (prefix, suffix) = match pattern.rfind('*') {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => (pattern, ""),
    };
    let mut tries = 0;
    loop {
        let mut r = [0u8; 4];
        getrandom(&mut r);
        let name = gopath::join(&[dir, format!("{prefix}{}{suffix}", u32::from_le_bytes(r)).as_bytes()]);
        match std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(gopath::os_path(&name)) {
            Ok(f) => return Ok((f, name)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 10_000 => tries += 1,
            Err(e) => return Err(goerr::path("open", &name, &e)),
        }
    }
}

/// Go's `os.WriteFile(path, data, perm)`: created with `perm` (before the umask) or truncated, with Go's error texts.
pub fn write_file(path: &[u8], data: &[u8], perm: u32) -> Result<(), String> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(perm)
        .open(gopath::os_path(path))
        .map_err(|e| goerr::path("open", path, &e))?;
    f.write_all(data).map_err(|e| goerr::path("write", path, &e))
}

/// Random bytes from the kernel (ring's generator, the one Heimdall's keys use).
fn getrandom(buf: &mut [u8]) {
    use ring::rand::SecureRandom;
    let _ = ring::rand::SystemRandom::new().fill(buf);
}

/// Go's `os.MkdirAll(dir, 0o755)` with Go's error texts.
pub fn mkdir_all(dir: &[u8]) -> Result<(), String> {
    mkdir_all_mode(dir, 0o755)
}

/// Go's `os.MkdirAll(dir, mode)` with Go's error texts.
pub fn mkdir_all_mode(dir: &[u8], mode: u32) -> Result<(), String> {
    match std::fs::metadata(gopath::os_path(dir)) {
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => return Err(goerr::path("mkdir", dir, &std::io::Error::from_raw_os_error(20))),
        Err(_) => {}
    }
    let parent = gopath::dir(dir);
    if parent != dir && !parent.is_empty() {
        mkdir_all_mode(&parent, mode)?;
    }
    match std::fs::DirBuilder::new().mode(mode).create(gopath::os_path(dir)) {
        Ok(()) => Ok(()),
        // made meanwhile (Go: a folder that exists now is fine)
        Err(e) if std::fs::metadata(gopath::os_path(dir)).is_ok_and(|m| m.is_dir()) => {
            let _ = e;
            Ok(())
        }
        Err(e) => Err(goerr::path("mkdir", dir, &e)),
    }
}
