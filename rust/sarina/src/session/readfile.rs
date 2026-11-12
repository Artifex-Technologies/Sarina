//! Go: `session/readfile.go`: a session file (or a session's notes) read as a regular file of bounded size. It is opened
//! without blocking, so a named pipe or a device planted under the name is refused at once instead of waiting forever
//! for a writer; a link is followed.

use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;

/// Go's `MaxFileBytes`.
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// Go's `ReadFile`: [`read_regular`] with [`MAX_FILE_BYTES`].
pub fn read_file(path: &[u8]) -> Result<Vec<u8>, String> {
    read_regular(path, MAX_FILE_BYTES)
}

/// Go's `ReadRegular`, with Go's error texts.
pub fn read_regular(path: &[u8], max: u64) -> Result<Vec<u8>, String> {
    let p = crate::gopath::os_path(path);
    let f =
        std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(p).map_err(|e| crate::goerr::path("open", path, &e))?;
    let st = f.metadata().map_err(|e| crate::goerr::path("stat", path, &e))?;
    let shown = crate::gotext::lossy(path);
    if !st.file_type().is_file() {
        return Err(format!("{shown} is not a regular file"));
    }
    let mut b = Vec::new();
    f.take(max + 1).read_to_end(&mut b).map_err(|e| crate::goerr::path("read", path, &e))?;
    if b.len() as u64 > max {
        return Err(format!("{shown} is larger than {max} bytes; not read"));
    }
    Ok(b)
}

/// Go's `Load`: a session file read and parsed (retired tool names rewritten in memory).
pub fn load(path: &str) -> Result<super::Session, String> {
    super::parse(&read_file(path.as_bytes())?)
}

/// Go's `LoadByID`.
pub fn load_by_id(dir: &str, id: &str) -> Result<super::Session, String> {
    load(&super::path(dir, id))
}
