package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

type backend struct {
	*httptest.Server
	mu      sync.Mutex
	reqs    []map[string]any
	auth    []string
	replies []string
}

func newBackend(t *testing.T, replies ...string) *backend {
	b := &backend{replies: replies}
	b.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		var req map[string]any
		json.Unmarshal(body, &req)
		b.mu.Lock()
		b.reqs = append(b.reqs, req)
		b.auth = append(b.auth, r.Header.Get("Authorization"))
		i := len(b.reqs) - 1
		b.mu.Unlock()
		if req["stream"] == true {
			w.Header().Set("Content-Type", "text/event-stream")
			text := "streamed answer"
			for _, part := range []string{text[:8], text[8:]} {
				j, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"content": part}}}})
				fmt.Fprintf(w, "data: %s\n\n", j)
			}
			fmt.Fprint(w, "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
			return
		}
		if i >= len(b.replies) {
			w.WriteHeader(400)
			io.WriteString(w, `{"error":"no more replies"}`)
			return
		}
		j, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{
			"message": map[string]any{"role": "assistant", "content": b.replies[i]}, "finish_reason": "stop"}},
			"usage": map[string]any{"prompt_tokens": 5, "completion_tokens": 2}})
		w.Write(j)
	}))
	t.Cleanup(b.Close)
	return b
}

func (b *backend) count() int { b.mu.Lock(); defer b.mu.Unlock(); return len(b.reqs) }

type harness struct {
	out, errb bytes.Buffer
	env       Env
	dir       string
	ints      chan os.Signal
}

func newHarness(t *testing.T, stdin io.Reader) *harness {
	h := &harness{dir: t.TempDir(), ints: make(chan os.Signal, 1)}
	h.env = Env{Stdin: stdin, Stdout: &h.out, Stderr: &h.errb, Interrupts: h.ints,
		Getenv: func(k string) string {
			if k == "SARINA_CONTEXT_DISCOVERY" {
				return "0" // these fake backends count requests; discovery has its own tests
			}
			return ""
		}, Home: t.TempDir()}
	return h
}

func (h *harness) common(b *backend) []string {
	// the exit-time memory review is on by default in the CLI; most tests count requests, so they turn it off
	return append(h.commonAuto(b), "--no-auto-memory-review")
}

func (h *harness) commonAuto(b *backend) []string {
	return []string{"--base-url", b.URL, "--cwd", h.dir, "--disable-claude-md", "--api-key", "cli-test-key-31337"}
}

func (h *harness) run(args ...string) int { return Main(args, h.env) }

func (h *harness) sessionFiles(t *testing.T) []string {
	files, _ := filepath.Glob(filepath.Join(h.dir, ".port_sessions", "agent", "*.json"))
	return files
}

func TestOneShotChat(t *testing.T) {
	b := newBackend(t, "the answer")
	h := newHarness(t, strings.NewReader(""))
	code := h.run(append([]string{"chat"}, append(h.common(b), "what", "is", "up")...)...)
	if code != 0 || strings.TrimSpace(h.out.String()) != "the answer" {
		t.Fatalf("code=%d out=%q err=%q", code, h.out.String(), h.errb.String())
	}
	if b.auth[0] != "Bearer cli-test-key-31337" {
		t.Fatalf("auth = %q", b.auth[0])
	}
	files := h.sessionFiles(t)
	if len(files) != 1 {
		t.Fatalf("session files = %v", files)
	}
	data, _ := os.ReadFile(files[0])
	if strings.Contains(string(data), "cli-test-key-31337") {
		t.Fatal("API key leaked into the session file")
	}
	if !strings.Contains(string(data), "what is up") {
		t.Fatal("prompt words must be joined")
	}
}

