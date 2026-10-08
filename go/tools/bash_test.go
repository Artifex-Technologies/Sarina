package tools

import (
	"context"
	"os"
	"os/exec"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

func needBash(t *testing.T) {
	t.Helper()
	if _, err := shellPath(); err != nil {
		t.Skip("bash is not available: ", err)
	}
}

func shellEnv(t *testing.T) *env {
	t.Helper()
	needBash(t)
	e := newEnv(t, shellAll())
	e.tc.CommandTimeout = 20 * time.Second
	// every test gets its own process manager so nothing leaks between tests
	pm := NewProcManager()
	e.tc.Runtimes = map[string]any{RuntimeProcs: pm}
	t.Cleanup(pm.StopAll)
	return e
}

func TestBashOutputFormat(t *testing.T) {
	e := shellEnv(t)
	r := e.ok("bash", map[string]any{"command": "echo out; echo err >&2; exit 3"})
	if r.Content != "exit_code=3\n[stdout]\nout\n[stderr]\nerr" {
		t.Fatalf("content: %q", r.Content)
	}
	if r.Metadata["exit_code"] != 3 || r.Metadata["action"] != "bash" || r.Metadata["command"] != "echo out; echo err >&2; exit 3" {
		t.Fatalf("metadata: %v", r.Metadata)
	}
	for _, k := range []string{"stdout_preview", "stderr_preview", "output_preview"} {
		if _, ok := r.Metadata[k]; !ok {
			t.Errorf("missing %s", k)
		}
	}
	if r = e.ok("bash", map[string]any{"command": "pwd"}); !strings.Contains(r.Content, "exit_code=0") {
		t.Fatalf("pwd: %q", r.Content)
	}
	e.put("sub/f.txt", "x")
	if r = e.ok("bash", map[string]any{"command": "cat sub/f.txt"}); !strings.Contains(r.Content, "[stdout]\nx\n") {
		t.Fatalf("runs in the workspace: %q", r.Content)
	}
	e.tc.MaxOutputChars = 200
	if r = e.ok("bash", map[string]any{"command": "yes line | head -n 500"}); !strings.Contains(r.Content, "omitted") {
		t.Fatalf("output must be truncated: %d chars", len(r.Content))
	}
}

func TestBashDestructiveGateAndUnsafeFlag(t *testing.T) {
	e := shellEnv(t)
	e.put("victim.txt", "x")
	for _, cmd := range []string{"rm victim.txt", "echo hi && rm victim.txt", "bash -c 'rm victim.txt'", "echo $(rm victim.txt)", "sudo rm victim.txt"} {
		r := e.fails("bash", map[string]any{"command": cmd}, "permission_denied")
		if !strings.Contains(r.Content, "--unsafe") {
			t.Errorf("%q: the refusal must name --unsafe: %q", cmd, r.Content)
		}
	}
	if !e.exists("victim.txt") {
		t.Fatal("a refused command ran")
	}
	// background launches go through the same gate
	e.fails("bash", map[string]any{"command": "rm victim.txt", "run_in_background": true}, "permission_denied")
	if !e.exists("victim.txt") {
		t.Fatal("a refused background command ran")
	}
	e.tc.Permissions.AllowDestructiveShellCommands = true
	e.ok("bash", map[string]any{"command": "rm victim.txt"})
	if e.exists("victim.txt") {
		t.Fatal("--unsafe should let the command run")
	}
}

func TestBashDenialsOnTheStreamingPath(t *testing.T) {
	e := shellEnv(t)
	var got []string
	emit := func(stream, text string) { got = append(got, stream+":"+text) }

	// no shell permission
	e.tc.Permissions = config.Permissions{}
	r := e.reg.ExecuteStream(context.Background(), "bash", map[string]any{"command": "echo hi"}, e.tc, emit)
	if r.OK || r.Metadata["error_kind"] != "permission_denied" || !strings.Contains(r.Content, "--allow-shell") {
		t.Fatalf("streaming without shell permission: %+v", r)
	}
	// destructive command
	e.tc.Permissions = shellAll()
	e.put("victim.txt", "x")
	r = e.reg.ExecuteStream(context.Background(), "bash", map[string]any{"command": "rm victim.txt"}, e.tc, emit)
	if r.OK || r.Metadata["error_kind"] != "permission_denied" || !strings.Contains(r.Content, "--unsafe") {
		t.Fatalf("streaming destructive: %+v", r)
	}
	if !e.exists("victim.txt") || len(got) != 0 {
		t.Fatalf("a denied streaming call must run nothing and emit nothing: %v", got)
	}
	// non-streamed tools denied on the streaming entry point too
	e.tc.Permissions = config.Permissions{}
	r = e.reg.ExecuteStream(context.Background(), "write_file", map[string]any{"path": "n.txt", "content": "x"}, e.tc, emit)
	if r.OK || r.Metadata["error_kind"] != "permission_denied" || e.exists("n.txt") {
		t.Fatalf("write_file via ExecuteStream: %+v", r)
	}
}

func TestBashStreamsLinesAsTheyArrive(t *testing.T) {
	e := shellEnv(t)
	var mu sync.Mutex
	var got []string
	r := e.reg.ExecuteStream(context.Background(), "bash", map[string]any{"command": "echo one; echo two >&2; printf three"}, e.tc,
		func(stream, text string) { mu.Lock(); got = append(got, stream+"|"+text); mu.Unlock() })
	if !r.OK || r.Metadata["streamed"] != true {
		t.Fatalf("result: %+v", r)
	}
	joined := strings.Join(got, "")
	for _, want := range []string{"stdout|one\n", "stderr|two\n", "stdout|three"} {
		if !strings.Contains(joined, want) {
			t.Errorf("missing %q in %q", want, joined)
		}
	}
	if !strings.Contains(r.Content, "one") || !strings.Contains(r.Content, "two") {
		t.Fatalf("the final result still carries the full output: %q", r.Content)
	}
	// the non-streaming path is unchanged
	if r = e.ok("bash", map[string]any{"command": "echo hi"}); r.Metadata["streamed"] != nil {
		t.Fatalf("Execute must not mark results streamed: %v", r.Metadata)
	}
}

func TestChildEnvironmentFilter(t *testing.T) {
	e := shellEnv(t)
	parent := []string{
		"PATH=" + os.Getenv("PATH"), "HOME=" + os.Getenv("HOME"), "SYSTEMROOT=" + os.Getenv("SYSTEMROOT"),
		"OPENAI_API_KEY=sk-live-ENVVALUE1", "GITHUB_TOKEN=ghp_ENVVALUE2", "DB_PASSWORD=ENVVALUE3", "my_secret=ENVVALUE4",
		"AWS_CREDENTIALS_FILE=ENVVALUE5", "SSH_AUTH_SOCK=ENVVALUE6", "GIT_AUTHOR_NAME=ENVVALUE7", "TLS_PRIVATE_KEY=ENVVALUE8",
		"Mixed_Case_Token=ENVVALUE9", "HARMLESS=visible",
	}
	e.tc.Environ = func() []string { return parent }
	r := e.ok("bash", map[string]any{"command": "env"})
	if strings.Contains(r.Content, "ENVVALUE") {
		t.Fatalf("a secret-named variable reached the child: %q", r.Content)
	}
	if !strings.Contains(r.Content, "HARMLESS=visible") {
		t.Fatalf("ordinary variables must pass: %q", r.Content)
	}
	for _, name := range []string{"OPENAI_API_KEY", "GITHUB_TOKEN", "DB_PASSWORD", "my_secret", "SSH_AUTH_SOCK", "GIT_AUTHOR_NAME"} {
		if strings.Contains(r.Content, name+"=") {
			t.Errorf("%s must be filtered", name)
		}
	}
	// the operator's own layers are deliberate and override
	e.tc.SessionEnv = map[string]string{"SESSION_TOKEN": "from-operator", "HARMLESS": "overridden"}
	e.tc.ExtraEnv = map[string]string{"EXTRA_API_KEY": "allowlisted"}
	r = e.ok("bash", map[string]any{"command": "env"})
	for _, want := range []string{"SESSION_TOKEN=from-operator", "HARMLESS=overridden", "EXTRA_API_KEY=allowlisted"} {
		if !strings.Contains(r.Content, want) {
			t.Errorf("operator layer lost: %s in %q", want, r.Content)
		}
	}
	// the tool result and metadata never carry the filtered values or the extra values' names
	for k, v := range r.Metadata {
		if s, ok := v.(string); ok && strings.Contains(s, "ENVVALUE") {
			t.Errorf("metadata %s leaks: %q", k, s)
		}
	}
}

func TestChildEnvUnit(t *testing.T) {
	tc := &Context{Environ: func() []string {
		return []string{"A=1", "TOKEN_X=no", "=C:=C:\\dir", "NOEQUALS", "B=2=3", "authz=no"}
	}, SessionEnv: map[string]string{"A": "9"}, ExtraEnv: map[string]string{"C": "3"}}
	got := strings.Join(childEnv(tc), "|")
	for _, bad := range []string{"TOKEN_X", "authz", "NOEQUALS"} {
		if strings.Contains(got, bad) {
			t.Errorf("%s must be dropped: %s", bad, got)
		}
	}
	for _, want := range []string{"A=9", "B=2=3", "C=3"} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %s in %s", want, got)
		}
	}
}

