package fleet

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"
	"unicode/utf8"
)

// Launching other AI agents: proposal-first, human-confirmed, audited.
//
// Omarchy starts every agent with its own spelling of "don't stop to ask". A
// launcher that copied that would turn a prompt-injected Sarina into a way to run
// another agent with no human in the loop, so this one:
//   - never adds an auto-approve flag and has no option to (the forms below are
//     Omarchy's with those flags removed);
//   - proposes first: PlanLaunch is pure, nothing runs until Launch is called
//     with confirmed=true;
//   - is reachable only from the fleet-launch CLI command, never as a model tool;
//   - audits every attempt, confirmed or refused, to a log Chymaera Sentry can
//     read (the prompt is stored as a hash, a length and a short preview).

const (
	PromptPreviewLength = 60
	MaxPromptLength     = 8000
)

type form struct {
	kind   string // positional | flag | joined
	head   []string
	attach []string // positional: tokens before the prompt; flag/joined: attach[0] is the flag
}

// Per agent: argv up to the prompt, then how the prompt is attached. "--" is
// used wherever the agent supports it so a prompt beginning with a dash cannot be
// read as an option. No entry contains an approval-skipping flag.
var forms = map[string]form{
	"claude":       {"positional", []string{"claude"}, []string{"--"}},
	"codex":        {"positional", []string{"codex"}, []string{"--"}},
	"grok":         {"positional", []string{"grok"}, []string{"--"}},
	"muse":         {"positional", []string{"muse"}, []string{"--"}},
	"omp":          {"positional", []string{"omp"}, []string{"--"}},
	"pi":           {"positional", []string{"pi"}, nil},
	"cursor-agent": {"positional", []string{"cursor-agent"}, []string{"agent", "--"}},
	"opencode":     {"flag", []string{"opencode"}, []string{"--prompt"}},
	"gemini":       {"flag", []string{"gemini"}, []string{"--prompt-interactive"}},
	"copilot":      {"flag", []string{"copilot"}, []string{"--interactive"}},
	"hermes":       {"joined", []string{"hermes", "chat", "--tui"}, []string{"--query"}},
}

// Unsupported lists agents Omarchy can launch that this module deliberately does
// not, with the reason.
var Unsupported = map[string]string{
	"crush":    "its only prompt-seeded mode (`crush run`) never asks for approval",
	"openclaw": "it needs Omarchy's own launcher to attach to its gateway",
}

// NoApprovalPrompts are agents with no approval prompts of their own; launching
// one is unattended no matter what Sarina passes, so the operator is told.
var NoApprovalPrompts = map[string]bool{"pi": true}

// LaunchError is a proposal that cannot be made or a launch that was not authorised.
type LaunchError struct{ Msg string }

func (e *LaunchError) Error() string { return e.Msg }

func launchErr(format string, a ...any) error { return &LaunchError{fmt.Sprintf(format, a...)} }

// LaunchProposal describes a launch without doing it.
type LaunchProposal struct {
	Agent        string
	Argv         []string
	Cwd          string
	BinaryPath   string // empty when not found on PATH
	Display      string
	PromptSHA256 string
	PromptLength int
	Warnings     []string
}

// AsText is the human-readable proposal. It never contains the prompt text.
func (p LaunchProposal) AsText() string {
	approval := "  approval: Sarina passes no auto-approve flag"
	if !NoApprovalPrompts[p.Agent] {
		approval += "; the agent will ask before acting"
	}
	lines := []string{
		"Proposed launch: " + p.Agent,
		"  command : " + p.Display,
		"  cwd     : " + p.Cwd,
		fmt.Sprintf("  prompt  : %d characters", p.PromptLength),
		approval,
	}
	if p.BinaryPath == "" {
		lines = append(lines, fmt.Sprintf("  warning : `%s` was not found on PATH", p.Argv[0]))
	}
	for _, w := range p.Warnings {
		lines = append(lines, "  warning : "+w)
	}
	return strings.Join(lines, "\n")
}

func buildArgv(f form, prompt string) []string {
	argv := append([]string(nil), f.head...)
	if prompt == "" {
		return argv
	}
	switch f.kind {
	case "positional":
		argv = append(argv, f.attach...)
		argv = append(argv, prompt)
	case "flag":
		argv = append(argv, f.attach[0], prompt)
	default:
		argv = append(argv, f.attach[0]+"="+prompt)
	}
	return argv
}

