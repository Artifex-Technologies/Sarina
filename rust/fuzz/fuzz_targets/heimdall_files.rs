#![no_main]
// any bytes as Heimdall's grants file, the signed list's key or signature, or a time in them
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::heimdall_files(data));
