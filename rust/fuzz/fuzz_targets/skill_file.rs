#![no_main]
// any bytes as a skill file (~/.sarina/skills, SARINA_SKILLS_DIR, the system folder)
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::skill_file(data));
