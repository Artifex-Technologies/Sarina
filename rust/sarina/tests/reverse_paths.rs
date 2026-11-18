//! Go's TestSandboxPathsAreReadAsTheirFoldersAndNothingElse (go/reverse/reverse_test.go): a path as the sandbox shows it
//! (Heimdall names a held file it lent Sarina as /quarantine/<sha256>.sample) is read as that folder and checked like any
//! other path, so it reaches nothing the folders do not hold. Its own test binary: it sets HOME and LABYRINTH_REA_SAMPLES.

use sarina::reverse::{args, quarantine_dir, sandbox_path};
use std::os::unix::ffi::OsStrExt;

#[test]
fn sandbox_paths_are_read_as_their_folders_and_nothing_else() {
    let base = std::env::temp_dir().join(format!("sarina-reverse-paths-{}", std::process::id()));
    let (samples, home) = (base.join("samples"), base.join("home"));
    std::fs::create_dir_all(&samples).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("LABYRINTH_REA_SAMPLES", &samples);
    std::env::set_var("HOME", &home);
    let q = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&quarantine_dir()));
    std::fs::create_dir_all(&q).unwrap();
    std::fs::write(q.join("abc.sample"), "x").unwrap();
    std::fs::write(samples.join("a.bin"), "x").unwrap();
    for (input, want) in [
        ("/quarantine/abc.sample", "/quarantine/abc.sample"),
        ("/quarantine//./abc.sample", "/quarantine/abc.sample"),
        ("/samples/a.bin", "/samples/a.bin"),
        ("/samples/sub/../a.bin", "/samples/a.bin"),
        ("/quarantine/x/../../samples/a.bin", "/samples/a.bin"),
    ] {
        assert_eq!(sandbox_path(input).map(|b| String::from_utf8(b).unwrap()), Ok(want.to_string()), "{input}");
    }
    std::os::unix::fs::symlink("/etc/passwd", q.join("link")).unwrap();
    let ssh = home.join(".ssh/id_ed25519");
    for bad in [
        "/quarantine/../etc/passwd",
        "/quarantine/../../etc/shadow",
        "/samples/../x",
        "/quarantine/link",
        "/quarantinex/abc.sample",
        "/etc/passwd",
        "/proc/self/environ",
        ssh.to_str().unwrap(),
    ] {
        assert!(sandbox_path(bad).is_err(), "{bad} was accepted");
    }
    let a = args("analyze", "/quarantine/abc.sample", "", 0).unwrap();
    assert_eq!(a[1], b"/quarantine/abc.sample");
    let _ = std::fs::remove_dir_all(&base);
}
