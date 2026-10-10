package council

import (
	"bufio"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/flagx"
)

// Importing this package adds the council command to the CLI and the council_ask
// tool (see tool.go for what gates it).
func init() {
	cli.Register("council", "ask several models the same question and compare: council [--context-file F] [--members a,b] [--local-only] [--no-synthesis] [--json] QUESTION | council status [--probe]", command)
}

const maxContextFile = 64 * 1024

const usage = "usage: sarina council [--context-file F] [--members a,b] [--local-only] [--no-synthesis] [--json] QUESTION\n       sarina council status [--probe]"

func command(env *cli.Env, args []string) int {
	if len(args) > 0 && args[0] == "status" {
		return status(env, args[1:])
	}
	fs := flag.NewFlagSet("council", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	ctxFile := fs.String("context-file", "", "a file whose text is added to the question (secrets are removed for remote members)")
	members := fs.String("members", "", "comma-separated member names (default: all enabled)")
	localOnly := fs.Bool("local-only", false, "ask only members on this machine")
	noSynth := fs.Bool("no-synthesis", false, "skip the local comparison of the answers")
	asJSON := fs.Bool("json", false, "print the result as JSON")
	pos, err := flagx.Parse(fs, args)
	if err != nil {
		if err == flag.ErrHelp {
			return 0
		}
		return 2
	}
	question := strings.TrimSpace(strings.Join(pos, " "))
	if question == "" {
		fmt.Fprintln(env.Stderr, usage)
		return 2
	}
	cfg, err := Load(env.Home, env.Getenv)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return 1
	}
	for _, w := range cfg.Warnings {
		fmt.Fprintf(env.Stderr, "[council] warning: %s\n", w)
	}
	if len(cfg.Members) == 0 {
		fmt.Fprintf(env.Stderr, "sarina: no council configured; list members in %s (see docs/ECOSYSTEM.md section 9)\n", orPath(Path(env.Home, env.Getenv)))
		return 1
	}
	if *ctxFile != "" {
		text, err := readCapped(*ctxFile)
		if err != nil {
			fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
			return 1
		}
		question += "\n\nContext (" + *ctxFile + "):\n" + text
	}
	var only []string
	if *members != "" {
		only = strings.Split(*members, ",")
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	br := bufio.NewReader(env.Stdin)
	confirm := func(prompt string) bool {
		fmt.Fprint(env.Stderr, prompt)
		line, _ := br.ReadString('\n')
		a := strings.ToLower(strings.TrimSpace(line))
		return a == "y" || a == "yes"
	}
	res := Ask(ctx, Options{Config: cfg, Getenv: env.Getenv, Confirm: confirm, Only: only, LocalOnly: *localOnly, NoSynthesis: *noSynth}, question)
	if *asJSON {
		out, _ := json.MarshalIndent(res, "", "  ")
		fmt.Fprintln(env.Stdout, string(out))
	} else {
		fmt.Fprintln(env.Stdout, res.Text())
	}
	for _, a := range res.Answers {
		if a.Text != "" {
			return 0
		}
	}
	return 1 // nobody answered
}

func status(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("council status", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	probe := fs.Bool("probe", false, "check that each enabled member answers (one models-list request each; no question is sent)")
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return 0
		}
		return 2
	}
	cfg, err := Load(env.Home, env.Getenv)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return 1
	}
	if cfg.Path == "" {
		fmt.Fprintf(env.Stdout, "Config: none (%s)\nNo council members.\n", orPath(Path(env.Home, env.Getenv)))
		return 0
	}
	fmt.Fprintf(env.Stdout, "Config: %s\n", cfg.Path)
	for _, w := range cfg.Warnings {
		fmt.Fprintf(env.Stdout, "warning: %s\n", w)
	}
	for i, m := range cfg.Members {
		where := "remote"
		if m.Local {
			where = "local"
		}
		state := "enabled"
		if !m.Enabled {
			state = "disabled"
		}
		endpoint := m.BaseURL
		if endpoint == "" {
			endpoint = "(" + m.Kind + " default)"
		}
		fmt.Fprintf(env.Stdout, "%d. %s [%s, %s, %s] model %s at %s\n", i+1, m.Name, m.Kind, where, state, m.Model, endpoint)
		fmt.Fprintf(env.Stdout, "     allow_data %s", m.AllowData)
		if m.APIKeyEnv != "" {
			set := "not set"
			if env.Getenv != nil && env.Getenv(m.APIKeyEnv) != "" {
				set = "set"
			}
			fmt.Fprintf(env.Stdout, "  key from $%s (%s)", m.APIKeyEnv, set)
		}
		if !m.Auto {
			fmt.Fprint(env.Stdout, "  asks before first use")
		}
		fmt.Fprintln(env.Stdout)
		if *probe && m.Enabled {
			if ok, why := Probe(context.Background(), Options{Config: cfg, Getenv: env.Getenv}, m); ok {
				fmt.Fprintln(env.Stdout, "     reachable")
			} else {
				fmt.Fprintf(env.Stdout, "     NOT reachable: %s\n", why)
			}
		}
	}
	if cfg.Synthesizer != "" {
		fmt.Fprintf(env.Stdout, "Comparison by: %s\n", cfg.Synthesizer)
	} else {
		fmt.Fprintln(env.Stdout, "Comparison: none (no local member)")
	}
	if *probe {
		fmt.Fprintln(env.Stdout, "Probed with a models-list request only; no question was sent.")
	} else {
		fmt.Fprintln(env.Stdout, "Status reads the config only; nothing was sent to any member.")
	}
	return 0
}

func readCapped(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	b, err := io.ReadAll(io.LimitReader(f, maxContextFile+1))
	if err != nil {
		return "", err
	}
	if len(b) > maxContextFile {
		return "", fmt.Errorf("%s is larger than %d KB; trim it first", path, maxContextFile/1024)
	}
	return string(b), nil
}

func orPath(p string) string {
	if p == "" {
		return "~/.sarina/council.json"
	}
	return p
}
