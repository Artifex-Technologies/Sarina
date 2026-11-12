//! Go's TestSessionReadsRefusePipesAndHugeFiles and TestSearchSkipsNamedPipes (2026-11-12): a named pipe under a session
//! file's name is refused at once (not waited on), a file past the bound is not read; Load, MigrateDir and the session
//! search agree.

use std::ffi::CString;
use std::time::Duration;

use sarina::session::readfile::{load_by_id, read_regular, MAX_FILE_BYTES};

fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| panic!("{what} waited on a named pipe"))
}

#[test]
fn session_reads_refuse_pipes_and_huge_files() {
    let dir = std::env::temp_dir().join(format!("sarina-session-reads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pipe = CString::new(dir.join("pipe.json").to_string_lossy().into_owned()).unwrap();
    // SAFETY: mkfifo on a path this test made
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
    let huge = dir.join("huge.json");
    std::fs::File::create(&huge).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
    let d = dir.to_string_lossy().into_owned();

    let d2 = d.clone();
    let err = within("load_by_id", move || load_by_id(&d2, "pipe").err().unwrap());
    assert_eq!(err, format!("{d}/pipe.json is not a regular file"));
    assert_eq!(load_by_id(&d, "huge").err().unwrap(), format!("{d}/huge.json is larger than {MAX_FILE_BYTES} bytes; not read"));
    assert!(load_by_id(&d, "absent").err().unwrap().ends_with("absent.json: no such file or directory"));
    let d2 = d.clone();
    let (r, err) = within("migrate_dir", move || sarina::session::save::migrate_dir(&d2, true, &|_: &str| String::new()));
    assert!(err.is_none());
    assert_eq!((r.skipped, r.scanned), (2, 0));
    assert_eq!(read_regular(huge.to_string_lossy().as_bytes(), 1 << 40).unwrap().len() as u64, MAX_FILE_BYTES + 1);

    std::fs::write(dir.join("b.json"), r#"{"messages":[{"role":"user","content":"where is the blue widget"}]}"#).unwrap();
    std::fs::remove_file(&huge).unwrap();
    let d2 = d.clone();
    let hits = within("the session search", move || sarina::sessionsearch::search("blue widget", &d2, 5).len());
    assert_eq!(hits, 1);
    let _ = std::fs::remove_dir_all(&dir);
}
