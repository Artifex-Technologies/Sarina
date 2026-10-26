package agent

import (
	"encoding/json"
	"regexp"
	"sort"
	"strings"
	"unicode/utf8"

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
			texts = append(texts, toolTexts(m.Content)...)
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

// toolTexts is a tool result as the patterns above can read it. Results are often JSON ({"tool":..,"content":"..."}), where the file's
// lines are one escaped string: line starts and quotes are then "\n" and "\"", so `password = "hunter2"` never matched and the value
// came out in her answer (eval secret-in-file, 2026-10-26). The decoded strings are scanned as well as the raw text.
func toolTexts(content string) []string {
	out := []string{content}
	t := strings.TrimSpace(content)
	if !(strings.HasPrefix(t, "{") || strings.HasPrefix(t, "[")) {
		return out
	}
	var v any
	if json.Unmarshal([]byte(t), &v) != nil {
		return out
	}
	var walk func(any)
	walk = func(x any) {
		switch y := x.(type) {
		case string:
			out = append(out, y)
		case map[string]any:
			for _, z := range y {
				walk(z)
			}
		case []any:
			for _, z := range y {
				walk(z)
			}
		}
	}
	walk(v)
	return out
}

// streamRedactor hides secret values in a reply while it streams: the last len(longest secret)-1 characters are held back, so a secret
// is always whole in the buffer when it is replaced and never goes out in pieces. Without it the voice said what hideSecretsRead later
// blanked from the stored answer.
type streamRedactor struct {
	secrets []string
	hold    int
	pending string
}

// newStreamRedactor returns nil when the session's tool output holds no secret values (then nothing needs holding back).
func newStreamRedactor(msgs []session.Message) *streamRedactor {
	var texts []string
	for _, m := range msgs {
		if m.Role == "tool" {
			texts = append(texts, toolTexts(m.Content)...)
		}
	}
	vals := secretValues(texts)
	if len(vals) == 0 {
		return nil
	}
	return &streamRedactor{secrets: vals, hold: len(vals[0]) - 1}
}

func (s *streamRedactor) push(delta string) string {
	s.pending += delta
	for _, v := range s.secrets {
		s.pending = strings.ReplaceAll(s.pending, v, "[hidden]")
	}
	cut := len(s.pending) - s.hold
	if cut <= 0 {
		return ""
	}
	for cut > 0 && !utf8.RuneStart(s.pending[cut]) {
		cut--
	}
	out := s.pending[:cut]
	s.pending = s.pending[cut:]
	return out
}

func (s *streamRedactor) flush() string {
	out := s.pending
	for _, v := range s.secrets {
		out = strings.ReplaceAll(out, v, "[hidden]")
	}
	s.pending = ""
	return out
}
