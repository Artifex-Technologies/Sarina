package agent

import (
	"context"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/skills"
)

// WindowDiscoverer asks the backend for its context window (llm.Discoverer
// satisfies it). n == 0 means "not reported".
type WindowDiscoverer interface {
	ContextWindow(ctx context.Context, cfg config.ModelConfig) (n int, source string)
}

// resolveWindow picks the context window and says where it came from:
// an explicit option or SARINA_CONTEXT_WINDOW, then what the backend reports
// (when a discoverer is configured and discovery is on), then inference from
// the model name. Discovery results are cached by the discoverer.
func (a *Agent) resolveWindow() (int, string) {
	if a.opts.ContextWindow > 0 {
		return a.opts.ContextWindow, "flag"
	}
	if n := config.ContextWindowFromEnv(a.opts.Getenv); n > 0 {
		return n, "env"
	}
	if a.ctxSettings.Discovery && a.opts.Discoverer != nil {
		if n, src := a.opts.Discoverer.ContextWindow(context.Background(), a.opts.Model); n > 0 {
			return n, "backend:" + src
		}
	}
	return InferContextWindow(a.opts.Model.Model), "inferred"
}

// scanner is the gate session notes and summaries pass before re-injection.
func (a *Agent) scanner() skills.Scanner {
	if a.opts.Scanner != nil {
		return a.opts.Scanner
	}
	return skills.BasicScanner{}
}

// describeWindowSource turns a resolveWindow source tag into words for the operator.
func describeWindowSource(src string) string {
	switch {
	case src == "flag":
		return "set with --context-window"
	case src == "env":
		return "set with SARINA_CONTEXT_WINDOW"
	case strings.HasPrefix(src, "backend:"):
		return "reported by the backend (" + strings.TrimPrefix(src, "backend:") + ")"
	}
	return "inferred from the model name"
}
