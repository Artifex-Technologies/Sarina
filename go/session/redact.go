package session

import (
	"regexp"
	"strings"
)

type redactRule struct {
	re   *regexp.Regexp
	kind func(match string) string
}

func fixed(kind string) func(string) string { return func(string) string { return kind } }

var redactRules = []redactRule{
	{regexp.MustCompile(`sk-(ant|proj|or|live|test)-[A-Za-z0-9_\-]{8,}`), func(m string) string {
		rest := strings.TrimPrefix(m, "sk-")
		return rest[:strings.IndexByte(rest, '-')]
	}},
	{regexp.MustCompile(`\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{16,}`), fixed("stripe")},
	{regexp.MustCompile(`\bghp_[A-Za-z0-9]{20,}`), fixed("github")},
	{regexp.MustCompile(`\bAKIA[A-Z0-9]{16,}`), fixed("aws")},
	{regexp.MustCompile(`\bxoxb-[A-Za-z0-9\-]{20,}`), fixed("slack")},
	{regexp.MustCompile(`\bAIza[A-Za-z0-9_\-]{35}`), fixed("google")},
	{regexp.MustCompile(`\beyJ[A-Za-z0-9_\-]{5,}\.eyJ[A-Za-z0-9_\-]{5,}\.[A-Za-z0-9_\-]{5,}`), fixed("jwt")},
	{regexp.MustCompile(`-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----`), fixed("pem")},
}

// Redact replaces high-confidence credentials in tool output with
// [REDACTED:<kind>]. It is applied to tool results only.
func Redact(text string) string {
	for _, r := range redactRules {
		rule := r
		text = rule.re.ReplaceAllStringFunc(text, func(m string) string {
			return "[REDACTED:" + rule.kind(m) + "]"
		})
	}
	return text
}
