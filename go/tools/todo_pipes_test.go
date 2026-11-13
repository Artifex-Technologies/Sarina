//go:build unix

package tools

import (
	"os"
	"path/filepath"
	"runtime"
	"syscall"
	"testing"
	"time"
)

// A named pipe planted under the to-do list's or the plan's file name is not waited on: before, every to-do and plan
// tool opened them with os.ReadFile and waited for a writer for ever (the session and its turn held).
func TestTodoToolsDoNotWaitOnNamedPipes(t *testing.T) {
	for _, call := range []struct {
		name string
		args map[string]any
		ok   bool // a pipe reads as no list: updating a task fails as for any unknown id
	}{
		{"task_list", map[string]any{}, true},
		{"task_create", map[string]any{"title": "design", "task_id": "a"}, true},
		{"task_update", map[string]any{"task_id": "a", "status": "completed"}, false},
		{"todo_write", map[string]any{"items": []any{map[string]any{"title": "one"}}}, true},
		{"plan_get", map[string]any{}, true},
		{"update_plan", map[string]any{"items": []any{map[string]any{"step": "one", "status": "pending"}}}, true},
	} {
		t.Run(call.name, func(t *testing.T) {
			e := newEnv(t, writeAll())
			dir := filepath.Join(e.root, ".port_sessions")
			if err := os.MkdirAll(dir, 0o755); err != nil {
				t.Fatal(err)
			}
			pipes := []string{filepath.Join(dir, taskFileName), filepath.Join(dir, planFileName)}
			for _, p := range pipes {
				if err := syscall.Mkfifo(p, 0o644); err != nil {
					t.Skipf("no named pipes here: %v", err)
				}
			}
			done := make(chan Result, 1)
			go func() { done <- e.run(call.name, call.args) }()
			select {
			case r := <-done:
				if r.OK != call.ok {
					t.Fatalf("%s: ok %v, want %v: %s", call.name, r.OK, call.ok, r.Content)
				}
			case <-time.After(3 * time.Second):
				// let the stuck readers go (writers that close at once, as often as a reader waits), so the test ends
				stop := time.After(10 * time.Second)
				for {
					for _, p := range pipes {
						if w, err := os.OpenFile(p, os.O_WRONLY|syscall.O_NONBLOCK, 0); err == nil {
							w.Close()
						}
					}
					select {
					case <-done:
						t.Fatalf("%s waited on a named pipe", call.name)
					case <-stop:
						t.Fatalf("%s waited on a named pipe and was still waiting", call.name)
					case <-time.After(50 * time.Millisecond):
					}
				}
			}
		})
	}
}

// A to-do list or plan file larger than any real one is not read: before, os.ReadFile read the whole file to find it
// was not a list (a sparse file of gigabytes costs nothing on disk).
func TestTodoToolsDoNotReadHugeFiles(t *testing.T) {
	e := newEnv(t, writeAll())
	dir := filepath.Join(e.root, ".port_sessions")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{taskFileName, planFileName} {
		f, err := os.Create(filepath.Join(dir, name))
		if err != nil {
			t.Fatal(err)
		}
		if err := f.Truncate(80 << 20); err != nil {
			t.Fatal(err)
		}
		f.Close()
	}
	for _, name := range []string{"task_list", "plan_get"} {
		var before, after runtime.MemStats
		runtime.ReadMemStats(&before)
		e.ok(name, map[string]any{})
		runtime.ReadMemStats(&after)
		if got := after.TotalAlloc - before.TotalAlloc; got > 8<<20 {
			t.Fatalf("%s allocated %d bytes for an 80 MiB file", name, got)
		}
	}
}
