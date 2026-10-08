package tiers

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// Defaults for the claude_cli kind, taken from the original escalation helper.
const (
	defaultCLIPath       = "claude"
	defaultCLITimeout    = 300.0 // seconds
	defaultMaxEscalation = 1
	cliPreviewChars      = 2000
	cliDigestMessages    = 8
	cliDigestChars       = 400
)

// Escalation triggers (the original's stop reasons that the router can see).
const (
	TriggerBackendError      = "backend_error"      // every lower tier was unreachable/unusable
	TriggerResourceExhausted = "resource_exhausted" // the first tier is under resource pressure
)

// CLIError is a failed hand-off to the claude CLI. It is deliberately not a
// llm.BackendError: a long-running hand-off must never be retried on the same tier.
type CLIError struct{ Message string }

func (e *CLIError) Error() string { return e.Message }

type reasonKey struct{}

// withReason carries the escalation reason to the CLI client, which cannot see
// the router's decision any other way.
func withReason(ctx context.Context, trigger, reason string) context.Context {
	return context.WithValue(ctx, reasonKey{}, [2]string{trigger, reason})
}

// cliResult is what the last hand-off produced, for the session record.
type cliResult struct {
	OK        bool
	Err       string
	SessionID string
	CostUSD   float64
	Stdout    string
	Stderr    string
}

// cliClient hands a conversation to the local `claude` executable in
// non-interactive print mode. It is a stateless one-shot: the model call the
// router was about to make becomes "finish this task", and the CLI's final
// text is the turn's answer (no tool calls come back).
type cliClient struct {
	tier    Tier
	workdir string
	environ func() []string
	secrets []string // literal secret values never to pass on
	last    cliResult
}

// secretEnvName reports whether an environment variable name looks like it
// holds a credential.
func secretEnvName(name string) bool {
	u := strings.ToUpper(name)
	for _, w := range []string{"KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL", "AUTH", "COOKIE", "PRIVATE", "SESSION"} {
		if strings.Contains(u, w) {
			return true
		}
	}
	return false
}

// childEnv is the environment given to the CLI: the process environment minus
// every secret-named variable, every tier's api_key_env, and any variable whose
// value contains a secret this process holds.
func (c *cliClient) childEnv(apiKeyNames []string) []string {
	skip := map[string]bool{"OPENAI_API_KEY": true, "ANTHROPIC_API_KEY": true, "ANTHROPIC_AUTH_TOKEN": true}
	for _, n := range apiKeyNames {
		skip[strings.ToUpper(n)] = true
	}
	environ := c.environ
	if environ == nil {
		environ = os.Environ
	}
	var out []string
	for _, kv := range environ() {
		name, val, ok := strings.Cut(kv, "=")
		if !ok || name == "" {
			continue
		}
		if skip[strings.ToUpper(name)] || secretEnvName(name) {
			continue
		}
		leak := false
		for _, s := range c.secrets {
			if len(s) >= 8 && strings.Contains(val, s) {
				leak = true
				break
			}
		}
		if !leak {
			out = append(out, kv)
		}
	}
	return out
}

func (c *cliClient) timeout() time.Duration {
	s := c.tier.TimeoutSeconds
	if s <= 0 {
		s = defaultCLITimeout
	}
	return time.Duration(s * float64(time.Second))
}

func (c *cliClient) path() string {
	if c.tier.CLIPath != "" {
		return c.tier.CLIPath
	}
	return defaultCLIPath
}

// BuildPrompt makes the self-contained prompt for the stateless `claude -p`
// call: the original task, why the hand-off happens, a digest of the most
// recent messages, and a request to finish. The messages are already guarded
// (secrets redacted, captures refused) by the router before they get here.
func BuildPrompt(msgs []llm.Message, reason string) string {
	task := ""
	for i := len(msgs) - 1; i >= 0; i-- {
		m := msgs[i]
		if m.Role == "user" && strings.TrimSpace(m.Content) != "" && !strings.HasPrefix(strings.TrimSpace(m.Content), "<system-reminder>") {
			task = m.Content
			break
		}
	}
	var tail []llm.Message
	for _, m := range msgs {
		if m.Role == "user" || m.Role == "assistant" || m.Role == "tool" {
			tail = append(tail, m)
		}
	}
	if len(tail) > cliDigestMessages {
		tail = tail[len(tail)-cliDigestMessages:]
	}
	var recent []string
	for _, m := range tail {
		content := strings.TrimSpace(m.Content)
		if content == "" {
			continue
		}
		if r := []rune(content); len(r) > cliDigestChars {
			content = string(r[:cliDigestChars]) + "..."
		}
		recent = append(recent, fmt.Sprintf("[%s] %s", m.Role, content))
	}
	summary := "(no prior progress)"
	if len(recent) > 0 {
		summary = strings.Join(recent, "\n")
	}
	return "A local coding agent was working on the task below and could not continue " +
		"locally, so this task has been handed off to you to finish.\n\n" +
		fmt.Sprintf("Escalation reason: %s.\n\n", reason) +
		fmt.Sprintf("Original task:\n%s\n\n", task) +
		fmt.Sprintf("Progress so far (most recent messages):\n%s\n\n", summary) +
		"Please continue and complete the task."
}

