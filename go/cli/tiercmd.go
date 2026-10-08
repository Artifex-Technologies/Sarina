package cli

import (
	"bufio"
	"context"
	"flag"
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

func init() {
	Register("tiers", "compute tiers: tiers status", tiersCommand)
	RegisterSlash("tier", "compute tiers: /tier status, /tier up (next turn starts one tier higher), /tier down", tierSlash)
}

// tierRouter builds the router from the operator's tiers.json. It returns nil
// (and the agent runs exactly as before) when there is no file, the file lists
// a single tier, or the file is invalid: a broken config can only ever mean
// "local only", never a wider destination for data.
//
// The claude_cli tier is added only by the operator's flags (or env for the
// path); escalation_config in a stored session file is deliberately ignored,
// since a workspace could otherwise ship a session that names an executable.
func (o *opts) tierRouter(env *Env, m config.ModelConfig, client agent.Completer, rt config.RuntimeConfig) *tiers.Router {
	cfg, err := tiers.Load(env.Home, env.Getenv)
	if err != nil {
		fmt.Fprintf(env.Stderr, "[tier] ignoring the tiers config, staying on the local backend only: %v\n", err)
		return nil
	}
	if eo := o.esc.Options(); eo != nil {
		eo.AcceptEdits = rt.Permissions.AllowFileWrite || rt.Permissions.AllowShellCommands
		eo.ExtraDirs = rt.AdditionalWorkingDirectories
		cfg = tiers.FromEscalation(cfg, config.EscalationConfig{}, *eo, env.Getenv)
	}
	for _, w := range cfg.Warnings {
		fmt.Fprintf(env.Stderr, "[tier] warning: %s\n", w)
	}
	if !cfg.Multi() {
		return nil
	}
	r, err := tiers.New(tiers.Options{Config: cfg, Base: m, BaseClient: client, Getenv: env.Getenv,
		Workdir: rt.CWD, Wake: tiers.NewWake(rt.Permissions, rt.CWD, nil)})
	if err != nil {
		return nil
	}
	return r
}

// stdinConfirm asks a yes/no question on the terminal, reading one line.
func stdinConfirm(env *Env) func(string) bool {
	var br *bufio.Reader
	return func(prompt string) bool {
		fmt.Fprint(env.Stderr, prompt)
		if br == nil {
			br = bufio.NewReader(env.Stdin)
		}
		line, _ := br.ReadString('\n')
		a := strings.ToLower(strings.TrimSpace(line))
		return a == "y" || a == "yes"
	}
}

// linesConfirm asks through the interactive input channel.
func linesConfirm(env *Env, lines <-chan string) func(string) bool {
	return func(prompt string) bool {
		fmt.Fprint(env.Stdout, prompt)
		select {
		case l, ok := <-lines:
			a := strings.ToLower(strings.TrimSpace(l))
			return ok && (a == "y" || a == "yes")
		case <-env.Interrupts:
			fmt.Fprintln(env.Stdout)
			return false
		}
	}
}

func renderTierStatus(rows []tiers.TierStatus, path string, resource string) string {
	var b strings.Builder
	if path == "" {
		b.WriteString("Config: none (default: the local backend only)\n")
	} else {
		fmt.Fprintf(&b, "Config: %s\n", path)
	}
	for i, s := range rows {
		mark := " "
		if s.Current {
			mark = "*"
		}
		state := "healthy"
		switch {
		case !s.Enabled:
			state = "disabled"
		case !s.Healthy:
			state = "unavailable: " + s.Why
		}
		fmt.Fprintf(&b, "%s %d. %s [%s] %s\n", mark, i+1, s.Name, s.Kind, state)
		if s.Kind == tiers.KindClaudeCLI {
			found := "not found"
			if s.CLIFound {
				found = "found"
			}
			fmt.Fprintf(&b, "     claude executable %s (%s)\n", s.CLIPath, found)
		}
		if s.BaseURL != "" || s.Model != "" {
			fmt.Fprintf(&b, "     endpoint %s  model %s\n", orDash(s.BaseURL), orDash(s.Model))
		}
		fmt.Fprintf(&b, "     allow_data %s", s.AllowData)
		if s.MaxContext > 0 {
			fmt.Fprintf(&b, "  max_context %d", s.MaxContext)
		}
		if s.APIKeyEnv != "" {
			set := "not set"
			if s.KeySet {
				set = "set"
			}
			fmt.Fprintf(&b, "  key from $%s (%s)", s.APIKeyEnv, set)
		}
		if s.Wake {
			b.WriteString("  wake configured")
		}
		if !s.Auto {
			b.WriteString("  asks before first use")
		}
		b.WriteString("\n")
	}
	if resource != "" {
		b.WriteString(resource + "\n")
	}
	return strings.TrimRight(b.String(), "\n")
}

func orDash(s string) string {
	if s == "" {
		return "-"
	}
	return s
}

func resourceLine(r *tiers.Router) string {
	s, pressure := r.Pressure(context.Background())
	th := r.Thresholds()
	part := func(ok bool, v, limit float64) string {
		if !ok {
			return "unknown"
		}
		if limit <= 0 {
			return fmt.Sprintf("%.0f%% (no limit)", v)
		}
		return fmt.Sprintf("%.0f%% (limit %.0f%%)", v, limit)
	}
	line := fmt.Sprintf("Resources: memory %s, GPU memory %s", part(s.MemOK, s.MemPercent, th.MaxMemoryPercent), part(s.GPUOK, s.GPUPercent, th.MaxGPUMemoryPercent))
	if pressure != "" {
		line += "\nPRESSURE: " + pressure
	}
	return line
}

func tiersCommand(env *Env, args []string) int {
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if sub != "status" {
		fmt.Fprintln(env.Stderr, "usage: sarina tiers status [--escalate-to-claude-code] [--claude-code-cli-path P] [--escalation-trigger T] [--disable-resource-check]")
		return exitUsage
	}
	fs := flag.NewFlagSet("tiers status", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	var esc runkit.EscalationFlags
	esc.Register(fs)
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	cfg, err := tiers.Load(env.Home, env.Getenv)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitFailure
	}
	if eo := esc.Options(); eo != nil {
		cfg = tiers.FromEscalation(cfg, config.EscalationConfig{}, *eo, env.Getenv)
	}
	for _, w := range cfg.Warnings {
		fmt.Fprintf(env.Stderr, "[tier] warning: %s\n", w)
	}
	base := config.ModelFromEnv(env.Getenv)
	r, err := tiers.New(tiers.Options{Config: cfg, Base: base, Getenv: env.Getenv})
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitFailure
	}
	fmt.Fprintln(env.Stdout, renderTierStatus(r.Status(context.Background()), cfg.Path, resourceLine(r)))
	return exitOK
}

func tierSlash(sc *SlashContext, args string) SlashResult {
	r := sc.Tiers
	if r == nil {
		return SlashResult{Text: "Compute tiers are not configured: only the local backend is used. Define tiers in ~/.sarina/tiers.json."}
	}
	sub, _, _ := strings.Cut(strings.TrimSpace(args), " ")
	switch strings.ToLower(sub) {
	case "", "status":
		return SlashResult{Text: renderTierStatus(r.Status(context.Background()), "", resourceLine(r))}
	case "up":
		name, err := r.RequestUp()
		if err != nil {
			return SlashResult{Text: "Cannot go up: " + err.Error() + "."}
		}
		return SlashResult{Text: fmt.Sprintf("The next turn will start on tier %s. Later turns go back to the lowest healthy tier.", name)}
	case "down":
		return SlashResult{Text: fmt.Sprintf("Back on tier %s; any pending /tier up is cancelled.", r.RequestDown())}
	}
	return SlashResult{Text: fmt.Sprintf("Unknown /tier option %q. Use /tier status, /tier up or /tier down.", sub)}
}
