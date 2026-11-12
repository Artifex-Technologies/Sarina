#![no_main]
// any bytes as an OpenAI-compatible server's answer, streamed and not
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::model_answer(data));
