package cli

import (
	"sort"
	"strings"
)

// SlashInfo describes one registered slash command (for palettes).
type SlashInfo struct {
	Name        string `json:"name"`
	Description string `json:"description"`
}

// SlashCommands lists the registered slash commands, sorted by name.
func SlashCommands() []SlashInfo {
	slashMu.Lock()
	defer slashMu.Unlock()
	out := make([]SlashInfo, 0, len(slashCmd))
	for n, e := range slashCmd {
		out = append(out, SlashInfo{Name: n, Description: e.desc})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

// LocalSlash answers input locally when it is a slash command that needs no
// agent or terminal context (today: /help). It reports handled=false for
// everything else, which then goes to the model like any prompt. Front ends
// without a terminal (the HTTP service and GUI) use it so that a trivial
// command works offline without letting a remote caller reach commands that
// are built around a live console.
func LocalSlash(input string) (string, bool) {
	input = strings.TrimSpace(input)
	if !strings.HasPrefix(input, "/") {
		return "", false
	}
	name, args, _ := strings.Cut(input[1:], " ")
	if strings.ToLower(name) != "help" {
		return "", false
	}
	r, handled := dispatchSlash(&SlashContext{}, "/help "+args)
	if !handled || r.Prompt != "" {
		return "", false
	}
	return r.Text, true
}
