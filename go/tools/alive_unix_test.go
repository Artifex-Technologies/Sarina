//go:build !windows

package tools

import (
	"os"
	"os/exec"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func processAliveOS(p *os.Process) bool { return p.Signal(syscall.Signal(0)) == nil }

// A timed-out command must take everything it started with it, not just the shell.
func TestTimeoutKillsGrandchildren(t *testing.T) {
	e := shellEnv(t)
	e.tc.CommandTimeout = 500 * time.Millisecond
	pidFile := e.root + "/child.pid"
	e.fails("bash", map[string]any{"command": "(sleep 120 & echo $! > child.pid; wait)"}, "tool_execution_error")
	time.Sleep(200 * time.Millisecond)
	raw, err := os.ReadFile(pidFile)
	if err != nil {
		t.Fatal(err)
	}
	pid, _ := strconv.Atoi(strings.TrimSpace(string(raw)))
	if pid <= 0 {
		t.Fatalf("bad pid %q", raw)
	}
	if proc, _ := os.FindProcess(pid); processAliveOS(proc) {
		_ = proc.Kill()
		t.Fatalf("grandchild %d survived the timeout", pid)
	}
}

// A foreground command must not leave a background job behind either.
func TestForegroundCommandLeavesNoStrays(t *testing.T) {
	e := shellEnv(t)
	e.ok("bash", map[string]any{"command": "sleep 120 & echo $! > stray.pid; true"})
	time.Sleep(200 * time.Millisecond)
	raw, _ := os.ReadFile(e.root + "/stray.pid")
	pid, _ := strconv.Atoi(strings.TrimSpace(string(raw)))
	if pid <= 0 {
		t.Fatalf("bad pid %q", raw)
	}
	if proc, _ := os.FindProcess(pid); processAliveOS(proc) {
		_ = proc.Kill()
		t.Fatalf("stray process %d survived", pid)
	}
}

// TaskStop takes down the whole tree, including children of the shell.
func TestTaskStopKillsChildren(t *testing.T) {
	e := shellEnv(t)
	id := startBG(t, e, "sleep 120 & echo $! > kid.pid; wait")
	time.Sleep(500 * time.Millisecond)
	e.ok("TaskStop", map[string]any{"task_id": id})
	time.Sleep(200 * time.Millisecond)
	raw, _ := os.ReadFile(e.root + "/kid.pid")
	pid, _ := strconv.Atoi(strings.TrimSpace(string(raw)))
	if pid <= 0 {
		t.Fatalf("bad pid %q", raw)
	}
	if proc, _ := os.FindProcess(pid); processAliveOS(proc) {
		_ = proc.Kill()
		t.Fatalf("child %d survived TaskStop", pid)
	}
}

var _ = exec.Command
