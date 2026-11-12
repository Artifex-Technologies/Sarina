#![no_main]
// any bytes through CheckDestructive and CheckPrivileged
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::bash_guards(data));
