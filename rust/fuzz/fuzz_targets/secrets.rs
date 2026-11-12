#![no_main]
// any content the model writes (ExfilRisk, PrivateReference) and any bytes as a file name (secretName)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::secrets(data));
