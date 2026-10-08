package fleet

import (
	"bufio"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// Importing this package adds the fleet-usage, fleet-status and fleet-launch
// commands to the CLI. It deliberately registers NO tools (see the package doc).
func init() {
	cli.Register("fleet-usage", "print (or --write) Sarina's usage record for Omarchy's agents panel", usageCommand)
	cli.Register("fleet-status", "show other agents' usage and limits from Omarchy's records: fleet-status [--suggest]", statusCommand)
	cli.Register("fleet-launch", "propose launching another agent, run it only after you confirm: fleet-launch AGENT [PROMPT] [--cwd D] [--yes]", launchCommand)
}

// Seams for tests.
var (
	runner     Runner                 = ExecRunner
	lookPath   func(string) string    // nil = exec.LookPath
	isTerminal func(r io.Reader) bool = stdinIsTerminal
)

func stdinIsTerminal(r io.Reader) bool {
	f, ok := r.(*os.File)
	if !ok {
		return false
	}
	st, err := f.Stat()
	return err == nil && st.Mode()&os.ModeCharDevice != 0
}

func homeOf(env *cli.Env) string {
	if env.Home != "" {
		return env.Home
	}
	h, _ := os.UserHomeDir()
	return h
}

type multiFlag []string

func (m *multiFlag) String() string     { return strings.Join(*m, ",") }
func (m *multiFlag) Set(s string) error { *m = append(*m, s); return nil }

func newFlags(name string, env *cli.Env) *flag.FlagSet {
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	return fs
}

func parseFlags(fs *flag.FlagSet, args []string) (code int, done bool) {
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return 0, true
		}
		return 2, true
	}
	return 0, false
}

// fleet-usage [--session-dir D ...] [--write] [--usage-dir D] [--force] [--limits-only]
func usageCommand(env *cli.Env, args []string) int {
	fs := newFlags("fleet-usage", env)
	var dirs multiFlag
	fs.Var(&dirs, "session-dir", "directory of saved sessions (repeatable; replaces the defaults)")
	write := fs.Bool("write", false, "write sarina.json into the panel's usage directory instead of printing")
	usageDir := fs.String("usage-dir", "", "panel usage directory (default ~/.local/state/omarchy/agents/usage)")
	fs.Bool("force", false, "accepted and ignored (collector compatibility)")
	fs.Bool("limits-only", false, "accepted and ignored (collector compatibility)")
	if code, done := parseFlags(fs, args); done {
		return code
	}
	home := homeOf(env)
	sessionDirs := []string(dirs)
	if len(sessionDirs) == 0 {
		cwd, _ := os.Getwd()
		sessionDirs = DefaultSessionDirectories(env.Getenv, cwd, home)
	}
	record := CollectRecord(sessionDirs, time.Time{})
	if !*write {
		text, err := record.JSON()
		if err != nil {
			fmt.Fprintln(env.Stdout, "fleet-usage:", err)
			return 1
		}
		fmt.Fprintln(env.Stdout, text)
		return 0
	}
	target := *usageDir
	if target == "" {
		target = UsageDirectory(env.Getenv, home)
	}
	path, err := WriteRecord(record, target)
	if err != nil {
		fmt.Fprintln(env.Stdout, "fleet-usage:", err)
		return 1
	}
	fmt.Fprintln(env.Stdout, path)
	return 0
}

// fleet-status [--usage-dir D] [--suggest]
func statusCommand(env *cli.Env, args []string) int {
	fs := newFlags("fleet-status", env)
	usageDir := fs.String("usage-dir", "", "panel usage directory (default ~/.local/state/omarchy/agents/usage)")
	suggest := fs.Bool("suggest", false, "also name the agent with the most headroom")
	if code, done := parseFlags(fs, args); done {
		return code
	}
	dir := *usageDir
	if dir == "" {
		dir = UsageDirectory(env.Getenv, homeOf(env))
	}
	statuses := ReadAgentStatus(dir, time.Time{}, 0)
	fmt.Fprintln(env.Stdout, RenderStatus(statuses))
	if *suggest {
		fmt.Fprintln(env.Stdout)
		if pick := SuggestAgent(statuses, nil); pick != nil {
			fmt.Fprintf(env.Stdout, "Most headroom: %s (%.0f%% left in its tightest window)\n", pick.Name, *pick.HeadroomPercent())
		} else {
			fmt.Fprintln(env.Stdout, "No agent has a current, measured limit to compare.")
		}
	}
	return 0
}

