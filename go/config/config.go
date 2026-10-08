// Package config holds Sarina's model and runtime configuration, the
// environment handling that feeds it, and tolerant loaders used when a saved
// session file is read back.
package config

import (
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// Defaults for the backend connection.
const (
	DefaultModel          = "Qwen/Qwen3-Coder-30B-A3B-Instruct"
	DefaultBaseURL        = "http://127.0.0.1:8000/v1"
	DefaultAPIKey         = "local-token"
	DefaultTimeoutSeconds = 120.0
)

// Pricing is expressed in USD per million tokens.
type Pricing struct {
	InputPerMillion         float64 `json:"input_cost_per_million_tokens_usd"`
	OutputPerMillion        float64 `json:"output_cost_per_million_tokens_usd"`
	CacheCreationPerMillion float64 `json:"cache_creation_input_cost_per_million_tokens_usd"`
	CacheReadPerMillion     float64 `json:"cache_read_input_cost_per_million_tokens_usd"`
}

// ModelConfig describes the OpenAI-compatible backend. The API key is never
// serialised: it is excluded from every JSON form on purpose.
type ModelConfig struct {
	Model          string  `json:"model"`
	BaseURL        string  `json:"base_url"`
	APIKey         string  `json:"-"`
	Temperature    float64 `json:"temperature"`
	TimeoutSeconds float64 `json:"timeout_seconds"`
	Pricing        Pricing `json:"pricing"`
}

// Permissions gate the side-effecting tools.
type Permissions struct {
	AllowFileWrite                bool `json:"allow_file_write"`
	AllowShellCommands            bool `json:"allow_shell_commands"`
	AllowDestructiveShellCommands bool `json:"allow_destructive_shell_commands"`
}

// BudgetConfig limits a session. A nil field means "unset".
type BudgetConfig struct {
	MaxTotalTokens     *int     `json:"max_total_tokens"`
	MaxInputTokens     *int     `json:"max_input_tokens"`
	MaxOutputTokens    *int     `json:"max_output_tokens"`
	MaxReasoningTokens *int     `json:"max_reasoning_tokens"`
	MaxTotalCostUSD    *float64 `json:"max_total_cost_usd"`
	MaxToolCalls       *int     `json:"max_tool_calls"`
	MaxDelegatedTasks  *int     `json:"max_delegated_tasks"`
	MaxModelCalls      *int     `json:"max_model_calls"`
	MaxSessionTurns    *int     `json:"max_session_turns"`
}

// ResourceThresholds are persisted for file-format compatibility.
type ResourceThresholds struct {
	MaxMemoryPercent    *float64 `json:"max_memory_percent"`
	MaxCPUPercent       *float64 `json:"max_cpu_percent"`
	MaxGPUMemoryPercent *float64 `json:"max_gpu_memory_percent"`
	MinDiskFreeMB       *float64 `json:"min_disk_free_mb"`
}

// EscalationConfig is persisted for file-format compatibility. Escalation
// itself is not implemented in the Go core yet.
type EscalationConfig struct {
	Enabled            bool               `json:"enabled"`
	CLIPath            string             `json:"cli_path"`
	TriggerStopReasons []string           `json:"trigger_stop_reasons"`
	MaxEscalations     int                `json:"max_escalations"`
	TimeoutSeconds     float64            `json:"timeout_seconds"`
	ResourceThresholds ResourceThresholds `json:"resource_thresholds"`
	ExtraCLIArgs       []string           `json:"extra_cli_args"`
}

// OutputSchema requests structured output from the backend.
type OutputSchema struct {
	Name   string         `json:"name"`
	Schema map[string]any `json:"schema"`
	Strict bool           `json:"strict"`
}

// RuntimeConfig controls the agent loop.
type RuntimeConfig struct {
	CWD                          string           `json:"cwd"`
	MaxTurns                     int              `json:"max_turns"`
	CommandTimeoutSeconds        float64          `json:"command_timeout_seconds"`
	MaxOutputChars               int              `json:"max_output_chars"`
	StreamModelResponses         bool             `json:"stream_model_responses"`
	AutoSnipThresholdTokens      *int             `json:"auto_snip_threshold_tokens"`
	AutoCompactThresholdTokens   *int             `json:"auto_compact_threshold_tokens"`
	CompactPreserveMessages      int              `json:"compact_preserve_messages"`
	Permissions                  Permissions      `json:"permissions"`
	AdditionalWorkingDirectories []string         `json:"additional_working_directories"`
	DisableClaudeMdDiscovery     bool             `json:"disable_claude_md_discovery"`
	BudgetConfig                 BudgetConfig     `json:"budget_config"`
	EscalationConfig             EscalationConfig `json:"escalation_config"`
	OutputSchema                 *OutputSchema    `json:"output_schema"`
	SessionDirectory             string           `json:"session_directory"`
	ScratchpadRoot               string           `json:"scratchpad_root"`
	ContextManagement            *ContextConfig   `json:"context_management,omitempty"`
}

// DefaultModel returns the model config built from the environment, using
// getenv (os.Getenv when nil).
func ModelFromEnv(getenv func(string) string) ModelConfig {
	if getenv == nil {
		getenv = os.Getenv
	}
	m := ModelConfig{
		Model:          DefaultModel,
		BaseURL:        DefaultBaseURL,
		APIKey:         DefaultAPIKey,
		Temperature:    0,
		TimeoutSeconds: DefaultTimeoutSeconds,
	}
	if v := getenv("OPENAI_MODEL"); v != "" {
		m.Model = v
	}
	if v := getenv("OPENAI_BASE_URL"); v != "" {
		m.BaseURL = v
	}
	if v := getenv("OPENAI_API_KEY"); v != "" {
		m.APIKey = v
	}
	return m
}

// ContextWindowFromEnv reads SARINA_CONTEXT_WINDOW; zero means "not set".
func ContextWindowFromEnv(getenv func(string) string) int {
	if getenv == nil {
		getenv = os.Getenv
	}
	n, err := strconv.Atoi(strings.TrimSpace(getenv("SARINA_CONTEXT_WINDOW")))
	if err != nil || n <= 0 {
		return 0
	}
	return n
}

// DefaultRuntime returns the default runtime configuration for a workspace.
// .port_sessions is anchored at the workspace, not at the process directory.
func DefaultRuntime(cwd string) RuntimeConfig {
	abs := cwd
	if a, err := filepath.Abs(cwd); err == nil {
		abs = a
	}
	return RuntimeConfig{
		CWD:                          abs,
		MaxTurns:                     12,
		CommandTimeoutSeconds:        30,
		MaxOutputChars:               12000,
		CompactPreserveMessages:      4,
		AdditionalWorkingDirectories: []string{},
		EscalationConfig:             defaultEscalation(),
		SessionDirectory:             filepath.Join(abs, ".port_sessions", "agent"),
		ScratchpadRoot:               filepath.Join(abs, ".port_sessions", "scratchpad"),
	}
}

func defaultEscalation() EscalationConfig {
	return EscalationConfig{
		CLIPath:            "claude",
		TriggerStopReasons: []string{"backend_error", "resource_exhausted"},
		MaxEscalations:     1,
		TimeoutSeconds:     300,
		ExtraCLIArgs:       []string{},
	}
}

// Normalized returns a copy whose slice fields are non-nil so they serialise
// as [] rather than null.
func (r RuntimeConfig) Normalized() RuntimeConfig {
	if r.AdditionalWorkingDirectories == nil {
		r.AdditionalWorkingDirectories = []string{}
	}
	if r.EscalationConfig.TriggerStopReasons == nil {
		r.EscalationConfig.TriggerStopReasons = []string{}
	}
	if r.EscalationConfig.ExtraCLIArgs == nil {
		r.EscalationConfig.ExtraCLIArgs = []string{}
	}
	return r
}

// ---- tolerant loaders -----------------------------------------------------

// Coercion helpers. Booleans are never accepted as numbers; numeric strings are.

// AsInt coerces v to an int.
func AsInt(v any) (int, bool) {
	switch t := v.(type) {
	case float64:
		return int(t), true
	case int:
		return t, true
	case int64:
		return int(t), true
	case string:
		s := strings.TrimSpace(t)
		if n, err := strconv.Atoi(s); err == nil {
			return n, true
		}
		if f, err := strconv.ParseFloat(s, 64); err == nil {
			return int(f), true
		}
	}
	return 0, false
}

// AsFloat coerces v to a float64.
func AsFloat(v any) (float64, bool) {
	switch t := v.(type) {
	case float64:
		return t, true
	case int:
		return float64(t), true
	case int64:
		return float64(t), true
	case string:
		if f, err := strconv.ParseFloat(strings.TrimSpace(t), 64); err == nil {
			return f, true
		}
	}
	return 0, false
}

// AsMap returns v as an object or an empty map.
func AsMap(v any) map[string]any {
	if m, ok := v.(map[string]any); ok {
		return m
	}
	return map[string]any{}
}

func intOr(m map[string]any, key string, def int) int {
	if n, ok := AsInt(m[key]); ok {
		return n
	}
	return def
}

func floatOr(m map[string]any, key string, def float64) float64 {
	if f, ok := AsFloat(m[key]); ok {
		return f
	}
	return def
}

func boolOr(m map[string]any, key string, def bool) bool {
	if b, ok := m[key].(bool); ok {
		return b
	}
	return def
}

func strOr(m map[string]any, key, def string) string {
	if s, ok := m[key].(string); ok {
		return s
	}
	return def
}

func intPtr(m map[string]any, key string) *int {
	if n, ok := AsInt(m[key]); ok {
		return &n
	}
	return nil
}

func floatPtr(m map[string]any, key string) *float64 {
	if f, ok := AsFloat(m[key]); ok {
		return &f
	}
	return nil
}

func strList(v any, def []string) []string {
	list, ok := v.([]any)
	if !ok {
		return def
	}
	out := make([]string, 0, len(list))
	for _, item := range list {
		if s, ok := item.(string); ok {
			out = append(out, s)
		}
	}
	return out
}

// ModelFromMap loads a ModelConfig from a saved file's object. "model" is
// required. The API key is not read from the file even if present.
func ModelFromMap(m map[string]any) (ModelConfig, bool) {
	model, ok := m["model"].(string)
	if !ok || model == "" {
		return ModelConfig{}, false
	}
	p := AsMap(m["pricing"])
	return ModelConfig{
		Model:          model,
		BaseURL:        strOr(m, "base_url", DefaultBaseURL),
		Temperature:    floatOr(m, "temperature", 0),
		TimeoutSeconds: floatOr(m, "timeout_seconds", DefaultTimeoutSeconds),
		Pricing: Pricing{
			InputPerMillion:         floatOr(p, "input_cost_per_million_tokens_usd", 0),
			OutputPerMillion:        floatOr(p, "output_cost_per_million_tokens_usd", 0),
			CacheCreationPerMillion: floatOr(p, "cache_creation_input_cost_per_million_tokens_usd", 0),
			CacheReadPerMillion:     floatOr(p, "cache_read_input_cost_per_million_tokens_usd", 0),
		},
	}, true
}

// RuntimeFromMap loads a RuntimeConfig from a saved file's object. "cwd" is
// required; everything else falls back to defaults.
func RuntimeFromMap(m map[string]any) (RuntimeConfig, bool) {
	cwd, ok := m["cwd"].(string)
	if !ok || cwd == "" {
		return RuntimeConfig{}, false
	}
	r := DefaultRuntime(cwd)
	r.CWD = cwd
	r.MaxTurns = intOr(m, "max_turns", r.MaxTurns)
	r.CommandTimeoutSeconds = floatOr(m, "command_timeout_seconds", r.CommandTimeoutSeconds)
	r.MaxOutputChars = intOr(m, "max_output_chars", r.MaxOutputChars)
	r.StreamModelResponses = boolOr(m, "stream_model_responses", false)
	r.AutoSnipThresholdTokens = intPtr(m, "auto_snip_threshold_tokens")
	r.AutoCompactThresholdTokens = intPtr(m, "auto_compact_threshold_tokens")
	r.CompactPreserveMessages = intOr(m, "compact_preserve_messages", r.CompactPreserveMessages)
	pm := AsMap(m["permissions"])
	r.Permissions = Permissions{
		AllowFileWrite:                boolOr(pm, "allow_file_write", false),
		AllowShellCommands:            boolOr(pm, "allow_shell_commands", false),
		AllowDestructiveShellCommands: boolOr(pm, "allow_destructive_shell_commands", false),
	}
	r.AdditionalWorkingDirectories = strList(m["additional_working_directories"], []string{})
	r.DisableClaudeMdDiscovery = boolOr(m, "disable_claude_md_discovery", false)
	bm := AsMap(m["budget_config"])
	r.BudgetConfig = BudgetConfig{
		MaxTotalTokens:     intPtr(bm, "max_total_tokens"),
		MaxInputTokens:     intPtr(bm, "max_input_tokens"),
		MaxOutputTokens:    intPtr(bm, "max_output_tokens"),
		MaxReasoningTokens: intPtr(bm, "max_reasoning_tokens"),
		MaxTotalCostUSD:    floatPtr(bm, "max_total_cost_usd"),
		MaxToolCalls:       intPtr(bm, "max_tool_calls"),
		MaxDelegatedTasks:  intPtr(bm, "max_delegated_tasks"),
		MaxModelCalls:      intPtr(bm, "max_model_calls"),
		MaxSessionTurns:    intPtr(bm, "max_session_turns"),
	}
	em := AsMap(m["escalation_config"])
	rt := AsMap(em["resource_thresholds"])
	esc := defaultEscalation()
	esc.Enabled = boolOr(em, "enabled", false)
	esc.CLIPath = strOr(em, "cli_path", esc.CLIPath)
	esc.TriggerStopReasons = strList(em["trigger_stop_reasons"], esc.TriggerStopReasons)
	esc.MaxEscalations = intOr(em, "max_escalations", esc.MaxEscalations)
	esc.TimeoutSeconds = floatOr(em, "timeout_seconds", esc.TimeoutSeconds)
	esc.ExtraCLIArgs = strList(em["extra_cli_args"], esc.ExtraCLIArgs)
	esc.ResourceThresholds = ResourceThresholds{
		MaxMemoryPercent:    floatPtr(rt, "max_memory_percent"),
		MaxCPUPercent:       floatPtr(rt, "max_cpu_percent"),
		MaxGPUMemoryPercent: floatPtr(rt, "max_gpu_memory_percent"),
		MinDiskFreeMB:       floatPtr(rt, "min_disk_free_mb"),
	}
	r.EscalationConfig = esc
	if sm, ok := m["output_schema"].(map[string]any); ok {
		name, _ := sm["name"].(string)
		schema, isMap := sm["schema"].(map[string]any)
		if name != "" && isMap {
			r.OutputSchema = &OutputSchema{Name: name, Schema: schema, Strict: boolOr(sm, "strict", false)}
		}
	}
	r.SessionDirectory = strOr(m, "session_directory", r.SessionDirectory)
	if r.SessionDirectory == "" {
		r.SessionDirectory = filepath.Join(cwd, ".port_sessions", "agent")
	}
	r.ContextManagement = contextFromMap(AsMap(m["context_management"]))
	r.ScratchpadRoot = strOr(m, "scratchpad_root", r.ScratchpadRoot)
	if r.ScratchpadRoot == "" {
		r.ScratchpadRoot = filepath.Join(cwd, ".port_sessions", "scratchpad")
	}
	return r, true
}
