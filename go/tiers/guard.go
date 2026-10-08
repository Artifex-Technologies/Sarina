package tiers

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

var pemBlock = regexp.MustCompile(`(?s)-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----.*?(?:-----END (?:[A-Z]+ )*PRIVATE KEY-----|$)`)

var extraSecretRules = []struct {
	re   *regexp.Regexp
	repl string
}{
	{regexp.MustCompile(`(?i)\bBearer\s+[A-Za-z0-9._~+/=\-]{16,}`), "Bearer [REDACTED:token]"},
	{regexp.MustCompile(`(?i)\b(api[_-]?key|secret|token|passwd|password|credential)s?(["']?\s*[:=]\s*["']?)[^\s"',;]{6,}`), "${1}${2}[REDACTED:assignment]"},
	{regexp.MustCompile(`\bgh[opsur]_[A-Za-z0-9]{30,}`), "[REDACTED:github]"},
	{regexp.MustCompile(`\bglpat-[A-Za-z0-9_\-]{16,}`), "[REDACTED:gitlab]"},
	{regexp.MustCompile(`\bsk-[A-Za-z0-9_\-]{20,}`), "[REDACTED:key]"},
	{regexp.MustCompile(`\bhf_[A-Za-z0-9]{30,}`), "[REDACTED:huggingface]"},
	{regexp.MustCompile(`\bASIA[A-Z0-9]{16}\b`), "[REDACTED:aws]"},
}

// RedactSecrets replaces credential-shaped material with [REDACTED:<kind>]
// markers (the session redactor's rules plus broader ones for text the model
// or operator wrote) and every literal in known (secret values the process
// holds). It reports how many replacements were made.
func RedactSecrets(text string, known []string) (string, int) {
	count := 0
	for _, k := range known {
		if len(k) >= 8 && strings.Contains(text, k) {
			count += strings.Count(text, k)
			text = strings.ReplaceAll(text, k, "[REDACTED:key]")
		}
	}
	text = pemBlock.ReplaceAllStringFunc(text, func(string) string { count++; return "[REDACTED:pem]" })
	before := strings.Count(text, "[REDACTED:")
	text = session.Redact(text)
	count += strings.Count(text, "[REDACTED:") - before
	for _, r := range extraSecretRules {
		text = r.re.ReplaceAllStringFunc(text, func(m string) string {
			if strings.Contains(m, "[REDACTED:") {
				return m // already redacted; do not count it twice
			}
			count++
			return r.re.ReplaceAllString(m, r.repl)
		})
	}
	return text, count
}

var captureRules = []*regexp.Regexp{
	regexp.MustCompile(`(?m)^Filetype:\s*Flipper\b`),
	regexp.MustCompile(`(?mi)^RAW_Data:\s*-?\d`),
	regexp.MustCompile(`(?m)^(?:ATQA|SAK):\s*[0-9A-Fa-f]{2}`),
	regexp.MustCompile(`(?m)^UID:\s*(?:[0-9A-Fa-f]{2}\s){3,}`),
	regexp.MustCompile(`(?m)^Block\s+\d+:\s*(?:[0-9A-Fa-f]{2}\s){8,}`),
}

// LooksLikeCapture reports whether text contains an RF/NFC capture dump.
func LooksLikeCapture(text string) bool {
	for _, re := range captureRules {
		if re.MatchString(text) {
			return true
		}
	}
	return false
}

// GuardReport describes what the allow_data guard did to one request.
type GuardReport struct {
	Redactions int
}

// RefusedError means a tier's allow_data forbids the request; nothing was sent.
type RefusedError struct{ Reason string }

func (e *RefusedError) Error() string { return e.Reason }

// Sanitize returns a copy of req that is safe to send to a tier with the given
// allow_data. The caller's request (and so the session) is never modified.
// Secrets are redacted; capture dumps refuse the tier outright, because they
// cannot be reliably cut out of free text.
func Sanitize(req llm.Request, allow AllowData, known []string) (llm.Request, GuardReport, error) {
	var rep GuardReport
	if allow.All {
		return req, rep, nil
	}
	if allow.Has(ClassNoCaptures) {
		for _, m := range req.Messages {
			if LooksLikeCapture(m.Content) || anyToolArgs(m, LooksLikeCapture) {
				return req, rep, &RefusedError{Reason: "the conversation contains capture data (RF/NFC dump) and allow_data has no_captures"}
			}
		}
	}
	if !allow.Has(ClassNoSecrets) {
		return req, rep, nil
	}
	out := req
	out.Messages = make([]llm.Message, len(req.Messages))
	for i, m := range req.Messages {
		c, n := RedactSecrets(m.Content, known)
		rep.Redactions += n
		m.Content = c
		if len(m.ToolCalls) > 0 {
			calls := make([]llm.WireToolCall, len(m.ToolCalls))
			for j, tc := range m.ToolCalls {
				a, n := RedactSecrets(tc.Function.Arguments, known)
				rep.Redactions += n
				tc.Function.Arguments = a
				calls[j] = tc
			}
			m.ToolCalls = calls
		}
		out.Messages[i] = m
	}
	return out, rep, nil
}

func anyToolArgs(m llm.Message, f func(string) bool) bool {
	for _, tc := range m.ToolCalls {
		if f(tc.Function.Arguments) {
			return true
		}
	}
	return false
}

func describeGuard(rep GuardReport) string {
	if rep.Redactions == 0 {
		return ""
	}
	return fmt.Sprintf("%d secret-shaped value(s) redacted before sending", rep.Redactions)
}