// splitArgs separates flags from positionals so flags may follow the agent or
// the prompt (as with argparse). Everything after a literal "--" is positional,
// which lets a prompt begin with a dash.
func splitArgs(fs *flag.FlagSet, args []string) (positional []string, code int, done bool) {
	head, tail := args, []string(nil)
	for i, a := range args {
		if a == "--" {
			head, tail = args[:i], args[i+1:]
			break
		}
	}
	for {
		if c, d := parseFlags(fs, head); d {
			return nil, c, true
		}
		rest := fs.Args()
		if len(rest) == 0 {
			return append(positional, tail...), 0, false
		}
		positional = append(positional, rest[0])
		head = rest[1:]
	}
}

// fleet-launch AGENT [PROMPT] [--cwd D] [--yes]
//
// Exit codes: the launched agent's own status when confirmed; 1 cancelled;
// 2 could not ask or plan. --yes only skips the question: the launched agent
// still asks before acting. There is no auto-approve mode.
func launchCommand(env *cli.Env, args []string) int {
	fs := newFlags("fleet-launch", env)
	fs.Usage = func() {
		fmt.Fprintln(env.Stderr, "usage: sarina fleet-launch AGENT [PROMPT] [--cwd DIR] [--yes]")
		fmt.Fprintln(env.Stderr, "  supported agents: "+strings.Join(SupportedAgents(), ", "))
		fs.PrintDefaults()
	}
	cwd := fs.String("cwd", ".", "directory to launch the agent in")
	yes := fs.Bool("yes", false, "skip the question; the launched agent still asks before acting")
	pos, code, done := splitArgs(fs, args)
	if done {
		return code
	}
	if len(pos) == 0 || len(pos) > 2 {
		fmt.Fprintln(env.Stderr, "sarina fleet-launch: expected AGENT and at most one PROMPT")
		fs.Usage()
		return 2
	}
	agent, prompt := pos[0], ""
	if len(pos) == 2 {
		prompt = pos[1]
	}
	_, unsupported := Unsupported[agent]
	_, supported := forms[agent]
	if !supported && !unsupported {
		fmt.Fprintf(env.Stderr, "sarina fleet-launch: invalid agent %q (choose from %s)\n",
			agent, strings.Join(append(SupportedAgents(), unsupportedNames()...), ", "))
		return 2
	}
	if st, err := os.Stat(*cwd); err != nil || !st.IsDir() {
		fmt.Fprintf(env.Stdout, "fleet-launch: --cwd %q is not a directory\n", *cwd)
		return 2
	}
	proposal, err := PlanLaunch(agent, prompt, *cwd, lookPath)
	if err != nil {
		fmt.Fprintf(env.Stdout, "fleet-launch: %v\n", err)
		return 2
	}
	fmt.Fprintln(env.Stdout, proposal.AsText())
	auditPath := AuditLogPath(env.Getenv, homeOf(env))

	if !*yes {
		if !isTerminal(env.Stdin) {
			fmt.Fprintln(env.Stdout, "Not launching: standard input is not a terminal, so there is no one to confirm. Re-run interactively, or pass --yes to skip the question.")
			return 2
		}
		fmt.Fprint(env.Stdout, "Launch? [y/N] ")
		line, err := bufio.NewReader(env.Stdin).ReadString('\n')
		if err != nil && line == "" {
			fmt.Fprintln(env.Stdout)
			fmt.Fprintln(env.Stdout, "Not launching: no answer was given.")
			return 2
		}
		if a := strings.ToLower(strings.TrimSpace(line)); a != "y" && a != "yes" {
			fmt.Fprintln(env.Stdout, "Cancelled.")
			// Audit the refusal: every attempt is recorded.
			_, _ = Launch(proposal, prompt, false, auditPath, runner)
			return 1
		}
	}
	status, err := Launch(proposal, prompt, true, auditPath, runner)
	if err != nil {
		fmt.Fprintf(env.Stdout, "fleet-launch: %v\n", err)
		return 2
	}
	return status
}

func unsupportedNames() []string {
	names := make([]string, 0, len(Unsupported))
	for k := range Unsupported {
		names = append(names, k)
	}
	// stable order for the message
	for i := 1; i < len(names); i++ {
		for j := i; j > 0 && names[j] < names[j-1]; j-- {
			names[j], names[j-1] = names[j-1], names[j]
		}
	}
	return names
}
