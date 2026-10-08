// Package cli is the command-line front end: a small command table (so other
// packages can add subcommands by name) and the built-in chat and resume commands.
package cli

import (
	"fmt"
	"io"
	"os"
	"os/signal"
	"sort"
	"strings"
	"sync"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
)

// Env is everything a command needs from its surroundings; tests replace it.
type Env struct {
	Stdin      io.Reader
	Stdout     io.Writer
	Stderr     io.Writer
	Getenv     func(string) string
	Interrupts <-chan os.Signal // Ctrl-C; nil means "install a real handler"
	Home       string           // home directory for ~/.sarina; empty means the user's real one
	// NewClient builds the model transport; nil uses the HTTP client.
	NewClient func(config.ModelConfig) agent.Completer
}

// Command runs a subcommand and returns the process exit code.
type Command func(env *Env, args []string) int

type entry struct {
	summary string
	run     Command
}

var (
	mu       sync.Mutex
	commands = map[string]entry{}
)

// Register adds (or replaces) a subcommand. Other packages call this from
// init() or setup code; dispatch is by name.
func Register(name, summary string, run Command) {
	mu.Lock()
	defer mu.Unlock()
	commands[name] = entry{summary, run}
}

func init() {
	Register("chat", "run a prompt once, or hold an interactive conversation", chatCommand)
	Register("resume", "continue a saved session: resume <session_id> [prompt]", resumeCommand)
}

// Main dispatches args (without the program name) and returns the exit code.
func Main(args []string, env Env) int {
	if env.Stdin == nil {
		env.Stdin = os.Stdin
	}
	if env.Stdout == nil {
		env.Stdout = os.Stdout
	}
	if env.Stderr == nil {
		env.Stderr = os.Stderr
	}
	if env.Getenv == nil {
		env.Getenv = os.Getenv
	}
	if env.Interrupts == nil {
		ch := make(chan os.Signal, 1)
		signal.Notify(ch, os.Interrupt)
		defer signal.Stop(ch)
		env.Interrupts = ch
	}
	if len(args) == 0 || args[0] == "-h" || args[0] == "--help" || args[0] == "help" {
		usage(env.Stdout)
		if len(args) == 0 {
			return 2
		}
		return 0
	}
	mu.Lock()
	e, ok := commands[args[0]]
	mu.Unlock()
	if !ok {
		fmt.Fprintf(env.Stderr, "sarina: unknown command %q\n\n", args[0])
		usage(env.Stderr)
		return 2
	}
	return e.run(&env, args[1:])
}

func usage(w io.Writer) {
	mu.Lock()
	names := make([]string, 0, len(commands))
	for n := range commands {
		names = append(names, n)
	}
	sort.Strings(names)
	var b strings.Builder
	b.WriteString("Usage: sarina <command> [flags] [arguments]\n\nCommands:\n")
	for _, n := range names {
		fmt.Fprintf(&b, "  %-18s %s\n", n, commands[n].summary)
	}
	mu.Unlock()
	b.WriteString("\nRun 'sarina <command> -h' for a command's flags.\n")
	io.WriteString(w, b.String())
}
