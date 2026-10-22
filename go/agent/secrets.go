package agent

import (
	"regexp"
	"sort"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/session"
)

// Secrets she read must never come back out in an answer, whatever the model decides: values assigned to password-, secret-, token- and
// key-like names in what her tools returned in this task are blanked in the final answer (the high-confidence credential shapes are already
// redacted from tool output before the model sees them).

var secretAssignRE = regexp.MustCompile(`(?im)^[ \t]*(?:export[ \t]+)?["']?[\w.-]*(?:pass(?:word|wd|phrase)?|secret|token|api[_-]?key|private[_-]?key|credential|auth)[\w.-]*["']?[ \t]*[=:][ \t]*["']?([^\s"'#,;]{4,})`)

// secretValues finds the values of secret-looking settings in tool output.
func secretValues(texts []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, t := range texts {
		for _, m := range secretAssignRE.FindAllStringSubmatch(t, -1) {
			v := m[1]
			if seen[v] || strings.HasPrefix(v, "[REDACTED") || strings.HasPrefix(v, "$") || strings.EqualFold(v, "true") || strings.EqualFold(v, "false") || strings.EqualFold(v, "null") {
				continue
			}
			seen[v] = true
			out = append(out, v)
		}
	}
	// longest first, so a value that contains another is hidden whole
	sort.Slice(out, func(i, j int) bool { return len(out[i]) > len(out[j]) })
	return out
}

// hideSecretsRead blanks secret values from this task's tool output wherever they appear in the answer.
func hideSecretsRead(answer string, msgs []session.Message) (string, bool) {
	var texts []string
	for _, m := range msgs {
		if m.Role == "tool" {
			texts = append(texts, m.Content)
		}
	}
	changed := false
	for _, v := range secretValues(texts) {
		if strings.Contains(answer, v) {
			answer = strings.ReplaceAll(answer, v, "[hidden]")
			changed = true
		}
	}
	return answer, changed
}

func hideAll(text string, msgs []session.Message) string {
	out, _ := hideSecretsRead(text, msgs)
	return out
}
