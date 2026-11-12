#![no_main]
// any bytes as an analysis cache file (~/.sarina/cache)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::cache_file(data));
