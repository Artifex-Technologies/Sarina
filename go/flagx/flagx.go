// Package flagx adds interspersed flag parsing to the standard flag package.
//
// The standard FlagSet stops at the first non-flag argument, so
// `sarina chat "do a thing" --allow-write` silently treats `--allow-write` as
// part of the prompt. Parse keeps going, so flags work before, between and
// after positional arguments, and a bare "--" ends flag parsing.
//
// Use it only for commands whose documented usage puts flags after the
// positionals. Commands that pass the tail through verbatim (for example a
// guest command that has flags of its own) must keep using FlagSet.Parse.
package flagx

import "flag"

// Parse parses args into fs and returns the positional arguments in order.
func Parse(fs *flag.FlagSet, args []string) ([]string, error) {
	var positional []string
	for {
		if err := fs.Parse(args); err != nil {
			return nil, err
		}
		rest := fs.Args()
		if len(rest) == 0 {
			return positional, nil
		}
		consumed := len(args) - len(rest)
		if consumed > 0 && args[consumed-1] == "--" {
			// Parsing stopped at an explicit terminator: the rest is positional.
			return append(positional, rest...), nil
		}
		positional = append(positional, rest[0])
		args = rest[1:]
		if len(args) == 0 {
			return positional, nil
		}
	}
}
