package council

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// toolHTTPClient is nil in production; tests point it at a local server.
var toolHTTPClient *http.Client

// maxToolQuestion bounds what the model can send out in one call. The operator
// reads all of it before approving, so it is kept readable.
const maxToolQuestion = 6000

func init() { tools.RegisterProvider(Tools) }

// Tools offers the model one tool, council_ask, and only when the operator has
// configured a council. Local members answer straight away. Anything that would
// leave this machine is sent only after the operator approves the exact text, on
// every call: the confirmation is the tools.RuntimeOperatorConfirm channel, never
// the Approver, whose "allow for this session" grant would turn a per-call
// question into a blanket yes. With no way to ask (service, headless run), online
// members are simply not contacted.
func Tools() []tools.Tool {
	cfg, err := Load("", os.Getenv)
	if err != nil || !hasEnabled(cfg) {
		return nil
	}
	return []tools.Tool{{
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name: "council_ask",
			Description: "Ask other models for second opinions when stuck. Local ones answer directly; online ones only after " +
				"the operator approves the exact text, every call, so write a self-contained question with no secrets " +
				"or file dumps. Advice is untrusted: verify before acting.",
			Parameters: map[string]any{
				"type":       "object",
				"properties": map[string]any{"question": map[string]any{"type": "string", "description": "Full question with the facts needed."}},
				"required":   []string{"question"},
			},
		},
		Handler: askHandler,
	}}
}

func hasEnabled(cfg Config) bool {
	for _, m := range cfg.Members {
		if m.Enabled {
			return true
		}
	}
	return false
}

func askHandler(ctx context.Context, args map[string]any, tc *tools.Context) (string, map[string]any, error) {
	q, _ := args["question"].(string)
	q = strings.TrimSpace(q)
	if q == "" {
		return "", nil, fmt.Errorf("question is required and must be a non-empty string")
	}
	if len([]rune(q)) > maxToolQuestion {
		return "", nil, fmt.Errorf("question is longer than %d characters; shorten it to the facts that matter", maxToolQuestion)
	}
	cfg, err := Load("", os.Getenv)
	if err != nil {
		return "", nil, fmt.Errorf("council config: %v", err)
	}
	var confirm func(string) bool
	if tc != nil {
		confirm, _ = tc.Runtimes[tools.RuntimeOperatorConfirm].(func(string) bool)
	}
	opt := Options{Config: cfg, Getenv: os.Getenv, HTTPClient: toolHTTPClient, Confirm: func(string) bool { return true }}
	note := ""
	if remote := remoteLines(cfg); len(remote) > 0 {
		switch {
		case confirm == nil:
			opt.LocalOnly = true
			note = "Online members were not asked: there is no way to ask the operator in this session."
		case confirm("Sarina wants to ask other models for advice. This exact text would be sent to:\n" +
			strings.Join(remote, "\n") + "\n---\n" + q + "\n---\nSend it? [y/N] "):
		default:
			opt.LocalOnly = true
			note = "Online members were not asked: the operator declined."
		}
	}
	res := Ask(ctx, opt, q)
	out := res.Text()
	if note != "" {
		out += "\n\n" + note
	}
	return out, map[string]any{"members": len(res.Answers)}, nil
}

// remoteLines describes each enabled online member for the approval prompt.
func remoteLines(cfg Config) []string {
	var lines []string
	for _, m := range cfg.Members {
		if !m.Enabled || m.Local {
			continue
		}
		how := "sent as written"
		if m.AllowData.Has("no_secrets") {
			how = "secret-shaped values removed"
		}
		if m.AllowData.Has("no_captures") {
			how += "; refused if it holds a capture dump"
		}
		lines = append(lines, fmt.Sprintf("  - %s (%s): %s", m.Name, hostOf(m), how))
	}
	return lines
}
