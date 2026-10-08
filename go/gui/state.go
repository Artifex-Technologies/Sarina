package gui

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
)

// apiErr is a request failure with its HTTP status.
type apiErr struct {
	code int
	msg  string
}

func (e *apiErr) Error() string { return e.msg }

func bad(format string, a ...any) *apiErr {
	return &apiErr{400, fmt.Sprintf(format, a...)}
}

func wrongType(key, want string) *apiErr {
	return &apiErr{422, fmt.Sprintf("%s: expected %s", key, want)}
}

// Approval modes. "ask" puts every state-changing tool call to the operator;
// "auto" runs whatever the permission toggles allow without asking.
const (
	ApprovalAsk  = "ask"
	ApprovalAuto = "auto"
)

// State is the operator's settings. Changing it never touches a run in
// flight: the next run is built from the state as it is then.
type State struct {
	Model, BaseURL, APIKey string
	CWD                    string
	SessionDir             string
	sessionDirExplicit     bool
	AllowShell, AllowWrite bool
	ApprovalMode           string
	Temperature            float64
	TimeoutSeconds         float64
	Stream                 bool
	MaxTurns               int

	MaxTotalTokens, MaxInputTokens, MaxOutputTokens, MaxReasoningTokens *int
	MaxToolCalls, MaxDelegatedTasks, MaxModelCalls, MaxSessionTurns     *int
	MaxTotalCostUSD                                                     *float64

	SystemPrompt, AppendSystemPrompt, OverrideSystemPrompt string

	ResponseSchema       map[string]any
	ResponseSchemaName   string
	ResponseSchemaStrict bool

	AutoSnip, AutoCompact *int
	CompactPreserve       int
	DisableClaudeMd       bool
	ExtraDirs             []string

	ActiveSessionID string
}

// DefaultState is what the flags start from.
func DefaultState(getenv func(string) string, cwd string) State {
	m := config.ModelFromEnv(getenv)
	rt := config.DefaultRuntime(cwd)
	return State{
		Model: m.Model, BaseURL: m.BaseURL, APIKey: m.APIKey,
		CWD: rt.CWD, SessionDir: rt.SessionDirectory,
		ApprovalMode: ApprovalAsk, TimeoutSeconds: config.DefaultTimeoutSeconds,
		Stream: true, MaxTurns: 12, CompactPreserve: 4,
		ResponseSchemaName: "response", ExtraDirs: []string{},
	}
}

// PermissionMode names what the agent may do, for the persistent indicator.
func (st State) PermissionMode() string {
	switch {
	case st.AllowShell && st.AllowWrite:
		return "write+shell"
	case st.AllowShell:
		return "shell"
	case st.AllowWrite:
		return "write"
	}
	return "read-only"
}

func nilIfEmpty(s string) any {
	if s == "" {
		return nil
	}
	return s
}

func ptrVal[T any](p *T) any {
	if p == nil {
		return nil
	}
	return *p
}

