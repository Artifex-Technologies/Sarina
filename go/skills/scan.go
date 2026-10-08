package skills

import "github.com/Artifex-Technologies/Sarina/go/guard"

// ScanResult is a Scanner's verdict on a piece of text.
type ScanResult = guard.Result

// Scanner vets skill text before it can be used. It is the shared gate's
// interface, so one implementation serves skills, session notes and memory.
type Scanner = guard.Scanner

// BasicScanner is the shared security gate (package guard) without the Sentry
// call: invisible/bidirectional characters, credential shapes and
// instruction-override phrases. Kept as a name so existing callers compile.
type BasicScanner struct{}

// Scan implements Scanner.
func (BasicScanner) Scan(text string) ScanResult { return guard.Default().Scan(text) }
