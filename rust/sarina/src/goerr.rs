//! Go's error texts (from Heimdall's port): the kernel's error numbers as Go names them (`syscall.Errno.Error`, its own
//! table: `goerr_table.rs`, written by Heimdall's rust/gotools/errtable) and the shape of Go's `os.PathError`.

#[path = "goerr_table.rs"]
mod table;

/// Go's `syscall.Errno(n).Error()`.
pub fn errno(n: i32) -> String {
    match usize::try_from(n).ok().and_then(|i| table::ERRNO.get(i)) {
        Some(s) => s.to_string(),
        None => format!("errno {n}"),
    }
}

/// The text Go prints for an error Rust got from the kernel; an error without a number keeps Rust's own text.
pub fn text(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => errno(n),
        None => e.to_string(),
    }
}

/// Go's `*os.PathError`: "lstat /x: no such file or directory".
pub fn path(op: &str, path: &[u8], e: &std::io::Error) -> String {
    format!("{op} {}: {}", crate::gotext::lossy(path), text(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_texts() {
        assert_eq!(errno(2), "no such file or directory");
        assert_eq!(errno(40), "too many levels of symbolic links");
        assert_eq!(errno(9999), "errno 9999");
        assert_eq!(path("lstat", b"/x", &std::io::Error::from_raw_os_error(20)), "lstat /x: not a directory");
    }
}