// SupportedAgents lists the launchable agents, sorted.
func SupportedAgents() []string {
	out := make([]string, 0, len(forms))
	for k := range forms {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// PlanLaunch describes a launch without doing it. which resolves a binary name to
// a path ("" = not found); nil means exec.LookPath. An empty cwd means the
// current directory.
func PlanLaunch(agent, prompt, cwd string, which func(string) string) (LaunchProposal, error) {
	if reason, ok := Unsupported[agent]; ok {
		return LaunchProposal{}, launchErr("%s is not supported: %s.", agent, reason)
	}
	f, ok := forms[agent]
	if !ok {
		return LaunchProposal{}, launchErr("unknown agent %q; supported: %s", agent, strings.Join(SupportedAgents(), ", "))
	}
	if n := utf8.RuneCountInString(prompt); n > MaxPromptLength {
		return LaunchProposal{}, launchErr("prompt is %d characters; the limit is %d", n, MaxPromptLength)
	}
	if strings.ContainsRune(prompt, 0) {
		return LaunchProposal{}, launchErr("prompt contains a NUL byte")
	}
	if which == nil {
		which = func(name string) string {
			p, err := exec.LookPath(name)
			if err != nil {
				return ""
			}
			return p
		}
	}
	display := strings.Join(buildArgv(f, map[bool]string{true: "<prompt>", false: ""}[prompt != ""]), " ")
	var warnings []string
	if NoApprovalPrompts[agent] {
		warnings = append(warnings, agent+" has no approval prompts of its own, so it will run unattended")
	}
	if cwd == "" {
		cwd = "."
	}
	abs, err := filepath.Abs(cwd)
	if err != nil {
		abs = cwd
	}
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		abs = resolved
	}
	sum := sha256.Sum256([]byte(prompt))
	return LaunchProposal{
		Agent:        agent,
		Argv:         buildArgv(f, prompt),
		Cwd:          abs,
		BinaryPath:   which(f.head[0]),
		Display:      display,
		PromptSHA256: hex.EncodeToString(sum[:]),
		PromptLength: utf8.RuneCountInString(prompt),
		Warnings:     warnings,
	}, nil
}

// AuditLogPath is where launch attempts are recorded.
func AuditLogPath(getenv func(string) string, home string) string {
	if s := getenv("XDG_STATE_HOME"); s != "" {
		return filepath.Join(s, "sarina", "agent-launch.jsonl")
	}
	return filepath.Join(home, ".local", "state", "sarina", "agent-launch.jsonl")
}

type auditEntry struct {
	Time          string `json:"time"`
	Agent         string `json:"agent"`
	Argv0         string `json:"argv0"`
	Cwd           string `json:"cwd"`
	PromptSHA256  string `json:"promptSha256"`
	PromptLength  int    `json:"promptLength"`
	PromptPreview string `json:"promptPreview"`
	Outcome       string `json:"outcome"`
	Detail        string `json:"detail"`
}

func audit(p LaunchProposal, prompt, outcome, logPath, detail string) error {
	preview := prompt
	if r := []rune(preview); len(r) > PromptPreviewLength {
		preview = string(r[:PromptPreviewLength])
	}
	preview = strings.ReplaceAll(preview, "\n", " ")
	line, err := compactJSON(auditEntry{
		Time:          time.Now().UTC().Truncate(time.Microsecond).Format("2006-01-02T15:04:05.999999+00:00"),
		Agent:         p.Agent,
		Argv0:         p.Argv[0],
		Cwd:           p.Cwd,
		PromptSHA256:  p.PromptSHA256,
		PromptLength:  p.PromptLength,
		PromptPreview: preview,
		Outcome:       outcome,
		Detail:        detail,
	})
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(logPath), 0o755); err != nil {
		return err
	}
	f, err := os.OpenFile(logPath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	_, werr := f.WriteString(line + "\n")
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	return werr
}

// Runner runs argv in dir in the foreground (inheriting the terminal, so the
// human can answer the agent's own prompts) and returns its exit status.
type Runner func(argv []string, dir string) (int, error)

// ExecRunner is the real Runner.
func ExecRunner(argv []string, dir string) (int, error) {
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir = dir
	cmd.Stdin, cmd.Stdout, cmd.Stderr = os.Stdin, os.Stdout, os.Stderr
	err := cmd.Run()
	if err == nil {
		return 0, nil
	}
	var ee *exec.ExitError
	if errors.As(err, &ee) {
		return ee.ExitCode(), nil
	}
	return 0, err
}

// Launch runs an approved proposal in the foreground and returns its exit
// status. It returns a *LaunchError unless confirmed is true; the refusal is
// audited too. If the audit entry cannot be written, nothing is run.
func Launch(p LaunchProposal, prompt string, confirmed bool, auditPath string, run Runner) (int, error) {
	if run == nil {
		run = ExecRunner
	}
	sum := sha256.Sum256([]byte(prompt))
	refuse := func(detail string, e error) (int, error) {
		if aerr := audit(p, prompt, "refused", auditPath, detail); aerr != nil {
			return 0, launchErr("%v (and the audit log could not be written: %v)", e, aerr)
		}
		return 0, e
	}
	if hex.EncodeToString(sum[:]) != p.PromptSHA256 {
		return refuse("prompt differs from the proposal", launchErr("prompt does not match the proposal that was shown"))
	}
	if !confirmed {
		return refuse("not confirmed by the operator", launchErr("launch was not confirmed"))
	}
	if p.BinaryPath == "" {
		return refuse(p.Argv[0]+" not on PATH", launchErr("`%s` was not found on PATH", p.Argv[0]))
	}
	if err := audit(p, prompt, "launched", auditPath, ""); err != nil {
		return 0, launchErr("not launching: the audit log could not be written: %v", err)
	}
	return run(append([]string(nil), p.Argv...), p.Cwd)
}
