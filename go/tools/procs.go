package tools

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// Background process states.
const (
	ProcRunning   = "running"
	ProcCompleted = "completed" // exited 0
	ProcFailed    = "failed"    // exited non-zero
	ProcKilled    = "killed"    // stopped by TaskStop
)

const maxBackgroundProcs = 16

// BackgroundProc is one process started with bash run_in_background.
type BackgroundProc struct {
	ID      string
	Command string
	PID     int
	Started time.Time

	mu       sync.Mutex
	out      capBuffer
	status   string
	exitCode int
	finished time.Time
	stopped  bool
	done     chan struct{}
}

// Snapshot returns the current state and output.
func (p *BackgroundProc) Snapshot() (status string, exitCode int, output string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.status, p.exitCode, p.out.String()
}

// Done is closed when the process has exited.
func (p *BackgroundProc) Done() <-chan struct{} { return p.done }

// ProcManager owns the background processes of one agent.
type ProcManager struct {
	mu    sync.Mutex
	procs map[string]*BackgroundProc
}

// NewProcManager returns an empty manager.
func NewProcManager() *ProcManager { return &ProcManager{procs: map[string]*BackgroundProc{}} }

var defaultProcs = NewProcManager()

// DefaultProcManager is the manager used when a context carries none.
func DefaultProcManager() *ProcManager { return defaultProcs }

func procManager(tc *Context) *ProcManager {
	runtimesMu.Lock()
	defer runtimesMu.Unlock()
	if m, ok := tc.Runtimes[RuntimeProcs].(*ProcManager); ok {
		return m
	}
	return defaultProcs
}

func newProcID() string {
	b := make([]byte, 4)
	_, _ = rand.Read(b)
	return "bg_" + hex.EncodeToString(b)
}

// Start launches command detached, in its own process group, with the same
// filtered environment as a foreground command. It outlives the tool call.
func (m *ProcManager) Start(command string, tc *Context) (*BackgroundProc, error) {
	m.mu.Lock()
	running := 0
	for _, p := range m.procs {
		if s, _, _ := p.Snapshot(); s == ProcRunning {
			running++
		}
	}
	m.mu.Unlock()
	if running >= maxBackgroundProcs {
		return nil, fmt.Errorf("%d background processes are already running; stop some with TaskStop first", running)
	}
	cmd, err := newShellCmd(command, tc)
	if err != nil {
		return nil, err
	}
	p := &BackgroundProc{ID: newProcID(), Command: command, Started: time.Now(), status: ProcRunning, done: make(chan struct{})}
	cmd.Stdout, cmd.Stderr = &p.out, &p.out
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("cannot start the process: %v", err)
	}
	p.PID = cmd.Process.Pid
	m.mu.Lock()
	m.procs[p.ID] = p
	m.mu.Unlock()
	go func() {
		_ = cmd.Wait()
		reapGroup(p.PID)
		p.mu.Lock()
		p.exitCode = -1
		if cmd.ProcessState != nil {
			p.exitCode = cmd.ProcessState.ExitCode()
		}
		switch {
		case p.stopped:
			p.status = ProcKilled
		case p.exitCode == 0:
			p.status = ProcCompleted
		default:
			p.status = ProcFailed
		}
		p.finished = time.Now()
		p.mu.Unlock()
		close(p.done)
	}()
	return p, nil
}

// Get returns a process by id.
func (m *ProcManager) Get(id string) (*BackgroundProc, bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	p, ok := m.procs[id]
	return p, ok
}

// IDs lists known process ids, oldest first.
func (m *ProcManager) IDs() []string {
	m.mu.Lock()
	defer m.mu.Unlock()
	ids := make([]string, 0, len(m.procs))
	for id := range m.procs {
		ids = append(ids, id)
	}
	sort.Slice(ids, func(i, j int) bool { return m.procs[ids[i]].Started.Before(m.procs[ids[j]].Started) })
	return ids
}

