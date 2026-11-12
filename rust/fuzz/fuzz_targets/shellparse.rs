#![no_main]
// any bytes as a shell command line (the bash tool's command, a -c script, a here-document)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::shellparse(data));