func TestBashTimeout(t *testing.T) {
	e := shellEnv(t)
	e.tc.CommandTimeout = 400 * time.Millisecond
	start := time.Now()
	r := e.fails("bash", map[string]any{"command": "echo before; sleep 30; echo after"}, "tool_execution_error")
	if time.Since(start) > 10*time.Second {
		t.Fatalf("the timeout did not kill the command promptly: %v", time.Since(start))
	}
	if r.Metadata["timed_out"] != true || !strings.Contains(r.Content, "timed out") || !strings.Contains(r.Content, "sleep 30") {
		t.Fatalf("timeout result: %+v", r)
	}
	// a per-call override
	e.tc.CommandTimeout = time.Minute
	start = time.Now()
	e.fails("bash", map[string]any{"command": "sleep 30", "timeout_seconds": 1}, "tool_execution_error")
	if time.Since(start) > 10*time.Second {
		t.Fatal("timeout_seconds ignored")
	}
}

func TestBashCancellationKillsTheCommand(t *testing.T) {
	e := shellEnv(t)
	ctx, cancel := context.WithCancel(context.Background())
	go func() { time.Sleep(300 * time.Millisecond); cancel() }()
	start := time.Now()
	r := e.reg.Execute(ctx, "bash", map[string]any{"command": "sleep 30"}, e.tc)
	if r.OK || r.Metadata["interrupted"] != true || time.Since(start) > 10*time.Second {
		t.Fatalf("cancel: %+v after %v", r, time.Since(start))
	}
}