func TestInteractiveLoopResumesSession(t *testing.T) {
	b := newBackend(t, "answer one", "answer two")
	h := newHarness(t, strings.NewReader("first question\n\nsecond question\n/exit\nignored after exit\n"))
	code := h.run(append([]string{"chat"}, h.common(b)...)...)
	if code != 0 {
		t.Fatalf("code=%d err=%q", code, h.errb.String())
	}
	if b.count() != 2 {
		t.Fatalf("requests = %d (blank lines and text after /exit must not call the model)", b.count())
	}
	out := h.out.String()
	if !strings.Contains(out, "answer one") || !strings.Contains(out, "answer two") {
		t.Fatalf("out = %q", out)
	}
	msgs, _ := json.Marshal(b.reqs[1]["messages"])
	for _, want := range []string{"first question", "answer one", "second question"} {
		if !strings.Contains(string(msgs), want) {
			t.Fatalf("second request lacks %q", want)
		}
	}
	if len(h.sessionFiles(t)) != 1 {
		t.Fatalf("the conversation must stay in one session file: %v", h.sessionFiles(t))
	}
}

func TestInteractiveEOFExitsCleanly(t *testing.T) {
	b := newBackend(t, "hi")
	h := newHarness(t, strings.NewReader("hello"))
	if code := h.run(append([]string{"chat"}, h.common(b)...)...); code != 0 {
		t.Fatalf("code=%d", code)
	}
	if b.count() != 1 {
		t.Fatalf("a final line without newline must still run: %d", b.count())
	}
}

type blockingClient struct {
	once    sync.Once
	started chan struct{}
}

func (c *blockingClient) Complete(ctx context.Context, _ llm.Request, _ func(llm.StreamEvent)) (*llm.Turn, error) {
	c.once.Do(func() { close(c.started) })
	<-ctx.Done()
	return nil, ctx.Err()
}

