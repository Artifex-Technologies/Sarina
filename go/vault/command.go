package vault

import (
	"flag"
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// Importing this package adds the vault-init, vault-lint and vault-search commands (and the vault tools, when SARINA_VAULT is set).
func init() {
	cli.Register("vault-init", "make the starting folders and Home note in the vault (never overwrites): vault-init [--path DIR]", initCommand)
	cli.Register("vault-lint", "broken links, orphans and notes without front matter: vault-lint [--path DIR]", lintCommand)
	cli.Register("vault-search", "search the vault: vault-search [--path DIR] WORDS...", searchCommand)
}

func open(env *cli.Env, name string, args []string) (*Vault, []string, int) {
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	path := fs.String("path", env.Getenv(EnvVar), "the vault folder (default $"+EnvVar+")")
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return nil, nil, 0
		}
		return nil, nil, 2
	}
	if *path == "" {
		fmt.Fprintf(env.Stderr, "sarina %s: no vault: give --path or set %s\n", name, EnvVar)
		return nil, nil, 2
	}
	v, err := Open(*path)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina %s: %v\n", name, err)
		return nil, nil, 1
	}
	return v, fs.Args(), -1
}

func initCommand(env *cli.Env, args []string) int {
	v, _, rc := open(env, "vault-init", args)
	if rc >= 0 {
		return rc
	}
	made, err := v.Init()
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina vault-init:", err)
		return 1
	}
	if len(made) == 0 {
		fmt.Fprintln(env.Stdout, "the vault is already set up; nothing changed")
		return 0
	}
	fmt.Fprintln(env.Stdout, "made: "+strings.Join(made, ", "))
	return 0
}

func lintCommand(env *cli.Env, args []string) int {
	v, _, rc := open(env, "vault-lint", args)
	if rc >= 0 {
		return rc
	}
	notes, err := v.Notes()
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina vault-lint:", err)
		return 1
	}
	problems := Lint(notes)
	for _, p := range problems {
		fmt.Fprintf(env.Stdout, "%s: %s\n", p.Path, p.What)
	}
	fmt.Fprintf(env.Stdout, "%d notes, %d things to look at\n", len(notes), len(problems))
	if len(problems) > 0 {
		return 1
	}
	return 0
}

func searchCommand(env *cli.Env, args []string) int {
	v, rest, rc := open(env, "vault-search", args)
	if rc >= 0 {
		return rc
	}
	notes, err := v.Notes()
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina vault-search:", err)
		return 1
	}
	hits := Search(notes, strings.Join(rest, " "), 10)
	for _, h := range hits {
		fmt.Fprintf(env.Stdout, "%s  [%s]\n  %s\n", h.Path, strings.Join(h.Why, ", "), h.Snippet)
	}
	if len(hits) == 0 {
		fmt.Fprintln(env.Stdout, "no notes matched")
	}
	return 0
}
