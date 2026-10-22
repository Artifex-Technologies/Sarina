package agent

import (
	"path/filepath"
	"regexp"
	"strings"
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
