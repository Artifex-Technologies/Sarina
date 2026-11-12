#![no_main]
// any bytes as the workspace policy file (repository-controlled): it may only tighten
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::policy_file(data));
