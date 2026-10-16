// Package guard is the ONE text-security gate. Anything that will be written
// somewhere a future prompt reads from (curated memory, skills, session notes,
// compaction summaries) goes through Gate.Scan: invisible/bidirectional
// Unicode, instruction-override phrases and credential shapes are refused,
// and, for a gate with a HeimdallKind, Heimdall gets an out-of-process
// second opinion after the local checks pass.
//
// Keep every pattern list here. A second copy elsewhere is how gates drift.
package guard

import (
	"fmt"
	"regexp"
	"strings"
	"unicode"

	"github.com/Artifex-Technologies/Sarina/go/heimdall"
)

// Result is a scan verdict.
type Result struct {
	Blocked bool
	Reason  string
}

// Scanner vets text.
type Scanner interface {
	Scan(text string) Result
}

// Gate is the shared Scanner.
type Gate struct {
	// HeimdallKind, when set, makes Scan consult Heimdall (POST /v1/review) with
	// this kind after the local checks pass. Memory uses "memory_proposal".
	HeimdallKind string
	// SessionID is sent along to Heimdall (memory sends none today).
	SessionID string
	// AllowLeadingBOM tolerates a byte-order mark at the very start (whole
	// files read from disk); everywhere else it is blocked.
	AllowLeadingBOM bool
}

// Default is the gate for operator-visible local text: all local checks, no
// Heimdall call (every skill load and notes refresh would otherwise wait on it).
func Default() Gate { return Gate{AllowLeadingBOM: true} }

// Memory is the gate for curated-memory writes: local checks, then Heimdall.
func Memory() Gate { return Gate{HeimdallKind: "memory_proposal"} }

var invisibleCodepoints = map[rune]bool{
	0x200B: true, 0x200C: true, 0x200D: true, 0x2060: true, 0xFEFF: true, 0x00AD: true,
}

// injection phrases: "looks like an instruction rather than a fact".
var injectionPatterns = []*regexp.Regexp{
	regexp.MustCompile(`(?i)ignore (all|any|the)?\s*(previous|prior|above)\s*instructions`),
	regexp.MustCompile(`(?i)disregard (all|any|the)?\s*(previous|prior|above)`),
	regexp.MustCompile(`(?i)you are now\b`),
	regexp.MustCompile(`(?i)new (system )?instructions?\s*:`),
	regexp.MustCompile(`(?i)act as if\b`),
	regexp.MustCompile(`(?i)override (your|the|all) instructions`),
	regexp.MustCompile(`(?i)do not (tell|inform|notify) the user`),
	regexp.MustCompile(`(?i)without (informing|telling|notifying) the user`),
	regexp.MustCompile(`(?i)system prompt\s*:`),
	// broader shapes the Go skills scanner already refused
	regexp.MustCompile(`(?i)\b(ignore|disregard|forget|override)\b[^.\n]{0,40}\b(previous|prior|above|earlier|all|any|your)\b[^.\n]{0,40}\b(instructions?|rules|prompts?|guidelines|constraints)\b`),
	regexp.MustCompile(`(?i)\byou are no longer\b`),
	regexp.MustCompile(`(?i)\b(reveal|print|show|output)\b[^.\n]{0,30}\b(system prompt|hidden instructions)\b`),
	regexp.MustCompile(`(?i)\b(system|important|urgent|admin(istrator)?|security) (notice|message|instructions?|directive)s? (to|for) (the )?(ai|assistant|agent|model|llm)\b`),
	regexp.MustCompile(`(?i)\b(notes?|messages?|instructions?) (to|for) (the )?(ai|assistant|agent|llm|language model)\s*:`),
	regexp.MustCompile(`(?i)\b(bypass|disable|turn off)\b[^.\n]{0,30}\b(safety|permission|approval|guardrails?)\b`),
}

var credentialPatterns = []*regexp.Regexp{
	regexp.MustCompile(`-----BEGIN [A-Z ]*PRIVATE KEY`),
	regexp.MustCompile(`ssh-(rsa|ed25519|dss) AAAA[0-9A-Za-z+/]{10,}`),
	regexp.MustCompile(`\b(AKIA|ASIA)[0-9A-Z]{16}\b`),
	regexp.MustCompile(`ghp_[0-9A-Za-z]{36}`),
	regexp.MustCompile(`\bgh[pousr]_[A-Za-z0-9]{30,}`),
	regexp.MustCompile(`\bsk-[A-Za-z0-9_-]{20,}`),
	regexp.MustCompile(`xox[baprs]-[0-9A-Za-z-]{10,}`),
	regexp.MustCompile(`\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}`),
	regexp.MustCompile(`(?i)api[_-]?key\s*[:=]\s*['"]?[0-9A-Za-z_\-]{16,}`),
	regexp.MustCompile(`(?i)password\s*[:=]\s*\S+`),
	regexp.MustCompile(`(?i)secret[_-]?(key|token)?\s*[:=]\s*['"]?[0-9A-Za-z_\-]{12,}`),
	regexp.MustCompile(`(?i)\b(token|passw(or)?d|private[_-]?key)\b["']?\s*[:=]\s*["']?[A-Za-z0-9_\-+/=.]{12,}`),
}

// Scan implements Scanner.
func (g Gate) Scan(text string) Result {
	for i, r := range text {
		if g.AllowLeadingBOM && i == 0 && r == 0xFEFF {
			continue
		}
		if invisibleCodepoints[r] || unicode.Is(unicode.Cf, r) || (r >= 0xE0000 && r < 0xE0080) {
			return Result{true, fmt.Sprintf("contains an invisible/formatting Unicode character (U+%04X), which is not permitted here", r)}
		}
	}
	for _, p := range injectionPatterns {
		if m := p.FindString(text); m != "" {
			return Result{true, fmt.Sprintf("looks like an instruction rather than a fact (%q)", m)}
		}
	}
	for _, p := range credentialPatterns {
		if p.MatchString(text) {
			return Result{true, "looks like it contains a credential or secret; those belong outside memory"}
		}
	}
	if g.HeimdallKind != "" {
		if v := heimdall.Consult(text, g.HeimdallKind, g.SessionID); v != nil && v.Denied {
			return Result{true, "Heimdall flagged this independently: " + strings.Join(v.Reasons, "; ")}
		}
	}
	return Result{}
}
