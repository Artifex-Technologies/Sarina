#![no_main]
// any bytes as an answer from Limbo's agent socket (profiles, runs, the skipped list)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::limbo_answer(data));