// Snapshot is the GET /api/state body. It never includes the API key.
func (st State) Snapshot() map[string]any {
	var schema any
	if st.ResponseSchema != nil {
		schema = st.ResponseSchema
	}
	var active any
	if st.ActiveSessionID != "" {
		active = st.ActiveSessionID
	}
	extra := st.ExtraDirs
	if extra == nil {
		extra = []string{}
	}
	return map[string]any{
		"model": st.Model, "base_url": st.BaseURL, "api_key_set": st.APIKey != "",
		"cwd": st.CWD, "session_directory": st.SessionDir,
		"allow_shell": st.AllowShell, "allow_write": st.AllowWrite,
		"approval_mode": st.ApprovalMode, "permission_mode": st.PermissionMode(),
		"temperature": st.Temperature, "timeout_seconds": st.TimeoutSeconds,
		"stream_model_responses": st.Stream, "max_turns": st.MaxTurns,
		"max_total_tokens": ptrVal(st.MaxTotalTokens), "max_input_tokens": ptrVal(st.MaxInputTokens),
		"max_output_tokens": ptrVal(st.MaxOutputTokens), "max_reasoning_tokens": ptrVal(st.MaxReasoningTokens),
		"max_tool_calls": ptrVal(st.MaxToolCalls), "max_delegated_tasks": ptrVal(st.MaxDelegatedTasks),
		"max_model_calls": ptrVal(st.MaxModelCalls), "max_session_turns": ptrVal(st.MaxSessionTurns),
		"max_total_cost_usd": ptrVal(st.MaxTotalCostUSD),
		"system_prompt":      nilIfEmpty(st.SystemPrompt), "append_system_prompt": nilIfEmpty(st.AppendSystemPrompt),
		"override_system_prompt": nilIfEmpty(st.OverrideSystemPrompt),
		"response_schema":        schema, "response_schema_name": st.ResponseSchemaName,
		"response_schema_strict":         st.ResponseSchemaStrict,
		"auto_snip_threshold_tokens":     ptrVal(st.AutoSnip),
		"auto_compact_threshold_tokens":  ptrVal(st.AutoCompact),
		"compact_preserve_messages":      st.CompactPreserve,
		"disable_claude_md_discovery":    st.DisableClaudeMd,
		"additional_working_directories": extra,
		"active_session_id":              active,
	}
}

// ModelRuntime renders the state as the agent's configuration.
func (st State) ModelRuntime() (config.ModelConfig, config.RuntimeConfig) {
	m := config.ModelConfig{Model: st.Model, BaseURL: st.BaseURL, APIKey: st.APIKey,
		Temperature: st.Temperature, TimeoutSeconds: st.TimeoutSeconds}
	rt := config.DefaultRuntime(st.CWD)
	rt.SessionDirectory = st.SessionDir
	rt.MaxTurns = st.MaxTurns
	rt.StreamModelResponses = st.Stream
	rt.AutoSnipThresholdTokens, rt.AutoCompactThresholdTokens = st.AutoSnip, st.AutoCompact
	rt.CompactPreserveMessages = st.CompactPreserve
	rt.Permissions.AllowShellCommands, rt.Permissions.AllowFileWrite = st.AllowShell, st.AllowWrite
	rt.AdditionalWorkingDirectories = append([]string{}, st.ExtraDirs...)
	rt.DisableClaudeMdDiscovery = st.DisableClaudeMd
	b := &rt.BudgetConfig
	b.MaxTotalTokens, b.MaxInputTokens, b.MaxOutputTokens = st.MaxTotalTokens, st.MaxInputTokens, st.MaxOutputTokens
	b.MaxReasoningTokens, b.MaxToolCalls, b.MaxDelegatedTasks = st.MaxReasoningTokens, st.MaxToolCalls, st.MaxDelegatedTasks
	b.MaxModelCalls, b.MaxSessionTurns, b.MaxTotalCostUSD = st.MaxModelCalls, st.MaxSessionTurns, st.MaxTotalCostUSD
	if st.ResponseSchema != nil {
		rt.OutputSchema = &config.OutputSchema{Name: st.ResponseSchemaName, Schema: st.ResponseSchema, Strict: st.ResponseSchemaStrict}
	}
	return m, rt
}

// PromptOptions are the three system-prompt fields.
func (st State) PromptOptions() agent.PromptOptions {
	return agent.PromptOptions{System: st.SystemPrompt, Append: st.AppendSystemPrompt, Override: st.OverrideSystemPrompt}
}

// ---- patching ----------------------------------------------------------------------

func asString(key string, raw json.RawMessage) (string, *apiErr) {
	var s string
	if json.Unmarshal(raw, &s) != nil {
		return "", wrongType(key, "a string")
	}
	return s, nil
}

func isNull(raw json.RawMessage) bool { return strings.TrimSpace(string(raw)) == "null" }

func asBool(key string, raw json.RawMessage) (bool, *apiErr) {
	var b bool
	if json.Unmarshal(raw, &b) != nil {
		return false, wrongType(key, "a boolean")
	}
	return b, nil
}

func asFloat(key string, raw json.RawMessage) (float64, *apiErr) {
	var f float64
	if json.Unmarshal(raw, &f) != nil {
		return 0, wrongType(key, "a number")
	}
	return f, nil
}

