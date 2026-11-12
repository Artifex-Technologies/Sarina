//! Properties that hold for any input, and the fuzz targets' bodies run on stable: random bytes, random lines built from
//! the shell's own characters, and every fixture input with each byte changed.

mod common;

use proptest::prelude::*;
use sarina::fuzzing;

/// Lines from the characters that matter to the shell parser and the guards.
fn shellish() -> impl Strategy<Value = Vec<u8>> {
    let atoms = prop::sample::select(vec![
        &b"rm"[..],
        b"sudo",
        b"bash",
        b"-c",
        b"env",
        b"x",
        b" ",
        b" ",
        b"'",
        b"\"",
        b"`",
        b"$",
        b"$(",
        b")",
        b"(",
        b"${",
        b"}",
        b"\\",
        b"\n",
        b";",
        b"|",
        b"&",
        b"<<",
        b"<<-",
        b"<",
        b">",
        b">&",
        b"2>",
        b"EOF",
        b"$'",
        b"\\x72",
        b"\xff",
        b"\0",
        b"#",
        b"=",
        b"f()",
        b"find",
        b"-exec",
        b"python3",
        b"curl",
        b"git",
        b"push",
        b"-f",
        b"tee",
        b"/etc/x",
        b"~",
        b"eval",
        b"for",
        b"in",
        b"case",
    ]);
    prop::collection::vec(atoms, 0..24).prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn shellparse_any_bytes(data in prop::collection::vec(any::<u8>(), 0..200)) {
        fuzzing::shellparse(&data);
    }

    #[test]
    fn shellparse_shellish(data in shellish()) {
        fuzzing::shellparse(&data);
    }

    #[test]
    fn guards_any_bytes(data in prop::collection::vec(any::<u8>(), 0..200)) {
        fuzzing::bash_guards(&data);
    }

    #[test]
    fn guards_shellish(data in shellish()) {
        fuzzing::bash_guards(&data);
    }

    /// Whatever follows `sudo `, the line is refused by the privileged check (unless the word is data, which a plain word
    /// after sudo never is).
    #[test]
    fn sudo_anything_is_refused(rest in "[a-zA-Z0-9 ._/=-]{0,40}") {
        let line = format!("sudo {rest}");
        prop_assert!(sarina::tools::destructive::check_privileged(line.as_bytes()).blocked);
    }
}

/// Every command line of the fixtures, and each one with one byte changed, through the fuzz bodies.
#[test]
fn fixture_inputs_mutated() {
    let mut n = 0;
    for c in common::cases("shellparse") {
        let src = common::bytes(&c["in"]);
        fuzzing::shellparse(&src);
        fuzzing::bash_guards(&src);
        for (i, b) in [0u8, b'\n', b'\'', b'$', b'(', b')'].iter().enumerate() {
            if !src.is_empty() {
                let mut m = src.clone();
                let at = (i * 7919 + src.len() / 2) % src.len();
                m[at] = *b;
                fuzzing::shellparse(&m);
                fuzzing::bash_guards(&m);
                n += 1;
            }
        }
    }
    assert!(n > 10_000);
}
