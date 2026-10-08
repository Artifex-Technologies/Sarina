package tiers

import (
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// EscalationOptions are the parts of the original escalation setup that come
// from the command line / permissions rather than the config file.
type EscalationOptions struct {
	// Enabled is the original --escalate-to-claude-code flag; it is OR-ed with
	// escalation_config.enabled.
	Enabled bool
	// CLIPath is the original --claude-code-cli-path ("" = config value, then
	// CLAUDE_CODE_CLI_PATH, then "claude").
	CLIPath string
	// Triggers is the original repeatable --escalation-trigger, added to the config's list.
	Triggers []string
	// DisableResourceCheck is the original --disable-resource-check.
	DisableResourceCheck bool
	// AcceptEdits: the local agent was allowed to write files or run shell
	// commands, so the CLI gets --permission-mode acceptEdits. Never a bypass mode.
	AcceptEdits bool
	ExtraDirs   []string // additional working directories (--add-dir)
}

// FromEscalation reconciles the original escalation_config / flags / env with
// the tier config so existing setups keep working. When escalation is not
// enabled (it is off by default) cfg is returned unchanged. When it is, a
// claude_cli tier named "claude-cli" is appended (unless tiers.json already
// declares a claude_cli tier, which then wins) and the resource thresholds from
// escalation_config.resource_thresholds replace the tier defaults.
func FromEscalation(cfg Config, esc config.EscalationConfig, o EscalationOptions, getenv func(string) string) Config {
	if !(esc.Enabled || o.Enabled) {
		return cfg
	}
	for _, t := range cfg.Tiers {
		if t.Kind == KindClaudeCLI {
			return cfg
		}
	}
	out := cfg
	out.Tiers = append([]Tier{}, cfg.Tiers...)
	out.Warnings = append([]string{}, cfg.Warnings...)

	path := strings.TrimSpace(o.CLIPath)
	if path == "" {
		path = strings.TrimSpace(esc.CLIPath)
	}
	if (path == "" || path == defaultCLIPath) && getenv != nil {
		if v := strings.TrimSpace(getenv("CLAUDE_CODE_CLI_PATH")); v != "" {
			path = v
		}
	}
	if path == "" {
		path = defaultCLIPath
	}
	max := esc.MaxEscalations
	if max <= 0 {
		max = defaultMaxEscalation
	}
	timeout := esc.TimeoutSeconds
	if timeout <= 0 {
		timeout = defaultCLITimeout
	}
	var triggers []string
	for _, tr := range append(append([]string{}, esc.TriggerStopReasons...), o.Triggers...) {
		switch tr {
		case TriggerBackendError, TriggerResourceExhausted:
			triggers = append(triggers, tr)
		default:
			out.Warnings = append(out.Warnings, "escalation trigger "+tr+" is decided by the agent loop, not the tier router, and is ignored here")
		}
	}
	// The original defaulted to {backend_error, resource_exhausted}; an empty
	// config list means "unset" there too.
	if len(esc.TriggerStopReasons) == 0 && len(triggers) == 0 {
		triggers = []string{TriggerBackendError, TriggerResourceExhausted}
	}
	out.Tiers = append(out.Tiers, Tier{Name: "claude-cli", Kind: KindClaudeCLI, Enabled: true, Auto: true,
		AllowData: AllowData{Classes: []string{ClassNoSecrets, ClassNoCaptures}},
		CLIPath:   path, TimeoutSeconds: timeout, MaxEscalations: max, Triggers: triggers,
		ExtraArgs: append([]string{}, esc.ExtraCLIArgs...), ExtraDirs: o.ExtraDirs, AcceptEdits: o.AcceptEdits})

	rt := esc.ResourceThresholds
	if rt.MaxMemoryPercent != nil {
		out.Resource.MaxMemoryPercent = *rt.MaxMemoryPercent
	}
	if rt.MaxCPUPercent != nil {
		out.Resource.MaxCPUPercent = *rt.MaxCPUPercent
	}
	if rt.MaxGPUMemoryPercent != nil {
		out.Resource.MaxGPUMemoryPercent = *rt.MaxGPUMemoryPercent
	}
	if o.DisableResourceCheck {
		out.Resource = Thresholds{}
	}
	return out
}
