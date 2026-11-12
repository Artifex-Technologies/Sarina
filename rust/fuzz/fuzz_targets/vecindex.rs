#![no_main]
// any bytes as a saved vector index (Sarina's memory and skill indexes)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::vecindex(data));
