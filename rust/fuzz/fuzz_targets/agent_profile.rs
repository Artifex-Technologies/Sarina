#![no_main]
// any bytes as an operator's sub-agent profile (~/.sarina/agents/*.md)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::agent_profile(data));
