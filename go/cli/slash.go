package cli

import (
	"fmt"
	"sort"
	"strings"
	"sync"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

// SlashContext is what a slash command can reach.
type SlashContext struct {
	Env     *Env
	Agent   *agent.Agent
	Current func() *session.Session // the live session, nil before the first turn
	Tiers   *tiers.Router           // compute tiers; nil when only the local backend is configured
	opts    *opts
}

// SlashResult is a slash command's outcome. With Prompt empty the command was
// answered locally (Text) and no model call happens; with Prompt set, that
// text goes through the normal model loop instead. Narrow, when non-nil,
// restricts the registry for that run (it can only narrow).
type SlashResult struct {
	Text   string
	Prompt string
	Narrow []string
}

// SlashFunc handles /name args.
type SlashFunc func(sc *SlashContext, args string) SlashResult

type slashEntry struct {
	desc string
	run  SlashFunc
}

var (
	slashMu  sync.Mutex
	slashCmd = map[string]slashEntry{}
)

// RegisterSlash adds (or replaces) a slash command; names are case-insensitive.
func RegisterSlash(name, description string, run SlashFunc) {
	slashMu.Lock()
	defer slashMu.Unlock()
	slashCmd[strings.ToLower(name)] = slashEntry{description, run}
}

func init() {
	RegisterSlash("help", "list the slash commands", func(*SlashContext, string) SlashResult {
		slashMu.Lock()
		names := make([]string, 0, len(slashCmd))
		for n := range slashCmd {
			names = append(names, n)
		}
		sort.Strings(names)
		var b strings.Builder
		b.WriteString("# Slash Commands\n")
		for _, n := range names {
			fmt.Fprintf(&b, "- /%s: %s\n", n, slashCmd[n].desc)
		}
		slashMu.Unlock()
		b.WriteString("These are handled locally; the model is not called for them.")
		return SlashResult{Text: b.String()}
	})
}

func plainName(n string) bool {
	for _, r := range n {
		if !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == ':' || r == '-' || r == '_') {
			return false
		}
	}
	return n != ""
}

// dispatchSlash intercepts input that starts with "/" before the model loop.
// It reports handled=false for anything that is an ordinary prompt: text not
// starting with "/" and "/path/like/this" (names with other characters).
func dispatchSlash(sc *SlashContext, input string) (SlashResult, bool) {
	input = strings.TrimSpace(input)
	if !strings.HasPrefix(input, "/") {
		return SlashResult{}, false
	}
	body := input[1:]
	name, args, _ := strings.Cut(body, " ")
	args = strings.TrimSpace(args)
	if name == "" {
		return SlashResult{Text: "Slash commands look like /command [args]. Try /help."}, true
	}
	slashMu.Lock()
	e, ok := slashCmd[strings.ToLower(name)]
	slashMu.Unlock()
	if ok {
		return e.run(sc, args), true
	}
	if plainName(name) {
		return SlashResult{Text: "Unknown skill: " + name}, true
	}
	return SlashResult{}, false
}