func TestCtrlCStopsARunningOneShot(t *testing.T) {
	bc := &blockingClient{started: make(chan struct{})}
	h := newHarness(t, strings.NewReader(""))
	h.env.NewClient = func(config.ModelConfig) agent.Completer { return bc }
	go func() { <-bc.started; h.ints <- os.Interrupt }()
	done := make(chan int)
	go func() { done <- h.run("chat", "--cwd", h.dir, "--disable-claude-md", "long task") }()
	select {
	case code := <-done:
		if code != 130 {
			t.Fatalf("code = %d, want 130", code)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Ctrl-C did not stop the run")
	}
	if len(h.sessionFiles(t)) != 1 {
		t.Fatal("an interrupted run must still save its session")
	}
	if !strings.Contains(h.errb.String(), "interrupted") {
		t.Fatalf("stderr = %q", h.errb.String())
	}
}

func TestCtrlCWhileIdleEndsInteractiveLoop(t *testing.T) {
	b := newBackend(t)
	pr, pw := io.Pipe()
	defer pw.Close()
	h := newHarness(t, pr)
	done := make(chan int)
	go func() { done <- h.run(append([]string{"chat"}, h.common(b)...)...) }()
	time.Sleep(50 * time.Millisecond)
	h.ints <- os.Interrupt
	select {
	case code := <-done:
		if code != 130 {
			t.Fatalf("code = %d", code)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("idle Ctrl-C did not exit")
	}
}

func TestCtrlCInterruptsTurnButConversationContinues(t *testing.T) {
	bc := &blockingClient{started: make(chan struct{})}
	pr, pw := io.Pipe()
	h := newHarness(t, pr)
	h.env.NewClient = func(config.ModelConfig) agent.Completer { return bc }
	done := make(chan int)
	go func() { done <- h.run("chat", "--cwd", h.dir, "--disable-claude-md", "--no-auto-memory-review") }()
	go func() {
		io.WriteString(pw, "slow request\n")
		<-bc.started
		h.ints <- os.Interrupt
		time.Sleep(100 * time.Millisecond)
		io.WriteString(pw, "/exit\n")
	}()
	select {
	case code := <-done:
		if code != 0 {
			t.Fatalf("code = %d (an interrupted turn is not a failure of the whole conversation)", code)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("loop hung")
	}
	if !strings.Contains(h.errb.String(), "interrupted") {
		t.Fatalf("stderr = %q", h.errb.String())
	}
}

func TestResumeCommandAndPermissionWidening(t *testing.T) {
	b := newBackend(t, "one", "two")
	h := newHarness(t, strings.NewReader(""))
	if code := h.run(append([]string{"chat", "--allow-write"}, append(h.common(b), "start")...)...); code != 0 {
		t.Fatalf("code=%d err=%s", code, h.errb.String())
	}
	files := h.sessionFiles(t)
	id := strings.TrimSuffix(filepath.Base(files[0]), ".json")
	h.out.Reset()
	code := h.run("resume", "--base-url", b.URL, "--cwd", h.dir, id, "and", "more")
	if code != 0 || strings.TrimSpace(h.out.String()) != "two" {
		t.Fatalf("code=%d out=%q err=%q", code, h.out.String(), h.errb.String())
	}
	msgs, _ := json.Marshal(b.reqs[1]["messages"])
	if !strings.Contains(string(msgs), "start") || !strings.Contains(string(msgs), "and more") {
		t.Fatalf("history not carried: %s", msgs)
	}
	var saved map[string]any
	data, _ := os.ReadFile(files[0])
	json.Unmarshal(data, &saved)
	rc := saved["runtime_config"].(map[string]any)
	if rc["permissions"].(map[string]any)["allow_file_write"] != true {
		t.Fatalf("permissions must never be narrowed by a resume: %v", rc["permissions"])
	}
	if code := h.run("resume", "--cwd", h.dir, "deadbeef", "x"); code == 0 {
		t.Fatal("unknown session must fail")
	}
	if code := h.run("resume"); code != 2 {
		t.Fatalf("missing id = %d", code)
	}
}

func TestNoToolsFlagOmitsTools(t *testing.T) {
	b := newBackend(t, "plain")
	h := newHarness(t, strings.NewReader(""))
	h.run(append([]string{"chat", "--no-tools"}, append(h.common(b), "hi")...)...)
	if _, ok := b.reqs[0]["tools"]; ok {
		t.Fatal("--no-tools must omit tools")
	}
	b2 := newBackend(t, "plain")
	h2 := newHarness(t, strings.NewReader(""))
	h2.run(append([]string{"chat", "--tools", "read_file,nonexistent"}, append(h2.common(b2), "hi")...)...)
	if specs := b2.reqs[0]["tools"].([]any); len(specs) != 1 {
		t.Fatalf("allowlist = %v", specs)
	}
	b3 := newBackend(t, "plain")
	h3 := newHarness(t, strings.NewReader(""))
	h3.run(append([]string{"chat", "--tools", ""}, append(h3.common(b3), "hi")...)...)
	if _, ok := b3.reqs[0]["tools"]; ok {
		t.Fatal("an explicitly empty --tools must mean no tools")
	}
}

func TestStreamFlagPrintsDeltas(t *testing.T) {
	b := newBackend(t)
	h := newHarness(t, strings.NewReader(""))
	code := h.run(append([]string{"chat", "--stream"}, append(h.common(b), "go")...)...)
	if code != 0 || strings.TrimSpace(h.out.String()) != "streamed answer" {
		t.Fatalf("code=%d out=%q", code, h.out.String())
	}
}

func TestBackendFailureExitCode(t *testing.T) {
	b := newBackend(t) // no replies: every call is a 400
	h := newHarness(t, strings.NewReader(""))
	code := h.run(append([]string{"chat"}, append(h.common(b), "go")...)...)
	if code != 1 || !strings.Contains(h.out.String(), "no more replies") {
		t.Fatalf("code=%d out=%q", code, h.out.String())
	}
}

func TestBudgetAndPromptFlagsReachTheAgent(t *testing.T) {
	b := newBackend(t, "x")
	h := newHarness(t, strings.NewReader(""))
	h.run(append([]string{"chat", "--max-output-tokens", "321", "--append-system-prompt", "EXTRA-RULE", "--max-turns", "3"},
		append(h.common(b), "go")...)...)
	if mt, _ := b.reqs[0]["max_tokens"].(float64); mt != 321 {
		t.Fatalf("max_tokens = %v", b.reqs[0]["max_tokens"])
	}
	msgs, _ := json.Marshal(b.reqs[0]["messages"])
	if !strings.Contains(string(msgs), "EXTRA-RULE") {
		t.Fatal("--append-system-prompt ignored")
	}
}

func TestCommandTable(t *testing.T) {
	h := newHarness(t, strings.NewReader(""))
	if code := h.run(); code != 2 || !strings.Contains(h.out.String(), "Commands:") {
		t.Fatalf("no args: %d %q", code, h.out.String())
	}
	h.out.Reset()
	if code := h.run("--help"); code != 0 || !strings.Contains(h.out.String(), "chat") || !strings.Contains(h.out.String(), "resume") {
		t.Fatalf("help: %d %q", code, h.out.String())
	}
	if code := h.run("frobnicate"); code != 2 || !strings.Contains(h.errb.String(), "unknown command") {
		t.Fatalf("unknown: %d %q", code, h.errb.String())
	}

	// extension point: another package adds a command by name
	var got []string
	Register("limbo-profiles-test", "test command", func(env *Env, args []string) int {
		got = args
		fmt.Fprintln(env.Stdout, "ran")
		return 7
	})
	t.Cleanup(func() { mu.Lock(); delete(commands, "limbo-profiles-test"); mu.Unlock() })
	h.out.Reset()
	if code := h.run("limbo-profiles-test", "--opt", "pacman", "-Syu"); code != 7 || strings.Join(got, " ") != "--opt pacman -Syu" || h.out.String() != "ran\n" {
		t.Fatalf("dispatch: code=%d args=%v", code, got)
	}
	if code := h.run("chat", "--no-such-flag"); code != 2 {
		t.Fatalf("bad flag = %d", code)
	}
}

func TestPermissionFlagsReachTheRuntime(t *testing.T) {
	h := newHarness(t, strings.NewReader(""))
	for _, c := range []struct {
		flags            []string
		write, shell, un bool
	}{
		{nil, false, false, false},
		{[]string{"--allow-write"}, true, false, false},
		{[]string{"--allow-shell"}, false, true, false},
		{[]string{"--allow-shell", "--unsafe", "--allow-write"}, true, true, true},
	} {
		o := newOpts("chat", &h.env)
		if err := o.fs.Parse(c.flags); err != nil {
			t.Fatal(err)
		}
		_, rt, _, err := o.build(&h.env, nil)
		if err != nil {
			t.Fatal(err)
		}
		p := rt.Permissions
		if p.AllowFileWrite != c.write || p.AllowShellCommands != c.shell || p.AllowDestructiveShellCommands != c.un {
			t.Errorf("%v -> %+v", c.flags, p)
		}
	}
}

func TestContextManagementFlagsReachTheRuntime(t *testing.T) {
	h := newHarness(t, strings.NewReader(""))
	build := func(flags ...string) config.RuntimeConfig {
		o := newOpts("chat", &h.env)
		if err := o.fs.Parse(flags); err != nil {
			t.Fatal(err)
		}
		_, rt, _, err := o.build(&h.env, nil)
		if err != nil {
			t.Fatal(err)
		}
		return rt
	}
	if rt := build(); rt.ContextManagement != nil {
		t.Fatalf("defaults must store nothing: %+v", rt.ContextManagement)
	}
	rt := build("--no-session-notes", "--no-clear-tool-outputs", "--no-llm-compaction", "--no-context-discovery", "--notes-every", "3")
	want := config.ContextConfig{DisableNotes: true, DisableClearing: true, DisableLLMCompaction: true, DisableDiscovery: true, NotesEveryTurns: 3}
	if rt.ContextManagement == nil || *rt.ContextManagement != want {
		t.Fatalf("got %+v", rt.ContextManagement)
	}
	// a resumed session keeps what it was started with, and a flag only adds to it
	stored := &session.Session{ID: "x", Model: config.ModelConfig{Model: "m"}, Runtime: rt}
	o := newOpts("resume", &h.env)
	if err := o.fs.Parse(nil); err != nil {
		t.Fatal(err)
	}
	_, rt2, _, _ := o.build(&h.env, stored)
	if rt2.ContextManagement == nil || *rt2.ContextManagement != want {
		t.Fatalf("resume lost the settings: %+v", rt2.ContextManagement)
	}
}