func TestBashSelfAuthoredBanner(t *testing.T) {
	e := shellEnv(t)
	e.ok("write_file", map[string]any{"path": "check.sh", "content": "echo checked"})
	r := e.ok("bash", map[string]any{"command": "bash check.sh"})
	if !strings.HasPrefix(r.Content, "[note:") || !strings.Contains(r.Content, "check.sh") || !strings.Contains(r.Content, "checked") {
		t.Fatalf("banner: %q", r.Content)
	}
	if r = e.ok("bash", map[string]any{"command": "echo unrelated"}); strings.Contains(r.Content, "note:") {
		t.Fatalf("an unrelated command gets no banner: %q", r.Content)
	}
}

func TestBashIsNotConfinedBySecretGuardOnPurpose(t *testing.T) {
	e := shellEnv(t)
	e.put(".env", "VISIBLE_TO_EXPLICIT_BASH")
	r := e.ok("bash", map[string]any{"command": "cat .env"})
	if !strings.Contains(r.Content, "VISIBLE_TO_EXPLICIT_BASH") {
		t.Fatalf("documented behaviour: bash reads with explicit intent: %q", r.Content)
	}
}

// ---- background processes -----------------------------------------------------------

func startBG(t *testing.T, e *env, command string) string {
	t.Helper()
	r := e.ok("bash", map[string]any{"command": command, "run_in_background": true})
	id, _ := r.Metadata["task_id"].(string)
	if !strings.HasPrefix(id, "bg_") || r.Metadata["background"] != true {
		t.Fatalf("background start: %+v", r)
	}
	return id
}

func TestBackgroundOutputAndCompletion(t *testing.T) {
	e := shellEnv(t)
	id := startBG(t, e, "echo started; sleep 1; echo finished; exit 4")

	r := e.ok("TaskOutput", map[string]any{"task_id": id, "block": false})
	if r.Metadata["retrieval_status"] != "not_ready" || r.Metadata["status"] != ProcRunning {
		t.Fatalf("non-blocking read of a running process: %+v", r)
	}
	r = e.ok("TaskOutput", map[string]any{"task_id": id, "timeout": 20000})
	if r.Metadata["retrieval_status"] != "success" || r.Metadata["status"] != ProcFailed || r.Metadata["exit_code"] != 4 {
		t.Fatalf("blocking read: %+v", r)
	}
	for _, want := range []string{"<retrieval_status>success</retrieval_status>", "<task_id>" + id + "</task_id>", "started", "finished", "<exit_code>4</exit_code>"} {
		if !strings.Contains(r.Content, want) {
			t.Errorf("missing %q in %q", want, r.Content)
		}
	}
	id2 := startBG(t, e, "echo fine")
	r = e.ok("TaskOutput", map[string]any{"task_id": id2})
	if r.Metadata["status"] != ProcCompleted || !strings.Contains(r.Content, "fine") {
		t.Fatalf("exit 0 is completed: %+v", r)
	}
}