// Command builds the argument list: -p <prompt> --output-format json --cwd <dir>
// --bare, then --add-dir per extra directory, the permission flags, the extra
// args. It never emits a bypass-permissions mode.
func (t Tier) Command(prompt, cwd string) []string {
	args := []string{"-p", prompt, "--output-format", "json", "--cwd", cwd, "--bare"}
	for _, d := range t.ExtraDirs {
		args = append(args, "--add-dir", d)
	}
	if t.AcceptEdits {
		args = append(args, "--permission-mode", "acceptEdits")
	}
	return append(args, t.ExtraArgs...)
}

func preview(s string) string {
	if len(s) > cliPreviewChars {
		return s[:cliPreviewChars]
	}
	return s
}

// Complete implements Completer.
func (c *cliClient) Complete(ctx context.Context, req llm.Request, on func(llm.StreamEvent)) (*llm.Turn, error) {
	c.last = cliResult{}
	reason := "the local backend is unavailable"
	if v, ok := ctx.Value(reasonKey{}).([2]string); ok {
		reason = v[1]
	}
	prompt := BuildPrompt(req.Messages, reason)
	cwd := c.workdir
	if cwd == "" {
		cwd, _ = os.Getwd()
	}
	fail := func(msg string) (*llm.Turn, error) {
		msg, _ = RedactSecrets(msg, c.secrets)
		c.last.Err = msg
		return nil, &CLIError{Message: msg}
	}

	cctx, cancel := context.WithTimeout(ctx, c.timeout())
	defer cancel()
	cmd := exec.CommandContext(cctx, c.path(), c.tier.Command(prompt, cwd)...)
	cmd.Dir = cwd
	cmd.Env = c.childEnv([]string{c.tier.APIKeyEnv})
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	prepareTreeKill(cmd)
	cmd.WaitDelay = 2 * time.Second
	err := cmd.Run()
	c.last.Stdout, _ = RedactSecrets(preview(stdout.String()), c.secrets)
	c.last.Stderr, _ = RedactSecrets(preview(stderr.String()), c.secrets)
	if cctx.Err() != nil {
		if ctx.Err() != nil {
			return fail("claude CLI hand-off cancelled")
		}
		return fail(fmt.Sprintf("claude CLI timed out after %gs", c.timeout().Seconds()))
	}
	if err != nil {
		if _, ok := err.(*exec.ExitError); !ok {
			return fail(fmt.Sprintf("Failed to launch claude CLI: %v", err))
		}
	}
	var payload map[string]any
	if jerr := json.Unmarshal(stdout.Bytes(), &payload); jerr != nil || payload == nil || err != nil {
		return fail("claude CLI exited " + exitText(err) + " or returned no parseable JSON output")
	}
	c.last.SessionID, _ = payload["session_id"].(string)
	c.last.CostUSD, _ = payload["total_cost_usd"].(float64)
	text, _ := payload["result"].(string)
	if truthy(payload["is_error"]) {
		return fail("claude CLI reported is_error=true")
	}
	c.last.OK = true
	turn := &llm.Turn{Content: text, FinishReason: "stop", HasFinishReason: true,
		Raw: map[string]any{"cli_session_id": c.last.SessionID, "total_cost_usd": c.last.CostUSD}}
	if on != nil {
		on(llm.StreamEvent{Type: "message_start"})
		on(llm.StreamEvent{Type: "content_delta", Delta: text})
		on(llm.StreamEvent{Type: "message_stop", FinishReason: "stop"})
	}
	return turn, nil
}

func exitText(err error) string {
	if ee, ok := err.(*exec.ExitError); ok {
		return fmt.Sprint(ee.ExitCode())
	}
	return "0"
}

func truthy(v any) bool {
	switch x := v.(type) {
	case bool:
		return x
	case float64:
		return x != 0
	case string:
		return x != ""
	}
	return false
}
