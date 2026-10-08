package cli

import (
	"flag"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/agents"
	"github.com/Artifex-Technologies/Sarina/go/mcp"
	"github.com/Artifex-Technologies/Sarina/go/policy"
)

func init() {
	Register("mcp", "show configured MCP servers (nothing is started): mcp list", mcpCommand)
	Register("agents", "show sub-agent profiles: agents list", agentsCommand)
}

func mcpCommand(env *Env, args []string) int {
	if len(args) == 0 || args[0] != "list" {
		fmt.Fprintln(env.Stderr, "usage: sarina mcp list [--cwd DIR] [--trust-workspace-mcp]")
		return exitUsage
	}
	fs := flag.NewFlagSet("mcp list", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	cwd := fs.String("cwd", ".", "workspace directory")
	trust := fs.Bool("trust-workspace-mcp", false, "also read MCP definition files in the workspace")
	if err := fs.Parse(args[1:]); err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	ws, _ := filepath.Abs(*cwd)
	pol := policy.Load(policy.Options{Home: env.homeDir(), Getenv: env.Getenv, Workspace: ws})
	if !pol.Trusted {
		fmt.Fprintln(env.Stdout, "MCP servers are disabled: the operator policy sets trusted=false")
		return exitOK
	}
	cfg := mcp.Load(mcp.LoadOptions{Home: env.homeDir(), Getenv: env.Getenv, Workspace: ws, TrustWorkspace: *trust})
	for _, n := range cfg.Notices {
		fmt.Fprintf(env.Stdout, "NOTICE: %s\n", n)
	}
	if len(cfg.Servers) == 0 {
		fmt.Fprintln(env.Stdout, "No MCP servers configured. Define them in ~/.sarina/mcp.json or a file named by SARINA_MCP_FILE.")
	}
	for _, s := range cfg.Servers {
		ro := ""
		if s.ReadOnly {
			ro = "   read-only"
		}
		fmt.Fprintf(env.Stdout, "%s\n    %s %s\n    source: %s (%s)%s\n", s.Name, s.Command, strings.Join(s.Args, " "), s.Source, s.File, ro)
	}
	for _, p := range cfg.Problems {
		fmt.Fprintf(env.Stdout, "PROBLEM: %s\n", p)
	}
	return exitOK
}

func agentsCommand(env *Env, args []string) int {
	if len(args) == 0 || args[0] != "list" {
		fmt.Fprintln(env.Stderr, "usage: sarina agents list [--cwd DIR]")
		return exitUsage
	}
	fs := flag.NewFlagSet("agents list", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	cwd := fs.String("cwd", ".", "workspace directory")
	if err := fs.Parse(args[1:]); err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	ws, _ := filepath.Abs(*cwd)
	pol := policy.Load(policy.Options{Home: env.homeDir(), Getenv: env.Getenv, Workspace: ws})
	cat := agents.Load(agents.Options{Home: env.homeDir(), Getenv: env.Getenv, Trusted: pol.Trusted})
	for _, p := range cat.Profiles {
		tl := "everything a read-only helper may use"
		if p.Tools != nil {
			tl = strings.Join(p.Tools, ", ")
		}
		fmt.Fprintf(env.Stdout, "%s\n    %s\n    source: %s   tools: %s   shell: %v\n", p.Name, p.Description, p.Source, tl, p.Shell)
	}
	for _, p := range cat.Problems {
		fmt.Fprintf(env.Stdout, "NOT LOADED: %s\n", p)
	}
	return exitOK
}
