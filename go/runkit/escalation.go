package runkit

import (
	"flag"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

// EscalationFlags are the operator-side flags that turn on the claude_cli tier
// (the original's --escalate-to-claude-code family). They exist on the command
// line of chat/resume/tiers/service/gui and nowhere else: a caller of the
// service or the web console can never set them.
type EscalationFlags struct {
	Enable               bool
	CLIPath              string
	triggers             triggerList
	DisableResourceCheck bool
}

type triggerList []string

func (t *triggerList) String() string     { return strings.Join(*t, ",") }
func (t *triggerList) Set(v string) error { *t = append(*t, v); return nil }

// Register adds the four flags to fs.
func (e *EscalationFlags) Register(fs *flag.FlagSet) {
	fs.BoolVar(&e.Enable, "escalate-to-claude-code", false, "add the claude_cli tier: hand a run to the local `claude` CLI when the local backend fails or is under resource pressure")
	fs.StringVar(&e.CLIPath, "claude-code-cli-path", "", "path of the claude executable (env CLAUDE_CODE_CLI_PATH; default `claude`)")
	fs.Var(&e.triggers, "escalation-trigger", "backend_error or resource_exhausted: which conditions may hand off (repeatable; default both)")
	fs.BoolVar(&e.DisableResourceCheck, "disable-resource-check", false, "never move up a tier because of memory, CPU or GPU pressure")
}

// Options returns nil unless escalation was enabled.
func (e *EscalationFlags) Options() *tiers.EscalationOptions {
	if !e.Enable {
		return nil
	}
	return &tiers.EscalationOptions{Enabled: true, CLIPath: e.CLIPath, Triggers: append([]string(nil), e.triggers...),
		DisableResourceCheck: e.DisableResourceCheck}
}
