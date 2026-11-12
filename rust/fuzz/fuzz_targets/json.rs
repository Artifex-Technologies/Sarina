#![no_main]
// any bytes as a JSON document Sarina reads (session files, model and MCP answers), with Go's rules
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::json(data));
