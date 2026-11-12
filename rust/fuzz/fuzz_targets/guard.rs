#![no_main]
// any bytes through the text gate (what memory, skills and notes write goes through it)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::guard(data));
