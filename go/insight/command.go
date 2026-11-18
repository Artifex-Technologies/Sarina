package insight

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"github.com/Artifex-Technologies/Sarina/go/testguard"
	"os"
	"path/filepath"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/guard"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/sysinfo"
	"github.com/Artifex-Technologies/Sarina/go/vault"
)

func init() {
	cli.Register("reflect", "learn from the talk log what she knows about the owner (Entities/About <SARINA_OWNER_NAME>.md): reflect [--path VAULT] [--dry-run]", reflectCommand)
	cli.Register("vault-ingest", "turn new files in the vault's Inbox/ into linked notes under Sources/ (originals untouched): vault-ingest [--path VAULT] [--dry-run]", ingestCommand)
}

func ingestCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("vault-ingest", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	path := fs.String("path", env.Getenv(vault.EnvVar), "the vault folder (default $"+vault.EnvVar+")")
	dry := fs.Bool("dry-run", false, "show the notes; write nothing")
	home := env.Home
	if home == "" {
		home, _ = os.UserHomeDir()
	}
	statePath := fs.String("state", filepath.Join(home, ".local", "state", "sarina", "ingest.json"), "which inbox files were read, by content")
	force := fs.Bool("force", false, "run even while a game is running")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if rc := notWhileGaming(env, "vault-ingest", *force); rc != 0 {
		return rc
	}
	if *path == "" {
		fmt.Fprintln(env.Stderr, "sarina vault-ingest: no vault: give --path or set "+vault.EnvVar)
		return 2
	}
	v, err := vault.Open(*path)
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina vault-ingest:", err)
		return 1
	}
	done := map[string]string{}
	if b, err := readState(*statePath); err == nil {
		_ = json.Unmarshal(b, &done)
		if done == nil { // a state of "null"
			done = map[string]string{}
		}
	}
	files, skipped := NewInboxFiles(v.Root, done)
	for _, s := range skipped {
		fmt.Fprintln(env.Stdout, "skipped", s)
	}
	if len(files) == 0 {
		fmt.Fprintln(env.Stdout, "nothing new in the inbox")
		return 0
	}
	model := config.ModelFromEnv(env.Getenv)
	var c Completer
	if env.NewClient != nil {
		c = env.NewClient(model)
	} else {
		c = llm.NewClient(model)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Minute)
	defer cancel()
	notes, _ := v.Notes()
	wrote := 0
	for _, f := range files {
		sum, err := Summarize(ctx, c, filepath.Base(f.Rel), f.Text)
		if err != nil {
			fmt.Fprintf(env.Stdout, "%s: %v (tried again next time)\n", f.Rel, err)
			continue
		}
		name := FreeName(v.Root, sum.Title)
		body := sum.Render(f.Rel, f.Cut, Related(notes, sum, name))
		if *dry {
			fmt.Fprintf(env.Stdout, "--- %s\n%s\n", name, body)
			continue
		}
		if _, err := v.Write(guard.Memory(), name, body, vault.Create); err != nil {
			fmt.Fprintf(env.Stdout, "%s: %v\n", f.Rel, err)
		} else {
			fmt.Fprintf(env.Stdout, "%s -> %s\n", f.Rel, name)
			wrote++
		}
		done[f.Rel] = f.Hash // also when the gate refused it: the same file is not tried every day; a changed one is
	}
	if *dry {
		return 0
	}
	if wrote > 0 {
		v.Commit(fmt.Sprintf("notes from %d inbox file(s)", wrote))
	}
	if err := os.MkdirAll(filepath.Dir(*statePath), 0o700); err == nil {
		b, _ := json.Marshal(done)
		_ = writeState(*statePath, b)
	}
	return 0
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
	force := fs.Bool("force", false, "run even while a game is running")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *batch < 1 { // 0 never moved on through the lines; below 0 cut a slice backwards
		fmt.Fprintln(env.Stderr, "sarina reflect: --batch must be at least 1")
		return 2
	}
	if rc := notWhileGaming(env, "reflect", *force); rc != 0 {
		return rc
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
	if b, err := readState(*statePath); err == nil {
		_ = json.Unmarshal(b, &st)
	}
	lines, echoes := WithoutEchoes(ReadTalks(v.Root, st.Last))
	for _, e := range echoes {
		// her own answer heard back through the microphone is not something the owner said
		fmt.Fprintf(env.Stdout, "skipped (my own voice heard back): %q\n", e.Said)
	}
	if len(lines) == 0 {
		fmt.Fprintln(env.Stdout, "nothing new to reflect on")
		if len(echoes) > 0 {
			st.Last = echoes[len(echoes)-1].When
			if err := os.MkdirAll(filepath.Dir(*statePath), 0o700); err == nil {
				b, _ := json.Marshal(st)
				_ = writeState(*statePath, b)
			}
		}
		return 0
	}
	model := config.ModelFromEnv(env.Getenv)
	var c Completer
	if env.NewClient != nil {
		c = env.NewClient(model)
	} else {
		c = llm.NewClient(model)
	}
	// the note as it is: only a regular file is read (a link under its name is not followed, a named pipe not waited
	// on), before the model is asked
	notePath := filepath.Join(v.Root, filepath.FromSlash(NoteName()))
	var old []byte
	exists := false
	if st, err := os.Lstat(notePath); err == nil {
		if !st.Mode().IsRegular() {
			fmt.Fprintf(env.Stderr, "sarina reflect: %s is not a regular file; not read\n", NoteName())
			return 1
		}
		if old, err = session.ReadRegular(notePath, maxNoteBytes); err != nil {
			fmt.Fprintln(env.Stderr, "sarina reflect:", err)
			return 1
		}
		exists = true
	}
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
		if !exists {
			mode = vault.Create
		}
		if _, err := v.Write(guard.Memory(), NoteName(), n.Render(), mode); err != nil {
			fmt.Fprintln(env.Stderr, "sarina reflect:", err)
			return 1
		}
		v.Commit("what I learned from our talks")
	}
	st.Last = lines[len(lines)-1].When
	if len(echoes) > 0 && echoes[len(echoes)-1].When.After(st.Last) {
		st.Last = echoes[len(echoes)-1].When
	}
	if err := os.MkdirAll(filepath.Dir(*statePath), 0o700); err == nil {
		b, _ := json.Marshal(st)
		_ = writeState(*statePath, b)
	}
	return 0
}

