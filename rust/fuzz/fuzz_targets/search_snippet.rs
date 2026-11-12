#![no_main]
// any text as a saved message and a query for the session search's snippet
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::search_snippet(data));
