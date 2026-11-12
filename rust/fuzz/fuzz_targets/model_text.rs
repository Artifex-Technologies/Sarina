#![no_main]
// any bytes as a model's answer: the JSON repair and the normaliser for local servers
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::model_text(data));