// optPositiveInt reads "null or a positive whole number".
func optPositiveInt(key string, raw json.RawMessage) (*int, *apiErr) {
	if isNull(raw) {
		return nil, nil
	}
	f, e := asFloat(key, raw)
	if e != nil {
		return nil, e
	}
	if f <= 0 || f != math.Trunc(f) || f > 1e12 {
		return nil, bad("%s must be a positive whole number or null", key)
	}
	n := int(f)
	return &n, nil
}

func absDir(p string) (string, *apiErr) {
	p = strings.TrimSpace(p)
	if p == "" {
		return "", bad("directory must not be empty")
	}
	if p == "~" || strings.HasPrefix(p, "~/") {
		h, _ := os.UserHomeDir()
		p = filepath.Join(h, p[1:])
	}
	a, err := filepath.Abs(p)
	if err != nil {
		return "", bad("bad directory %q", p)
	}
	if st, err := os.Stat(a); err != nil || !st.IsDir() {
		return "", bad("%s does not exist or is not a directory", a)
	}
	if real, err := filepath.EvalSymlinks(a); err == nil {
		a = real
	}
	return a, nil
}

// apply merges a partial update. Omitted keys are unchanged; explicit null
// clears (budgets, prompts, schema, thresholds). It is all-or-nothing: on an
// error st is left as it was. grantsConfirmed says the caller confirmed
// widening permissions.
func (st *State) apply(patch map[string]json.RawMessage, confirmed bool) *apiErr {
	n := *st
	n.ExtraDirs = append([]string(nil), st.ExtraDirs...)
	for key, raw := range patch {
		var e *apiErr
		switch key {
		case "model", "base_url":
			var s string
			if s, e = asString(key, raw); e == nil {
				if strings.TrimSpace(s) == "" {
					e = bad("%s must not be empty", key)
				} else if key == "model" {
					n.Model = strings.TrimSpace(s)
				} else {
					n.BaseURL = strings.TrimSpace(s)
				}
			}
		case "api_key":
			if isNull(raw) {
				break
			}
			var s string
			if s, e = asString(key, raw); e == nil && s != "" {
				n.APIKey = s
			}
		case "cwd":
			var s string
			if s, e = asString(key, raw); e == nil {
				var a string
				if a, e = absDir(s); e == nil {
					n.CWD = a
					if !n.sessionDirExplicit {
						n.SessionDir = config.DefaultRuntime(a).SessionDirectory
					}
				}
			}
		case "session_directory":
			var s string
			if !isNull(raw) {
				if s, e = asString(key, raw); e != nil {
					break
				}
			}
			if strings.TrimSpace(s) == "" {
				n.sessionDirExplicit = false
				n.SessionDir = config.DefaultRuntime(n.CWD).SessionDirectory
			} else {
				a, err := filepath.Abs(strings.TrimSpace(s))
				if err != nil {
					e = bad("bad session directory")
					break
				}
				n.sessionDirExplicit, n.SessionDir = true, a
			}
		case "allow_shell":
			n.AllowShell, e = asBool(key, raw)
		case "allow_write":
			n.AllowWrite, e = asBool(key, raw)
		case "approval_mode":
			var s string
			if s, e = asString(key, raw); e == nil {
				if s != ApprovalAsk && s != ApprovalAuto {
					e = bad("approval_mode must be %q or %q", ApprovalAsk, ApprovalAuto)
				}
				n.ApprovalMode = s
			}
		case "temperature":
			var f float64
			if f, e = asFloat(key, raw); e == nil {
				if f < 0 {
					e = bad("temperature must be >= 0")
				}
				n.Temperature = f
			}
		case "timeout_seconds":
			var f float64
			if f, e = asFloat(key, raw); e == nil {
				if f <= 0 {
					e = bad("timeout_seconds must be > 0")
				}
				n.TimeoutSeconds = f
			}
		case "max_turns":
			var f float64
			if f, e = asFloat(key, raw); e == nil {
				if f < 1 || f != math.Trunc(f) || f > 1e6 {
					e = bad("max_turns must be a whole number >= 1")
				}
				n.MaxTurns = int(f)
			}
		case "stream_model_responses":
			n.Stream, e = asBool(key, raw)
		case "max_total_tokens":
			n.MaxTotalTokens, e = optPositiveInt(key, raw)
		case "max_input_tokens":
			n.MaxInputTokens, e = optPositiveInt(key, raw)
		case "max_output_tokens":
			n.MaxOutputTokens, e = optPositiveInt(key, raw)
		case "max_reasoning_tokens":
			n.MaxReasoningTokens, e = optPositiveInt(key, raw)
		case "max_tool_calls":
			n.MaxToolCalls, e = optPositiveInt(key, raw)
		case "max_delegated_tasks":
			n.MaxDelegatedTasks, e = optPositiveInt(key, raw)
		case "max_model_calls":
			n.MaxModelCalls, e = optPositiveInt(key, raw)
		case "max_session_turns":
			n.MaxSessionTurns, e = optPositiveInt(key, raw)
		case "auto_snip_threshold_tokens":
			n.AutoSnip, e = optPositiveInt(key, raw)
		case "auto_compact_threshold_tokens":
			n.AutoCompact, e = optPositiveInt(key, raw)
		case "max_total_cost_usd":
			if isNull(raw) {
				n.MaxTotalCostUSD = nil
				break
			}
			var f float64
			if f, e = asFloat(key, raw); e == nil {
				if f <= 0 {
					e = bad("max_total_cost_usd must be positive or null")
				}
				n.MaxTotalCostUSD = &f
			}
		case "system_prompt", "append_system_prompt", "override_system_prompt":
			var s string
			if !isNull(raw) {
				if s, e = asString(key, raw); e != nil {
					break
				}
			}
			switch key {
			case "system_prompt":
				n.SystemPrompt = s
			case "append_system_prompt":
				n.AppendSystemPrompt = s
			default:
				n.OverrideSystemPrompt = s
			}
		case "response_schema":
			if isNull(raw) {
				n.ResponseSchema = nil
				break
			}
			var m map[string]any
			if json.Unmarshal(raw, &m) != nil || m == nil {
				e = bad("response_schema must be a JSON object or null")
			} else {
				n.ResponseSchema = m
			}
		case "response_schema_name":
			var s string
			if s, e = asString(key, raw); e == nil {
				if strings.TrimSpace(s) == "" {
					e = bad("response_schema_name must not be empty")
				}
				n.ResponseSchemaName = strings.TrimSpace(s)
			}
		case "response_schema_strict":
			n.ResponseSchemaStrict, e = asBool(key, raw)
		case "compact_preserve_messages":
			var f float64
			if f, e = asFloat(key, raw); e == nil {
				if f < 0 || f != math.Trunc(f) {
					e = bad("compact_preserve_messages must be a whole number >= 0")
				}
				n.CompactPreserve = int(f)
			}
		case "disable_claude_md_discovery":
			n.DisableClaudeMd, e = asBool(key, raw)
		case "additional_working_directories":
			var list []string
			if !isNull(raw) {
				if json.Unmarshal(raw, &list) != nil {
					e = wrongType(key, "a list of strings")
					break
				}
			}
			n.ExtraDirs = []string{}
			for _, d := range list {
				if strings.TrimSpace(d) == "" {
					continue
				}
				var a string
				if a, e = absDir(d); e != nil {
					break
				}
				n.ExtraDirs = append(n.ExtraDirs, a)
			}
		}
		if e != nil {
			return e
		}
	}
	widened := (n.AllowShell && !st.AllowShell) || (n.AllowWrite && !st.AllowWrite) ||
		(n.ApprovalMode == ApprovalAuto && st.ApprovalMode != ApprovalAuto && (n.AllowShell || n.AllowWrite))
	if widened && !confirmed {
		return bad("turning on shell or write access (or switching to auto-approve) needs an explicit confirmation: send confirm_permissions=true")
	}
	*st = n
	return nil
}
