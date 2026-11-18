#![no_main]
// any bytes as sarina-tone's files: the saved baseline and the mood history
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::tone_files(data));