// Stop terminates a running process and everything it started: a polite stop
// first, then a forced kill after a short grace. It reports the final state.
func (m *ProcManager) Stop(id string) (*BackgroundProc, error) {
	p, ok := m.Get(id)
	if !ok {
		return nil, noProcError(id)
	}
	if s, _, _ := p.Snapshot(); s != ProcRunning {
		return p, fmt.Errorf("background process %s is not running (it %s)", id, s)
	}
	p.mu.Lock()
	p.stopped = true
	p.mu.Unlock()
	softStop(p.PID)
	select {
	case <-p.done:
	case <-time.After(2 * time.Second):
		hardKill(p.PID)
		select {
		case <-p.done:
		case <-time.After(3 * time.Second):
			return p, fmt.Errorf("background process %s did not exit after being killed", id)
		}
	}
	// A group member may have survived the polite stop after the leader left.
	reapGroup(p.PID)
	return p, nil
}

// StopAll stops every running process; call it when the agent exits.
func (m *ProcManager) StopAll() {
	var wg sync.WaitGroup
	for _, id := range m.IDs() {
		wg.Add(1)
		go func(id string) {
			defer wg.Done()
			_, _ = m.Stop(id)
		}(id)
	}
	wg.Wait()
}

// StopAllBackground stops every process in the default manager.
func StopAllBackground() { defaultProcs.StopAll() }

// ---- tools -------------------------------------------------------------------

// TaskOutput reads a background process's output.
func TaskOutput() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "TaskOutput",
			Description: "Get output and status of a background bash process.",
			Parameters: obj(map[string]any{
				"task_id": prop("string", "", "minLength", 1),
				"block":   prop("boolean", "Default true."),
				"timeout": prop("integer", "Milliseconds.", "minimum", 0, "maximum", 600000),
			}, "task_id"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			id := argStr(args, "task_id")
			p, ok := procManager(tc).Get(id)
			if !ok {
				return "", nil, noProcError(id)
			}
			retrieval := "success"
			if st, _, _ := p.Snapshot(); st == ProcRunning {
				if !argBool(args, "block", true) {
					retrieval = "not_ready"
				} else {
					wait := time.Duration(argInt(args, "timeout", 30000)) * time.Millisecond
					select {
					case <-p.Done():
					case <-time.After(wait):
						retrieval = "timeout"
					case <-ctx.Done():
						retrieval = "timeout"
					}
				}
			}
			status, code, output := p.Snapshot()
			var b strings.Builder
			fmt.Fprintf(&b, "<retrieval_status>%s</retrieval_status>\n<task_id>%s</task_id>\n<status>%s</status>\n", retrieval, p.ID, status)
			if status != ProcRunning {
				fmt.Fprintf(&b, "<exit_code>%d</exit_code>\n", code)
			}
			fmt.Fprintf(&b, "<output>\n%s\n</output>", strings.TrimRight(output, "\r\n"))
			md := map[string]any{"action": "task_output", "task_id": p.ID, "status": status, "retrieval_status": retrieval}
			if status != ProcRunning {
				md["exit_code"] = code
			}
			return TruncateOutput(b.String(), tc.MaxOutputChars), md, nil
		},
	}
}

// TaskStop stops a background process.
func TaskStop() Tool {
	return Tool{
		Requires: PermShell, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "TaskStop",
			Description: "Stop a background bash process.",
			Parameters: obj(map[string]any{
				"task_id": prop("string", "", "minLength", 1),
			}, "task_id"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			id := argStr(args, "task_id")
			p, err := procManager(tc).Stop(id)
			if err != nil {
				return "", nil, err
			}
			status, code, _ := p.Snapshot()
			return fmt.Sprintf("Stopped background process %s (pid %d); it is now %s.", p.ID, p.PID, status),
				map[string]any{"action": "task_stop", "task_id": p.ID, "status": status, "exit_code": code}, nil
		},
	}
}

func noProcError(id string) error {
	return fmt.Errorf("no background process %q. Ids look like bg_xxxxxxxx and come from bash with run_in_background; to-do items are separate and use the task_* tools", id)
}
