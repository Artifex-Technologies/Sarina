#![no_main]
// any text as a number a model sent (ValidateArgs coerces numeric strings with Go's ParseFloat)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::parse_float(data));
