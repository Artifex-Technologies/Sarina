package insight

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/guard"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/vault"
)

func init() {
	cli.Register("reflect", "learn from the talk log what she knows about the owner (Entities/About Zlothra.md): reflect [--path VAULT] [--dry-run]", reflectCommand)
}

type gateAdapter struct{ g guard.Gate }

func (a gateAdapter) Blocked(text string) bool { return a.g.Scan(text).Blocked }

type state struct {
	Last time.Time `json:"last"` // the newest talk line already reflected on
}

func reflectCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("reflect", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	path := fs.String("path", env.Getenv(vault.EnvVar), "the vault folder (default $"+vault.EnvVar+")")
	dry := fs.Bool("dry-run", false, "show what would change; write nothing")
	home := env.Home
	if home == "" {
		home, _ = os.UserHomeDir()
	}
	statePath := fs.String("state", filepath.Join(home, ".local", "state", "sarina", "reflect.json"), "where the last reflected talk line is remembered")
	batch := fs.Int("batch", 60, "talk lines per model call")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *path == "" {
		fmt.Fprintln(env.Stderr, "sarina reflect: no vault: give --path or set "+vault.EnvVar)
		return 2
	}
	v, err := vault.Open(*path)
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina reflect:", err)
		return 1
	}
	var st state
	if b, err := os.ReadFile(*statePath); err == nil {
		_ = json.Unmarshal(b, &st)
	}
	lines := ReadTalks(v.Root, st.Last)
	if len(lines) == 0 {
		fmt.Fprintln(env.Stdout, "nothing new to reflect on")
		return 0
	}
	model := config.ModelFromEnv(env.Getenv)
	var c Completer
	if env.NewClient != nil {
		c = env.NewClient(model)
	} else {
		c = llm.NewClient(model)
	}
	notePath := filepath.Join(v.Root, filepath.FromSlash(NoteName))
	old, readErr := os.ReadFile(notePath)
	n := ParseNote(string(old))
	gate := gateAdapter{guard.Memory()}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Minute)
	defer cancel()
	added, refined := 0, 0
	for i := 0; i < len(lines); i += *batch {
		part := lines[i:min(i+*batch, len(lines))]
		props, err := Ask(ctx, c, part)
		if err != nil {
			fmt.Fprintln(env.Stderr, "sarina reflect: the model did not answer:", err)
			return 1 // the state is not moved on, so these lines are read again next time
		}
		a, r, dropped := n.Merge(props, part, gate)
		added, refined = added+a, refined+r
		for _, d := range dropped {
			fmt.Fprintln(env.Stdout, "dropped", d)
		}
	}
	fmt.Fprintf(env.Stdout, "%d talk lines read: %d new observations, %d seen again\n", len(lines), added, refined)
	if *dry {
		fmt.Fprint(env.Stdout, "\n"+n.Render())
		return 0
	}
	if added+refined > 0 {
		mode := vault.Replace
		if readErr != nil {
			mode = vault.Create
		}
		if _, err := v.Write(guard.Memory(), NoteName, n.Render(), mode); err != nil {
			fmt.Fprintln(env.Stderr, "sarina reflect:", err)
			return 1
		}
		v.Commit("what I learned from our talks")
	}
	st.Last = lines[len(lines)-1].When
	if err := os.MkdirAll(filepath.Dir(*statePath), 0o700); err == nil {
		b, _ := json.Marshal(st)
		_ = os.WriteFile(*statePath, b, 0o600)
	}
	return 0
}
