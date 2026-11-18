#![no_main]
// any bytes as a recording sarina-tone measures (a WAV file read, then analysed)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::tone_wav(data));