func TestBackgroundBlockingTimeout(t *testing.T) {
	e := shellEnv(t)
	id := startBG(t, e, "echo early; sleep 30")
	start := time.Now()
	r := e.ok("TaskOutput", map[string]any{"task_id": id, "timeout": 700})
	if r.Metadata["retrieval_status"] != "timeout" || r.Metadata["status"] != ProcRunning || time.Since(start) > 10*time.Second {
		t.Fatalf("timeout: %+v", r)
	}
	if !strings.Contains(r.Content, "early") {
		t.Fatalf("a timeout still returns the output so far: %q", r.Content)
	}
}

func TestTaskStopKillsTheRealProcess(t *testing.T) {
	e := shellEnv(t)
	id := startBG(t, e, "echo up; sleep 60")
	pm := e.tc.Runtimes[RuntimeProcs].(*ProcManager)
	p, ok := pm.Get(id)
	if !ok {
		t.Fatal("process not registered")
	}
	for i := 0; i < 100; i++ { // let it print before it is stopped
		if _, _, out := p.Snapshot(); strings.Contains(out, "up") {
			break
		}
		time.Sleep(50 * time.Millisecond)
	}
	if st, _, _ := p.Snapshot(); st != ProcRunning {
		t.Fatalf("status before stop: %s", st)
	}
	r := e.ok("TaskStop", map[string]any{"task_id": id})
	if r.Metadata["status"] != ProcKilled || r.Metadata["action"] != "task_stop" || !strings.Contains(r.Content, id) {
		t.Fatalf("stop: %+v", r)
	}
	select {
	case <-p.Done():
	default:
		t.Fatal("TaskStop returned while the process was still running")
	}
	// it is really gone
	if proc, err := os.FindProcess(p.PID); err == nil && processAlive(proc) {
		t.Fatalf("pid %d is still alive after TaskStop", p.PID)
	}
	// stopping again is an error and a finished process reads as killed
	e.fails("TaskStop", map[string]any{"task_id": id}, "tool_execution_error")
	r = e.ok("TaskOutput", map[string]any{"task_id": id})
	if r.Metadata["status"] != ProcKilled || !strings.Contains(r.Content, "up") {
		t.Fatalf("output after stop: %+v", r)
	}
}

func TestTaskToolsAreNotTodoTools(t *testing.T) {
	e := shellEnv(t)
	e.ok("task_create", map[string]any{"title": "write docs", "task_id": "t1"})
	for _, name := range []string{"TaskOutput", "TaskStop"} {
		r := e.fails(name, map[string]any{"task_id": "t1"}, "tool_execution_error")
		if !strings.Contains(r.Content, "bg_") {
			t.Errorf("%s: the error should explain what ids look like: %q", name, r.Content)
		}
	}
	e.fails("TaskOutput", map[string]any{"task_id": "bg_nope"}, "tool_execution_error")
	// and TaskStop must not have touched the to-do list
	if r := e.ok("task_list", map[string]any{"task_id": "t1"}); !strings.Contains(r.Content, "status: pending") {
		t.Fatalf("the to-do task changed: %q", r.Content)
	}
}

func TestStopAllAndProcessLimit(t *testing.T) {
	e := shellEnv(t)
	pm := e.tc.Runtimes[RuntimeProcs].(*ProcManager)
	var ids []string
	for i := 0; i < 3; i++ {
		ids = append(ids, startBG(t, e, "sleep 60"))
	}
	pm.StopAll()
	for _, id := range ids {
		p, _ := pm.Get(id)
		if st, _, _ := p.Snapshot(); st != ProcKilled {
			t.Errorf("%s is %s after StopAll", id, st)
		}
	}
	for i := 0; i < maxBackgroundProcs; i++ {
		startBG(t, e, "sleep 60")
	}
	e.fails("bash", map[string]any{"command": "sleep 60", "run_in_background": true}, "tool_execution_error")
}

func TestBackgroundEnvironmentIsFilteredToo(t *testing.T) {
	e := shellEnv(t)
	e.tc.Environ = func() []string {
		return []string{"PATH=" + os.Getenv("PATH"), "SYSTEMROOT=" + os.Getenv("SYSTEMROOT"), "BG_TOKEN=ENVVALUE", "OKVAR=1"}
	}
	id := startBG(t, e, "env")
	r := e.ok("TaskOutput", map[string]any{"task_id": id})
	if strings.Contains(r.Content, "ENVVALUE") || !strings.Contains(r.Content, "OKVAR=1") {
		t.Fatalf("background env: %q", r.Content)
	}
}

func processAlive(p *os.Process) bool {
	if p == nil {
		return false
	}
	return processAliveOS(p)
}

var _ = exec.Command
