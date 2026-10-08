package cli

import (
	"context"
	"flag"
	"fmt"
	"github.com/Artifex-Technologies/Sarina/go/flagx"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/memory"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

func init() {
	Register("memory-status", "show curated memory: paths, usage and entries", memoryStatusCommand)
	Register("memory-show", "print the curated-memory block exactly as sessions receive it", memoryShowCommand)
	Register("session-search", "search saved sessions: session-search [--cwd D] [--session-dir D] [--max N] <query...>", sessionSearchCommand)
	Register("memory-review", "ask the model what is worth remembering from a saved session: memory-review [flags] <session_id>", memoryReviewCommand)
	Register("memory-review-list", "list memory proposals awaiting review", memoryReviewListCommand)
	Register("memory-review-approve", "approve a memory proposal: memory-review-approve <entry_id>", memoryReviewApproveCommand)
	Register("memory-review-reject", "reject a memory proposal: memory-review-reject <entry_id>", memoryReviewRejectCommand)
}

func (env *Env) memoryRuntime(cwd string) *memory.Runtime {
	return memory.FromWorkspace(cwd, env.homeDir())
}

func cwdFlag(name string, env *Env) (*flag.FlagSet, *string) {
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	return fs, fs.String("cwd", ".", "workspace directory")
}

func parseOrExit(fs *flag.FlagSet, args []string) (int, bool) {
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return exitOK, false
		}
		return exitUsage, false
	}
	return 0, true
}

func memoryStatusCommand(env *Env, args []string) int {
	fs, cwd := cwdFlag("memory-status", env)
	if code, ok := parseOrExit(fs, args); !ok {
		return code
	}
	fmt.Fprintln(env.Stdout, env.memoryRuntime(*cwd).RenderSummary())
	return exitOK
}

func memoryShowCommand(env *Env, args []string) int {
	fs, cwd := cwdFlag("memory-show", env)
	if code, ok := parseOrExit(fs, args); !ok {
		return code
	}
	block := env.memoryRuntime(*cwd).RenderPromptBlock()
	if block == "" {
		block = "No curated memory is stored."
	}
	fmt.Fprintln(env.Stdout, block)
	return exitOK
}

func sessionSearchCommand(env *Env, args []string) int {
	fs, cwd := cwdFlag("session-search", env)
	dir := fs.String("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)")
	max := fs.Int("max", 10, "maximum number of hits")
	words, err := flagx.Parse(fs, args)
	if err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	query := strings.TrimSpace(strings.Join(words, " "))
	if query == "" {
		fmt.Fprintln(env.Stderr, "usage: sarina session-search [flags] <query...>")
		return exitUsage
	}
	if *dir == "" {
		*dir = config.DefaultRuntime(*cwd).SessionDirectory
	}
	fmt.Fprintln(env.Stdout, memory.SearchSessions(query, *dir, *max))
	return exitOK
}

// reviewOutcome runs one review pass over a finished session and stages the
// proposals. It returns the one status line to print.
func reviewOutcome(ctx context.Context, env *Env, cwd string, sess *session.Session, client memory.Completer) (string, error) {
	proposals, err := memory.Propose(ctx, sess.ID, sess.Messages, client, nil)
	if err != nil {
		return "", err
	}
	if err := memory.QueueFromWorkspace(cwd).Add(proposals); err != nil {
		return "", err
	}
	pending, blocked := 0, 0
	for _, p := range proposals {
		if p.Status == memory.StatusBlocked {
			blocked++
		} else {
			pending++
		}
	}
	noun := "entries"
	if len(proposals) == 1 {
		noun = "entry"
	}
	return fmt.Sprintf("memory_review: proposed %d %s (%d pending, %d blocked) -- run 'sarina memory-review-list' to inspect, "+
		"'sarina memory-review-approve <id>' to apply", len(proposals), noun, pending, blocked), nil
}

func (o *opts) modelClient(env *Env) (agent.Completer, error) {
	m, _, _, err := o.build(env, nil)
	if err != nil {
		return nil, err
	}
	if env.NewClient != nil {
		return env.NewClient(m), nil
	}
	return llm.NewClient(m), nil
}

func memoryReviewCommand(env *Env, args []string) int {
	o := newOpts("memory-review", env)
	if code, ok := parseOrExit(o.fs, args); !ok {
		return code
	}
	if o.fs.NArg() != 1 {
		fmt.Fprintln(env.Stderr, "usage: sarina memory-review [flags] <session_id>")
		return exitUsage
	}
	id := o.fs.Arg(0)
	dir := o.sessionDir
	if dir == "" {
		dir = config.DefaultRuntime(o.cwd).SessionDirectory
	}
	sess, err := session.LoadByID(dir, id)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: cannot load session %s: %v\n", id, err)
		return exitFailure
	}
	client, err := o.modelClient(env)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitUsage
	}
	line, err := reviewOutcome(context.Background(), env, config.DefaultRuntime(o.cwd).CWD, sess, client)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: memory review failed: %v\n", err)
		return exitFailure
	}
	fmt.Fprintln(env.Stdout, line)
	return exitOK
}

func memoryReviewListCommand(env *Env, args []string) int {
	fs, cwd := cwdFlag("memory-review-list", env)
	if code, ok := parseOrExit(fs, args); !ok {
		return code
	}
	fmt.Fprintln(env.Stdout, memory.QueueFromWorkspace(*cwd).RenderSummary())
	return exitOK
}

func memoryReviewApproveCommand(env *Env, args []string) int {
	fs, cwd := cwdFlag("memory-review-approve", env)
	if code, ok := parseOrExit(fs, args); !ok {
		return code
	}
	if fs.NArg() != 1 {
		fmt.Fprintln(env.Stderr, "usage: sarina memory-review-approve [--cwd D] <entry_id>")
		return exitUsage
	}
	m, err := memory.QueueFromWorkspace(*cwd).Approve(fs.Arg(0), env.memoryRuntime(*cwd))
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitFailure
	}
	fmt.Fprintf(env.Stdout, "approved: %s %s memory entry (%d total)\n", m.Action, m.Scope, m.AfterCount)
	return exitOK
}

func memoryReviewRejectCommand(env *Env, args []string) int {
	fs, cwd := cwdFlag("memory-review-reject", env)
	if code, ok := parseOrExit(fs, args); !ok {
		return code
	}
	if fs.NArg() != 1 {
		fmt.Fprintln(env.Stderr, "usage: sarina memory-review-reject [--cwd D] <entry_id>")
		return exitUsage
	}
	if err := memory.QueueFromWorkspace(*cwd).Reject(fs.Arg(0)); err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitFailure
	}
	fmt.Fprintf(env.Stdout, "rejected: %s\n", fs.Arg(0))
	return exitOK
}

// autoMemoryReview is the exit-time hook of an interactive conversation: one
// review pass over the saved session, proposals staged for approval (nothing
// is written to memory). It never fails the conversation: any error is
// reported and swallowed, since the operator has already left the loop.
func autoMemoryReview(env *Env, o *opts, s *session.Session) {
	if s == nil || s.ID == "" || o.client == nil {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
	defer cancel()
	done := make(chan struct{})
	defer close(done)
	go func() {
		select {
		case <-env.Interrupts:
			cancel()
		case <-done:
		}
	}()
	line, err := reviewOutcome(ctx, env, s.Runtime.CWD, s, o.client)
	if err != nil {
		fmt.Fprintf(env.Stdout, "memory_review_skipped=error (%v)\n", err)
		return
	}
	fmt.Fprintln(env.Stdout, line)
}
