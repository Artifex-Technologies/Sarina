package cli

import (
	"flag"
	"fmt"
	"github.com/Artifex-Technologies/Sarina/go/flagx"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/skills"
)

func init() {
	Register("skill", "named reusable prompts: skill list | skill run <name> [args]", skillCommand)
	RegisterSlash("skill", "run a skill: /skill <name> [args]; /skill alone lists them", skillSlash)
}

func (env *Env) homeDir() string {
	if env.Home != "" {
		return env.Home
	}
	h, _ := os.UserHomeDir()
	return h
}

// catalog discovers skills once per command. Anything the operator must know
// (workspace skills in use, name collisions) is printed to stderr here.
func (o *opts) catalog(env *Env) *skills.Catalog {
	if o.cat != nil {
		return o.cat
	}
	ws, _ := filepath.Abs(o.cwd)
	o.cat = skills.Discover(skills.Options{Home: env.homeDir(), Getenv: env.Getenv, Workspace: ws, TrustWorkspace: o.trustWorkspaceSkills})
	for _, n := range o.cat.Notices {
		fmt.Fprintf(env.Stderr, "[notice] %s\n", n)
	}
	for _, c := range o.cat.Collisions {
		fmt.Fprintf(env.Stderr, "[skills] %q is defined twice; using %s, ignoring %s\n", c.Name, c.Kept, c.Shadowed)
	}
	if n := len(o.cat.Problems); n > 0 {
		fmt.Fprintf(env.Stderr, "[skills] %d skill file(s) were not loaded; 'sarina skill list' shows why\n", n)
	}
	return o.cat
}

func skillCommand(env *Env, args []string) int {
	if len(args) == 0 || args[0] == "-h" || args[0] == "--help" {
		fmt.Fprintln(env.Stderr, "usage: sarina skill list [--trust-workspace-skills]\n       sarina skill run [flags] <name> [arguments...]")
		return exitUsage
	}
	switch args[0] {
	case "list":
		return skillList(env, args[1:])
	case "run":
		return skillRun(env, args[1:])
	}
	fmt.Fprintf(env.Stderr, "sarina skill: unknown subcommand %q (want list or run)\n", args[0])
	return exitUsage
}

func skillList(env *Env, args []string) int {
	o := &opts{}
	fs := flag.NewFlagSet("skill list", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	fs.StringVar(&o.cwd, "cwd", ".", "workspace directory")
	fs.BoolVar(&o.trustWorkspaceSkills, "trust-workspace-skills", false, "also load skills from <cwd>/.sarina/skills")
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	// Everything goes to stdout here: the list is the output.
	quiet := *env
	quiet.Stderr = io.Discard
	cat := o.catalog(&quiet)
	for _, n := range cat.Notices {
		fmt.Fprintf(env.Stdout, "NOTICE: %s\n", n)
	}
	if len(cat.Skills) == 0 {
		fmt.Fprintln(env.Stdout, "No skills found. Put skill files in ~/.sarina/skills/ or a directory named by SARINA_SKILLS_DIR.")
	}
	yn := func(b bool) string {
		if b {
			return "yes"
		}
		return "no"
	}
	for _, s := range cat.Skills {
		fmt.Fprintf(env.Stdout, "%s\n    %s\n    source: %s (%s)   model-invocable: %s   user-invocable: %s\n",
			s.Name, s.Description, s.Source, s.Path, yn(s.ModelInvocable), yn(s.UserInvocable))
		if s.HasAllowed {
			fmt.Fprintf(env.Stdout, "    allowed tools: %s\n", strings.Join(s.AllowedTools, ", "))
		}
	}
	for _, c := range cat.Collisions {
		fmt.Fprintf(env.Stdout, "COLLISION: %q is defined twice; using %s, ignoring %s\n", c.Name, c.Kept, c.Shadowed)
	}
	for _, p := range cat.Problems {
		fmt.Fprintf(env.Stdout, "NOT LOADED: %s: %s\n", p.Path, p.Message)
	}
	return exitOK
}

func skillRun(env *Env, args []string) int {
	o := newOpts("skill run", env)
	rest, err := flagx.Parse(o.fs, args)
	if err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	if len(rest) == 0 {
		fmt.Fprintln(env.Stderr, "usage: sarina skill run [flags] <name> [arguments...]")
		return exitUsage
	}
	s, errText := findUserSkill(o.catalog(env), rest[0])
	if errText != "" {
		fmt.Fprintf(env.Stderr, "sarina: %s\n", errText)
		return exitFailure
	}
	a, err := o.agent(env, nil)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitUsage
	}
	defer o.closeMCP()
	if s.HasAllowed {
		a.NarrowTools(append([]string{}, s.AllowedTools...))
	}
	o.noSlash = true // the expanded skill text is a prompt, never a slash command
	return conversation(env, a, o, nil, s.Expand(strings.Join(rest[1:], " ")))
}

func findUserSkill(cat *skills.Catalog, name string) (*skills.Skill, string) {
	s, ok := cat.Get(name)
	if !ok {
		return nil, fmt.Sprintf("unknown skill %q (run 'sarina skill list' to see what exists)", name)
	}
	if !s.UserInvocable {
		return nil, fmt.Sprintf("skill %q is not user-invocable", name)
	}
	return s, ""
}

func skillSlash(sc *SlashContext, args string) SlashResult {
	cat := sc.opts.catalog(sc.Env)
	name, rest, _ := strings.Cut(args, " ")
	if name == "" {
		var b strings.Builder
		for _, s := range cat.Skills {
			if s.UserInvocable {
				fmt.Fprintf(&b, "- %s: %s\n", s.Name, s.Description)
			}
		}
		if b.Len() == 0 {
			return SlashResult{Text: "No skills are available. Put skill files in ~/.sarina/skills/."}
		}
		return SlashResult{Text: "Skills (use /skill <name> [args]):\n" + strings.TrimRight(b.String(), "\n")}
	}
	s, errText := findUserSkill(cat, name)
	if errText != "" {
		return SlashResult{Text: strings.ToUpper(errText[:1]) + errText[1:]}
	}
	r := SlashResult{Prompt: s.Expand(strings.TrimSpace(rest))}
	if s.HasAllowed {
		r.Narrow = append([]string{}, s.AllowedTools...)
	}
	return r
}
