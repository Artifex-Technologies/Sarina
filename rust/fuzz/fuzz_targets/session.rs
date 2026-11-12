#![no_main]
// any bytes as a saved session file (Heimdall reads them; `sarina resume` loads them)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::session(data));
