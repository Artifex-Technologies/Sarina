#![no_main]
// a glob segment the model writes and a file name (glob_search's path.Match)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::glob_pattern(data));
