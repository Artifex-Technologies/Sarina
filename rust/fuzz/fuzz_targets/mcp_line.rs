#![no_main]
// any bytes as a line from an MCP server
libfuzzer_sys::fuzz_target!(|data: &[u8]| sarina::fuzzing::mcp_line(data));
