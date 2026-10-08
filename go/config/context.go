package config

import (
	"os"
	"strconv"
	"strings"
)

// Defaults for context management (session notes, tool-output clearing, LLM
// compaction). They apply whenever the matching ContextConfig field is zero.
const (
	DefaultNotesEveryTurns     = 6
	DefaultNotesWindowPercent  = 50
	DefaultClearWindowPercent  = 60
	DefaultClearKeepRecent     = 4
	DefaultClearMinTokens      = 500
	DefaultCompactBreakerLimit = 3
)

// ContextConfig tunes how long sessions are kept inside the context window.
// The zero value means "all defaults, everything on", which is why the switches
// are phrased as Disable*. It is stored in the session file under
// runtime_config.context_management only when something was changed, so a
// default session's file is unchanged.
type ContextConfig struct {
	DisableNotes         bool `json:"disable_notes,omitempty"`
	NotesEveryTurns      int  `json:"notes_every_turns,omitempty"`
	NotesWindowPercent   int  `json:"notes_window_percent,omitempty"`
	DisableClearing      bool `json:"disable_clearing,omitempty"`
	ClearWindowPercent   int  `json:"clear_window_percent,omitempty"`
	ClearKeepRecent      int  `json:"clear_keep_recent,omitempty"`
	ClearMinTokens       int  `json:"clear_min_tokens,omitempty"`
	DisableLLMCompaction bool `json:"disable_llm_compaction,omitempty"`
	DisableDiscovery     bool `json:"disable_context_discovery,omitempty"`
}

// ContextSettings is the resolved, ready-to-use form.
type ContextSettings struct {
	Notes               bool
	NotesEveryTurns     int
	NotesWindowFraction float64
	Clear               bool
	ClearWindowFraction float64
	ClearKeepRecent     int
	ClearMinTokens      int
	LLMCompaction       bool
	Discovery           bool
}

// envOff reports whether an environment switch is set to a "disabled" value.
func envOff(getenv func(string) string, name string) bool {
	switch strings.ToLower(strings.TrimSpace(getenv(name))) {
	case "0", "false", "off", "no":
		return true
	}
	return false
}

func envPositive(getenv func(string) string, name string) int {
	n, err := strconv.Atoi(strings.TrimSpace(getenv(name)))
	if err != nil || n <= 0 {
		return 0
	}
	return n
}

// Settings resolves defaults, the stored config and the environment (getenv,
// os.Getenv when nil). A feature is off if either the config or the environment
// turns it off; a numeric value comes from the config, then the environment,
// then the default. A nil receiver is the all-defaults config.
//
// Environment: SARINA_SESSION_NOTES=0, SARINA_NOTES_EVERY=<n>,
// SARINA_CLEAR_TOOL_OUTPUTS=0, SARINA_LLM_COMPACTION=0, SARINA_CONTEXT_DISCOVERY=0.
func (c *ContextConfig) Settings(getenv func(string) string) ContextSettings {
	if getenv == nil {
		getenv = os.Getenv
	}
	var v ContextConfig
	if c != nil {
		v = *c
	}
	pick := func(fromCfg int, env string, def int) int {
		if fromCfg > 0 {
			return fromCfg
		}
		if n := envPositive(getenv, env); n > 0 {
			return n
		}
		return def
	}
	pct := func(n int, def int) float64 {
		if n <= 0 || n > 100 {
			n = def
		}
		return float64(n) / 100
	}
	return ContextSettings{
		Notes:               !v.DisableNotes && !envOff(getenv, "SARINA_SESSION_NOTES"),
		NotesEveryTurns:     pick(v.NotesEveryTurns, "SARINA_NOTES_EVERY", DefaultNotesEveryTurns),
		NotesWindowFraction: pct(v.NotesWindowPercent, DefaultNotesWindowPercent),
		Clear:               !v.DisableClearing && !envOff(getenv, "SARINA_CLEAR_TOOL_OUTPUTS"),
		ClearWindowFraction: pct(v.ClearWindowPercent, DefaultClearWindowPercent),
		ClearKeepRecent:     pick(v.ClearKeepRecent, "", DefaultClearKeepRecent),
		ClearMinTokens:      pick(v.ClearMinTokens, "", DefaultClearMinTokens),
		LLMCompaction:       !v.DisableLLMCompaction && !envOff(getenv, "SARINA_LLM_COMPACTION"),
		Discovery:           !v.DisableDiscovery && !envOff(getenv, "SARINA_CONTEXT_DISCOVERY"),
	}
}

// IsZero reports whether nothing differs from the defaults.
func (c *ContextConfig) IsZero() bool { return c == nil || *c == ContextConfig{} }

func contextFromMap(m map[string]any) *ContextConfig {
	if len(m) == 0 {
		return nil
	}
	c := &ContextConfig{
		DisableNotes:         boolOr(m, "disable_notes", false),
		NotesEveryTurns:      intOr(m, "notes_every_turns", 0),
		NotesWindowPercent:   intOr(m, "notes_window_percent", 0),
		DisableClearing:      boolOr(m, "disable_clearing", false),
		ClearWindowPercent:   intOr(m, "clear_window_percent", 0),
		ClearKeepRecent:      intOr(m, "clear_keep_recent", 0),
		ClearMinTokens:       intOr(m, "clear_min_tokens", 0),
		DisableLLMCompaction: boolOr(m, "disable_llm_compaction", false),
		DisableDiscovery:     boolOr(m, "disable_context_discovery", false),
	}
	if c.IsZero() {
		return nil
	}
	return c
}
