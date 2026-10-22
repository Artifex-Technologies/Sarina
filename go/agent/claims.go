package agent

import (
	"path/filepath"
	"regexp"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/session"
)

// A small model sometimes finishes by saying it changed a file it never wrote (it used the wrong tool, or none). Before such an answer is
// accepted, the claim is checked against what the file tools actually did in this task; if they did not touch that file, the model is told so
// once and asked to either do it or say plainly that it did not happen.

var (
	claimFileRE = regexp.MustCompile(`(?i)\b([\w.-]+\.(?:md|txt|json|py|go|rs|toml|ya?ml|sh|conf|env|ini|csv|js|ts|html|css|c|h|cpp|java|lua|xml))\b`)
	claimVerbRE = regexp.MustCompile(`(?i)\b(added|updated|changed|wrote|written|saved|edited|appended|created|modified|replaced|fixed|inserted|removed|deleted|renamed)\b`)
	negationRE  = regexp.MustCompile(`(?i)\b(not|n't|never|no|nothing|without|unable|cannot|could not|did not|didn't|won't|wasn't|haven't)\b`)
	sentenceEnd = regexp.MustCompile(`[.!?](?:\s+|$)|\n`)
)

var fileWriteTools = map[string]bool{"write_file": true, "edit_file": true, "vault_write": true}

// unbackedFileClaim returns the first file the answer says it changed that no file-writing tool changed successfully in this task, or "".
func unbackedFileClaim(answer string, history []map[string]any) string {
	written := map[string]bool{}
	for _, e := range history {
		name, _ := e["tool_name"].(string)
		ok, _ := e["ok"].(bool)
		if !fileWriteTools[name] || !ok {
			continue
		}
		if p, _ := e["path"].(string); p != "" {
			written[strings.ToLower(filepath.Base(p))] = true
		}
		if ps, ok := e["changed_paths"].([]string); ok {
			for _, p := range ps {
				written[strings.ToLower(filepath.Base(p))] = true
			}
		}
	}
	for _, s := range sentenceEnd.Split(answer, -1) {
		if !claimVerbRE.MatchString(s) || negationRE.MatchString(s) {
			continue
		}
		for _, m := range claimFileRE.FindAllStringSubmatch(s, -1) {
			f := strings.ToLower(filepath.Base(m[1]))
			if !written[f] {
				return m[1]
			}
		}
	}
	return ""
}

func claimCheckText(file string) string {
	return "Check before you finish: your answer says " + file + " was changed, but no file tool changed " + file +
		" in this task (a to-do or plan tool does not write files). Either make the change now with the file tools, or say plainly that " + file + " was not changed."
}

// unreportedChange returns a file a file-writing tool changed in this task that the answer never mentions, or "": a change the user is not
// told about is as bad as one that is claimed but never made.
func unreportedChange(answer string, history []map[string]any) string {
	low := strings.ToLower(answer)
	for _, e := range history {
		name, _ := e["tool_name"].(string)
		ok, _ := e["ok"].(bool)
		if !fileWriteTools[name] || !ok {
			continue
		}
		p, _ := e["path"].(string)
		if p == "" {
			continue
		}
		base := filepath.Base(p)
		if !strings.Contains(low, strings.ToLower(base)) && !strings.Contains(low, strings.ToLower(strings.TrimSuffix(base, filepath.Ext(base)))) {
			return base
		}
	}
	return ""
}

func unreportedText(file string) string {
	return "Check before you finish: in this task you changed " + file + ", but your answer does not say so. Tell the user plainly what you changed in " + file + " and what it holds now."
}

var quotedRE = regexp.MustCompile("(?:^|[\\s(:,])(?:`([^`\\n]{4,60})`|\"([^\"\\n]{4,60})\")(?:$|[\\s).,:;!?])")

// ungroundedQuote returns a literal the answer quotes (in backticks or double quotes) that appears in none of this task's tool output and not in
// the prompt, when tools were used at all: quoting a string as if it came from the evidence, when it did not, is how a small model invents
// passwords and messages a program never contained.
func ungroundedQuote(answer, prompt string, msgs []session.Message) string {
	var evidence strings.Builder
	used := false
	for _, m := range msgs {
		if m.Role == "tool" {
			used = true
			evidence.WriteString(m.Content)
			evidence.WriteByte('\n')
		}
	}
	if !used {
		return ""
	}
	ev := strings.ToLower(evidence.String())
	p := strings.ToLower(prompt)
	for _, m := range quotedRE.FindAllStringSubmatch(answer, -1) {
		q := m[1]
		if q == "" {
			q = m[2]
		}
		lq := strings.ToLower(strings.TrimSpace(q))
		if lq == "" || strings.Contains(ev, lq) || strings.Contains(p, lq) {
			continue
		}
		// a path, a command or a name the model writes in its own words is not a quote from the evidence: only literal-looking text counts
		if strings.ContainsAny(q, "/\\") || strings.HasPrefix(q, "-") {
			continue
		}
		return q
	}
	return ""
}

func ungroundedText(q string) string {
	return "Check before you finish: your answer quotes \"" + q + "\", but no tool output in this task contains it. Only state what the tools showed: look again with your tools, or say plainly that you have not seen it."
}