// notWhileGaming keeps a background job off the graphics card while a game runs: the model would take several gigabytes of the card's
// memory from the game. Exit 75 (EX_TEMPFAIL) is what the timers' units retry on later.
func notWhileGaming(env *cli.Env, job string, force bool) int {
	if force {
		return 0
	}
	if why, yes := sysinfo.GameRunning(context.Background()); yes {
		fmt.Fprintf(env.Stdout, "sarina %s: a game is running (%s); not now, so the game keeps the graphics card (--force overrides)\n", job, why)
		return 75
	}
	return 0
}

// maxStateBytes bounds a state file as read: ingest.json holds a line per inbox file ever read.
const maxStateBytes = 4 << 20

// maxNoteBytes bounds the About note as read: the vault reads a note it replaces up to the same size.
const maxNoteBytes = 4 << 20

// readState reads a state file: a regular file of a bounded size (a named pipe planted at its name is not waited on).
func readState(path string) ([]byte, error) { return session.ReadRegular(path, maxStateBytes) }

// writeState keeps which files were read; never under a test run outside its folders. It replaces the file through a new
// one renamed over it: writing into whatever is at the name would follow a link planted there (writing the state
// wherever it led) and wait for ever on a named pipe.
func writeState(path string, b []byte) error {
	if err := testguard.Allow(path); err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+"-*")
	if err != nil {
		return err
	}
	_, werr := f.Write(b)
	cerr := f.Close()
	if werr == nil && cerr == nil {
		if werr = os.Rename(f.Name(), path); werr == nil {
			return nil
		}
	}
	os.Remove(f.Name())
	if werr != nil {
		return werr
	}
	return cerr
}
