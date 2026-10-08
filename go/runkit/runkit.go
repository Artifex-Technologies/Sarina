// Package runkit is what the HTTP front ends (service and gui) share: building
// an agent from plain settings, serialising a run result the same way for both,
// and scrubbing secrets from anything that leaves the process.
package runkit

import (
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/agents"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/policy"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// Spec is everything needed to build one agent.
type Spec struct {
	Model     config.ModelConfig
	Runtime   config.RuntimeConfig
	Allowlist []string // nil = all tools; non-nil (even empty) = exactly those
	Prompt    agent.PromptOptions
	Home      string              // "" = the user's real home
	Getenv    func(string) string // nil = os.Getenv
	// NewClient builds the model transport; nil uses the HTTP client.
	NewClient func(config.ModelConfig) agent.Completer
	// Approver, when set, is asked before each state-changing tool call.
	Approver tools.Approver
	// Discover lets the agent ask the backend for its context window. Tests
	// that count backend requests leave it off.
	Discover bool
	// UseTiers consults ~/.sarina/tiers.json (compute tiers). Off for the
	// service, which is a fixed-backend integration surface.
	UseTiers bool
	// Escalation, when non-nil, adds the claude_cli tier (operator flags only,
	// never request data). It works with or without UseTiers.
	Escalation *tiers.EscalationOptions
	// Subagents gives the model the read-only `agent` tool.
	Subagents bool
}

// Built is an agent plus what was learned while building it.
type Built struct {
	Agent   *agent.Agent
	Router  *tiers.Router // nil unless more than one tier is configured
	Notices []string      // lines the operator should see
}

// Build creates an agent. The allowlist is part of the agent's tool builder,
// so it survives every internal rebuild of the registry.
func Build(s Spec) (*Built, error) {
	getenv := s.Getenv
	if getenv == nil {
		getenv = func(string) string { return "" }
	}
	b := &Built{}
	m, rt := s.Model, s.Runtime
	var client agent.Completer
	if s.NewClient != nil {
		client = s.NewClient(m)
	} else {
		client = llm.NewClient(m)
	}
	var tracker agent.TierTracker
	if s.UseTiers || s.Escalation != nil {
		cfg, err := tiers.Default(), error(nil)
		if s.UseTiers {
			cfg, err = tiers.Load(s.Home, getenv)
		}
		if err != nil {
			b.Notices = append(b.Notices, "ignoring the tiers config, staying on the local backend only: "+err.Error())
		} else {
			if s.Escalation != nil {
				o := *s.Escalation
				o.AcceptEdits = rt.Permissions.AllowFileWrite || rt.Permissions.AllowShellCommands
				o.ExtraDirs = rt.AdditionalWorkingDirectories
				// only the operator's flags enable this; a session file or request cannot
				cfg = tiers.FromEscalation(cfg, config.EscalationConfig{}, o, getenv)
			}
			for _, w := range cfg.Warnings {
				b.Notices = append(b.Notices, "tier warning: "+w)
			}
			if cfg.Multi() {
				if r, err := tiers.New(tiers.Options{Config: cfg, Base: m, BaseClient: client, Getenv: getenv,
					Workdir: rt.CWD, Wake: tiers.NewWake(rt.Permissions, rt.CWD, nil)}); err == nil {
					b.Router, client, tracker = r, r, r
				}
			}
		}
	}
	pol := policy.Load(policy.Options{Home: s.Home, Getenv: getenv, Workspace: rt.CWD})
	if len(pol.Fatal) > 0 {
		return nil, errors.New(strings.Join(pol.Fatal, "; "))
	}
	b.Notices = append(b.Notices, pol.Notices...)
	pol.ApplyBudget(&rt.BudgetConfig)
	var sessionEnv map[string]string
	for _, name := range pol.SafeEnv {
		if v := getenv(name); v != "" {
			if sessionEnv == nil {
				sessionEnv = map[string]string{}
			}
			sessionEnv[name] = v
		}
	}
	var discoverer agent.WindowDiscoverer
	if s.Discover {
		discoverer = llm.DefaultDiscoverer
	}
	var profiles *agents.Catalog
	if s.Subagents {
		profiles = agents.Load(agents.Options{Home: s.Home, Getenv: getenv, Trusted: pol.Trusted})
	}
	a, err := agent.New(agent.Options{
		Model: m, Runtime: rt, Client: client, Discoverer: discoverer,
		Tools:      tools.Builder{Allowlist: s.Allowlist, Deny: pol.Denied},
		Prompt:     s.Prompt,
		Getenv:     getenv,
		Home:       s.Home,
		Tiers:      tracker,
		Subagents:  profiles,
		SessionEnv: sessionEnv,
		Approver:   s.Approver,
	})
	if err != nil {
		return nil, err
	}
	b.Agent = a
	return b, nil
}

// ---- result serialisation ------------------------------------------------------

// Entry renders one transcript message in the wire shape both servers use:
// role and content always; name, tool_call_id, tool_calls, metadata and
// message_id only when non-empty. Tool entries carry an "ok" boolean in their
// metadata (read from the tool's own result body). scrub is applied to every
// string.
func Entry(m session.Message, scrub func(string) string) map[string]any {
	if scrub == nil {
		scrub = func(s string) string { return s }
	}
	e := map[string]any{"role": m.Role, "content": scrub(m.Content)}
	if m.Name != "" {
		e["name"] = m.Name
	}
	if m.ToolCallID != "" {
		e["tool_call_id"] = m.ToolCallID
	}
	if len(m.ToolCalls) > 0 {
		calls := make([]map[string]any, 0, len(m.ToolCalls))
		for _, c := range m.ToolCalls {
			calls = append(calls, map[string]any{"id": c.ID, "type": "function",
				"function": map[string]any{"name": c.Function.Name, "arguments": scrub(c.Function.Arguments)}})
		}
		e["tool_calls"] = calls
	}
	md := map[string]any{}
	for k, v := range m.Metadata {
		md[k] = v
	}
	if m.Role == "tool" {
		var body struct {
			OK      *bool  `json:"ok"`
			Content string `json:"content"`
		}
		if json.Unmarshal([]byte(m.Content), &body) == nil && body.OK != nil {
			md["ok"] = *body.OK
		} else if _, has := md["ok"]; !has {
			md["ok"] = false
		}
	}
	if len(md) > 0 {
		e["metadata"] = scrubValue(md, scrub)
	}
	if m.MessageID != "" {
		e["message_id"] = m.MessageID
	}
	return e
}

// Transcript renders every message with Entry.
func Transcript(msgs []session.Message, scrub func(string) string) []map[string]any {
	out := make([]map[string]any, 0, len(msgs))
	for _, m := range msgs {
		out = append(out, Entry(m, scrub))
	}
	return out
}

// Result renders a run result (spec section 1.1). stop_reason is null when the
// backend gave none.
func Result(res *agent.Result, scrub func(string) string) map[string]any {
	if scrub == nil {
		scrub = func(s string) string { return s }
	}
	var stop any
	if res.StopReason != "" {
		stop = res.StopReason
	}
	return map[string]any{
		"final_output":   scrub(res.FinalOutput),
		"turns":          res.Turns,
		"tool_calls":     res.ToolCalls,
		"transcript":     Transcript(res.Transcript, scrub),
		"session_id":     res.SessionID,
		"usage":          res.Usage,
		"total_cost_usd": res.TotalCostUSD,
		"stop_reason":    stop,
	}
}

func scrubValue(v any, scrub func(string) string) any {
	switch t := v.(type) {
	case string:
		return scrub(t)
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, x := range t {
			out[k] = scrubValue(x, scrub)
		}
		return out
	case []any:
		out := make([]any, len(t))
		for i, x := range t {
			out[i] = scrubValue(x, scrub)
		}
		return out
	case []string:
		out := make([]any, len(t))
		for i, x := range t {
			out[i] = scrub(x)
		}
		return out
	}
	return v
}

// ScrubValue applies scrub to every string inside a JSON-like value.
func ScrubValue(v any, scrub func(string) string) any { return scrubValue(v, scrub) }

// ---- secrets -----------------------------------------------------------------------

var (
	bearerRe = regexp.MustCompile(`(?i)\b(bearer)\s+[A-Za-z0-9._~+/=\-]{12,}`)
	assignRe = regexp.MustCompile(`(?i)\b([A-Za-z0-9_]*(?:api[_-]?key|secret|token|passw(?:or)?d)[A-Za-z0-9_]*)(\s*[=:]\s*)("[^"\s]{6,}"|'[^'\s]{6,}'|[^\s"',;]{6,})`)
)

// Scrubber returns a function that removes secrets from text: the literal
// values given (API keys and tokens this process holds), the credential shapes
// the session redactor knows, bearer tokens and key=value assignments.
func Scrubber(literals ...string) func(string) string {
	var lits []string
	for _, l := range literals {
		if len(l) >= 6 {
			lits = append(lits, l)
		}
	}
	return func(s string) string {
		for _, l := range lits {
			s = strings.ReplaceAll(s, l, "[REDACTED]")
		}
		s = session.Redact(s)
		s = bearerRe.ReplaceAllString(s, "$1 [REDACTED]")
		s = assignRe.ReplaceAllString(s, "$1$2[REDACTED]")
		return s
	}
}

// ErrorType names an error's concrete type the way the 500 body reports it.
func ErrorType(err error) string {
	t := fmt.Sprintf("%T", err)
	t = strings.TrimPrefix(t, "*")
	if i := strings.LastIndex(t, "."); i >= 0 {
		t = t[i+1:]
	}
	return t
}
