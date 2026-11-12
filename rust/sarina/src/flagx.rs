//! Go: `go/flagx`. Interspersed flag parsing over Go's flag package ([`crate::goflag`]).
//!
//! The standard FlagSet stops at the first non-flag argument, so `sarina chat "do a thing" --allow-write` would silently
//! treat `--allow-write` as part of the prompt. [`parse`] keeps going, so flags work before, between and after positional
//! arguments, and a bare "--" ends flag parsing.

use crate::goflag::FlagSet;

/// Parses args into fs and returns the positional arguments in order. `out` gets what Go's flag set writes to its output
/// (an error and the usage).
pub fn parse(fs: &mut FlagSet, args: &[String], out: &mut Vec<u8>) -> Result<Vec<String>, String> {
    let mut positional = Vec::new();
    let mut args = args.to_vec();
    loop {
        let rest = fs.parse(&args, out)?;
        if rest.is_empty() {
            return Ok(positional);
        }
        let consumed = args.len() - rest.len();
        if consumed > 0 && args[consumed - 1] == "--" {
            // parsing stopped at an explicit terminator: the rest is positional
            positional.extend(rest);
            return Ok(positional);
        }
        positional.push(rest[0].clone());
        args = rest[1..].to_vec();
        if args.is_empty() {
            return Ok(positional);
        }
    }
}
