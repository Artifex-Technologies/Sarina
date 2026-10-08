package service

import (
	"bytes"
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

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// fakeModel is an OpenAI-compatible backend that answers every request with
// the same text and records what it was sent.
type fakeModel struct {
	*httptest.Server
	mu   sync.Mutex
	reqs []map[string]any
	auth []string
}

func newFakeModel(t *testing.T) *fakeModel {
	t.Helper()
	f := &fakeModel{}
	f.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var req map[string]any
		_ = json.Unmarshal(b, &req)
		f.mu.Lock()
		f.reqs = append(f.reqs, req)
		f.auth = append(f.auth, r.Header.Get("Authorization"))
		n := len(f.reqs)
		f.mu.Unlock()
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(map[string]any{
			"choices": []any{map[string]any{"message": map[string]any{"role": "assistant", "content": fmt.Sprintf("answer %d", n)}, "finish_reason": "stop"}},
			"usage":   map[string]any{"prompt_tokens": 10, "completion_tokens": 2},
		})
	}))
	t.Cleanup(f.Close)
	return f
}

func (f *fakeModel) requests() []map[string]any {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]map[string]any(nil), f.reqs...)
}

type rig struct {
	t     *testing.T
	srv   *Server
	ts    *httptest.Server
	model *fakeModel
	work  string
	token string
}

func newRig(t *testing.T, mut ...func(*Config)) *rig {
	t.Helper()
	model := newFakeModel(t)
	root := t.TempDir()
	work := filepath.Join(root, "work")
	if err := os.MkdirAll(work, 0o755); err != nil {
		t.Fatal(err)
	}
	cfg := Config{
		Model: "test-model", BaseURL: model.URL, APIKey: "service-secret-key-123",
		SessionDir: filepath.Join(root, "state", "sessions"), ScratchpadRoot: filepath.Join(root, "state", "scratch"),
		Home: root, Getenv: func(string) string { return "" },
	}
	for _, m := range mut {
		m(&cfg)
	}
	srv := New(cfg)
	ts := httptest.NewServer(srv.Handler())
	t.Cleanup(ts.Close)
	return &rig{t: t, srv: srv, ts: ts, model: model, work: work, token: cfg.Token}
}

func (r *rig) do(method, path, token string, body any) (int, map[string]any) {
	r.t.Helper()
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	}
	req, _ := http.NewRequest(method, r.ts.URL+path, rd)
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		r.t.Fatal(err)
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	var out map[string]any
	_ = json.Unmarshal(raw, &out)
	if out == nil {
		out = map[string]any{"_raw": string(raw)}
	}
	return resp.StatusCode, out
}

func (r *rig) create(extra map[string]any) (int, map[string]any) {
	body := map[string]any{"cwd": r.work}
	for k, v := range extra {
		body[k] = v
	}
	return r.do("POST", "/v1/sessions", r.token, body)
}

func (r *rig) mustCreate(extra map[string]any) string {
	r.t.Helper()
	code, out := r.create(extra)
	if code != 200 {
		r.t.Fatalf("create: %d %v", code, out)
	}
	return out["session_id"].(string)
}

func (r *rig) chat(id string, body map[string]any) (int, map[string]any) {
	return r.do("POST", "/v1/sessions/"+id+"/chat", r.token, body)
}

func hasKey(m map[string]any, k string) bool { _, ok := m[k]; return ok }

// ---- basics -----------------------------------------------------------------------

func TestHealthzNeedsNoTokenAndCountsSessions(t *testing.T) {
	r := newRig(t, func(c *Config) { c.Token = "tok" })
	code, out := r.do("GET", "/healthz", "", nil)
	if code != 200 || out["status"] != "ok" || out["active_sessions"] != float64(0) {
		t.Fatalf("%d %v", code, out)
	}
	r.mustCreate(nil)
	_, out = r.do("GET", "/healthz", "", nil)
	if out["active_sessions"] != float64(1) {
		t.Fatalf("%v", out)
	}
}

func TestAuth(t *testing.T) {
	r := newRig(t, func(c *Config) { c.Token = "s3cret-token" })
	body := map[string]any{"cwd": r.work}
	if code, out := r.do("POST", "/v1/sessions", "", body); code != 401 || !hasKey(out, "detail") {
		t.Fatalf("no token: %d %v", code, out)
	}
	if code, _ := r.do("POST", "/v1/sessions", "wrong", body); code != 401 {
		t.Fatalf("wrong token: %d", code)
	}
	if code, _ := r.do("DELETE", "/v1/sessions/x", "wrong", nil); code != 401 {
		t.Fatalf("delete wrong token: %d", code)
	}
	// A scheme other than Bearer is not accepted even with the right secret.
	req, _ := http.NewRequest("POST", r.ts.URL+"/v1/sessions", strings.NewReader(`{"cwd":"`+filepath.ToSlash(r.work)+`"}`))
	req.Header.Set("Authorization", "Basic s3cret-token")
	resp, _ := http.DefaultClient.Do(req)
	resp.Body.Close()
	if resp.StatusCode != 401 {
		t.Fatalf("basic: %d", resp.StatusCode)
	}
	if code, _ := r.do("POST", "/v1/sessions", "s3cret-token", body); code != 200 {
		t.Fatalf("right token: %d", code)
	}
}

func TestNoTokenMeansOpen(t *testing.T) {
	r := newRig(t)
	if code, _ := r.create(nil); code != 200 {
		t.Fatalf("%d", code)
	}
}

func TestCreateValidation(t *testing.T) {
	r := newRig(t)
	if code, _ := r.do("POST", "/v1/sessions", "", map[string]any{"cwd": filepath.Join(r.work, "nope")}); code != 400 {
		t.Fatalf("missing dir: %d", code)
	}
	f := filepath.Join(r.work, "file.txt")
	os.WriteFile(f, []byte("x"), 0o644)
	if code, _ := r.do("POST", "/v1/sessions", "", map[string]any{"cwd": f}); code != 400 {
		t.Fatalf("file as cwd: %d", code)
	}
	code, out := r.do("POST", "/v1/sessions", "", map[string]any{})
	if code != 422 {
		t.Fatalf("no cwd: %d", code)
	}
	if _, ok := out["detail"].([]any); !ok {
		t.Fatalf("422 detail must be an array: %v", out)
	}
	if code, _ := r.create(map[string]any{"allow_shell": "yes"}); code != 422 {
		t.Fatalf("wrong type: %d", code)
	}
	if code, _ := r.create(map[string]any{"max_turns": 2.5}); code != 422 {
		t.Fatalf("float for int: %d", code)
	}
	// Unknown fields are ignored.
	if code, _ := r.create(map[string]any{"future_option": 1}); code != 200 {
		t.Fatalf("unknown field: %d", code)
	}
}

func TestChatErrors(t *testing.T) {
	r := newRig(t)
	if code, out := r.chat("nosuchid", map[string]any{"prompt": "hi"}); code != 404 || !hasKey(out, "detail") {
		t.Fatalf("%d %v", code, out)
	}
	id := r.mustCreate(nil)
	if code, _ := r.chat(id, map[string]any{"prompt": "   "}); code != 400 {
		t.Fatalf("blank: %d", code)
	}
	if code, _ := r.chat(id, map[string]any{"prompt": ""}); code != 422 {
		t.Fatalf("empty: %d", code)
	}
	if code, _ := r.chat(id, map[string]any{}); code != 422 {
		t.Fatalf("missing: %d", code)
	}
	if n := len(r.model.requests()); n != 0 {
		t.Fatalf("bad requests reached the model: %d", n)
	}
}

func TestSlashCommandIsAnsweredLocally(t *testing.T) {
	r := newRig(t)
	id := r.mustCreate(nil)
	code, out := r.chat(id, map[string]any{"prompt": "/help"})
	if code != 200 || out["final_output"] == "" {
		t.Fatalf("%d %v", code, out)
	}
	if n := len(r.model.requests()); n != 0 {
		t.Fatalf("a slash command called the model %d times", n)
	}
}

func TestChatRunResultShape(t *testing.T) {
	r := newRig(t)
	id := r.mustCreate(nil)
	code, out := r.chat(id, map[string]any{"prompt": "hello there"})
	if code != 200 {
		t.Fatalf("%d %v", code, out)
	}
	for _, k := range []string{"final_output", "turns", "tool_calls", "transcript", "session_id", "usage", "total_cost_usd", "stop_reason"} {
		if !hasKey(out, k) {
			t.Errorf("missing %s", k)
		}
	}
	if out["final_output"] != "answer 1" || out["stop_reason"] != "stop" {
		t.Fatalf("%v", out)
	}
	usage := out["usage"].(map[string]any)
	if usage["input_tokens"] != float64(10) || usage["total_tokens"] != float64(12) {
		t.Fatalf("usage %v", usage)
	}
	var sawUser bool
	for _, e := range out["transcript"].([]any) {
		m := e.(map[string]any)
		if m["role"] == "user" && m["content"] == "hello there" {
			sawUser = true
		}
	}
	if !sawUser {
		t.Fatalf("transcript lacks the user prompt")
	}
	if s := fmt.Sprint(out); strings.Contains(s, "service-secret-key-123") {
		t.Fatalf("the API key leaked into a response")
	}
}

func TestBackendFailureIs200WithBackendError(t *testing.T) {
	r := newRig(t, func(c *Config) { c.BaseURL = "http://127.0.0.1:1/v1" })
	id := r.mustCreate(nil)
	code, out := r.chat(id, map[string]any{"prompt": "hi"})
	if code != 200 || out["stop_reason"] != "backend_error" || out["final_output"] == "" {
		t.Fatalf("%d %v", code, out)
	}
}

// ---- tool_allowlist ----------------------------------------------------------------

func TestDefaultSessionHasManyToolsAndSendsThem(t *testing.T) {
	r := newRig(t, func(c *Config) { c.PermitShell, c.PermitWrite = true, true })
	code, out := r.create(map[string]any{"allow_shell": true, "allow_write": true})
	if code != 200 || out["tools"].(float64) <= 15 {
		t.Fatalf("%d %v", code, out)
	}
	id := out["session_id"].(string)
	r.chat(id, map[string]any{"prompt": "hi"})
	req := r.model.requests()[0]
	tl, _ := req["tools"].([]any)
	if len(tl) != int(out["tools"].(float64)) {
		t.Fatalf("model was offered %d tools, create said %v", len(tl), out["tools"])
	}
}

func TestEmptyAllowlistMeansZeroToolsEverywhere(t *testing.T) {
	r := newRig(t)
	code, out := r.create(map[string]any{"tool_allowlist": []string{}})
	if code != 200 || out["tools"] != float64(0) {
		t.Fatalf("%d %v", code, out)
	}
	id := out["session_id"].(string)
	// Twice, so a registry rebuild between turns cannot bring tools back.
	for i := 0; i < 2; i++ {
		if code, res := r.chat(id, map[string]any{"prompt": "no tools please"}); code != 200 || res["stop_reason"] != "stop" {
			t.Fatalf("%d %v", code, res)
		}
	}
	for i, req := range r.model.requests() {
		if hasKey(req, "tools") || hasKey(req, "tool_choice") {
			t.Fatalf("request %d carried tools/tool_choice: %v", i, req)
		}
	}
}

func TestEmptyAllowlistCannotBeWidenedByPermissions(t *testing.T) {
	r := newRig(t, func(c *Config) { c.PermitShell, c.PermitWrite = true, true })
	_, out := r.create(map[string]any{"tool_allowlist": []string{}, "allow_shell": true, "allow_write": true})
	if out["tools"] != float64(0) {
		t.Fatalf("%v", out)
	}
}

func TestNamedAllowlist(t *testing.T) {
	r := newRig(t)
	_, out := r.create(map[string]any{"tool_allowlist": []string{"read_file", "list_dir"}})
	if out["tools"] != float64(2) {
		t.Fatalf("%v", out)
	}
	id := out["session_id"].(string)
	r.chat(id, map[string]any{"prompt": "hi"})
	tl := r.model.requests()[0]["tools"].([]any)
	if len(tl) != 2 {
		t.Fatalf("%v", tl)
	}
	_, out = r.create(map[string]any{"tool_allowlist": []string{"read_file", "no_such_tool"}})
	if out["tools"] != float64(1) {
		t.Fatalf("unknown name must grant nothing: %v", out)
	}
	_, out = r.create(map[string]any{"tool_allowlist": []string{"no_such_tool"}})
	if out["tools"] != float64(0) {
		t.Fatalf("%v", out)
	}
}

// ---- the ceiling -------------------------------------------------------------------

func TestPrivilegeCeiling(t *testing.T) {
	r := newRig(t) // nothing permitted
	for _, extra := range []map[string]any{
		{"allow_shell": true},
		{"allow_write": true},
		{"base_url": "http://evil.example/v1"},
		{"api_key": "other-key-123456"},
		{"session_directory": filepath.Join(r.work, "elsewhere")},
		{"scratchpad_root": filepath.Join(r.work, "elsewhere")},
	} {
		if code, out := r.create(extra); code != 403 || !hasKey(out, "detail") {
			t.Errorf("%v: got %d %v, want 403", extra, code, out)
		}
	}
	// False values and the service's own settings are fine.
	if code, _ := r.create(map[string]any{"allow_shell": false, "allow_write": false, "base_url": r.srv.cfg.BaseURL}); code != 200 {
		t.Fatalf("harmless request refused: %d", code)
	}
	if code, _ := r.create(map[string]any{"session_directory": filepath.Join(r.srv.cfg.SessionDir, "caller1")}); code != 200 {
		t.Fatalf("a subdirectory of the session dir must be allowed: %d", code)
	}
	if code, _ := r.create(map[string]any{"session_directory": filepath.Join(r.srv.cfg.SessionDir, "..", "sessions-evil")}); code != 403 {
		t.Fatalf("a .. escape must be refused: %d", code)
	}
}

func TestPermittedCeilingGrantsAndGatesTools(t *testing.T) {
	r := newRig(t, func(c *Config) { c.PermitWrite = true })
	if code, _ := r.create(map[string]any{"allow_write": true}); code != 200 {
		t.Fatal("permitted write refused")
	}
	if code, _ := r.create(map[string]any{"allow_shell": true}); code != 403 {
		t.Fatal("shell must still be refused")
	}
}

func TestCWDRoots(t *testing.T) {
	r := newRig(t)
	inRoot := filepath.Join(r.work, "project")
	os.MkdirAll(inRoot, 0o755)
	outside := t.TempDir()
	r.srv.cfg.CWDRoots = []string{r.work}
	if code, _ := r.create(map[string]any{"cwd": inRoot}); code != 200 {
		t.Fatalf("inside the root: %d", code)
	}
	if code, out := r.create(map[string]any{"cwd": outside}); code != 403 {
		t.Fatalf("outside the root: %d %v", code, out)
	}
	if code, _ := r.create(map[string]any{"cwd": filepath.Join(r.work, "..")}); code != 403 {
		t.Fatalf("parent of the root: %d", code)
	}
}

func TestBackendOverrideWhenPermitted(t *testing.T) {
	other := newFakeModel(t)
	r := newRig(t, func(c *Config) { c.PermitBackendOverride = true })
	id := r.mustCreate(map[string]any{"base_url": other.URL, "api_key": "caller-key-987654"})
	r.chat(id, map[string]any{"prompt": "hi"})
	if len(other.requests()) != 1 || len(r.model.requests()) != 0 {
		t.Fatalf("override ignored: other=%d default=%d", len(other.requests()), len(r.model.requests()))
	}
	other.mu.Lock()
	auth := other.auth[0]
	other.mu.Unlock()
	if auth != "Bearer caller-key-987654" {
		t.Fatalf("auth header %q", auth)
	}
}

// ---- pool: LRU, eviction, resume ---------------------------------------------------------

func TestLRUEviction(t *testing.T) {
	r := newRig(t, func(c *Config) { c.MaxSessions = 2 })
	a := r.mustCreate(nil)
	b := r.mustCreate(nil)
	// Use a, so b is now the least recently used.
	if code, _ := r.chat(a, map[string]any{"prompt": "touch"}); code != 200 {
		t.Fatal("chat a")
	}
	c := r.mustCreate(nil)
	if r.srv.Active() != 2 {
		t.Fatalf("pool size %d", r.srv.Active())
	}
	if code, _ := r.chat(b, map[string]any{"prompt": "x"}); code != 404 {
		t.Fatalf("evicted b should 404, got %d", code)
	}
	for _, id := range []string{a, c} {
		if code, _ := r.chat(id, map[string]any{"prompt": "x"}); code != 200 {
			t.Fatalf("%s should survive: %d", id, code)
		}
	}
}

func TestDeleteThenReuse(t *testing.T) {
	r := newRig(t)
	id := r.mustCreate(nil)
	if code, out := r.do("DELETE", "/v1/sessions/"+id, "", nil); code != 200 || out["evicted"] != true {
		t.Fatalf("%d %v", code, out)
	}
	if code, _ := r.chat(id, map[string]any{"prompt": "hi"}); code != 404 {
		t.Fatalf("reuse after delete: %d", code)
	}
	if code, _ := r.do("DELETE", "/v1/sessions/"+id, "", nil); code != 404 {
		t.Fatalf("delete twice: %d", code)
	}
	if code, _ := r.do("DELETE", "/v1/sessions/unknown", "", nil); code != 404 {
		t.Fatalf("delete unknown: %d", code)
	}
}

func userTexts(req map[string]any) []string {
	var out []string
	for _, m := range req["messages"].([]any) {
		mm := m.(map[string]any)
		if mm["role"] == "user" {
			out = append(out, fmt.Sprint(mm["content"]))
		}
	}
	return out
}

func TestPoolSessionContinuesItsConversation(t *testing.T) {
	r := newRig(t)
	id := r.mustCreate(nil)
	_, first := r.chat(id, map[string]any{"prompt": "first question"})
	_, second := r.chat(id, map[string]any{"prompt": "second question"})
	if first["session_id"] != second["session_id"] {
		t.Fatalf("agent session id changed: %v vs %v", first["session_id"], second["session_id"])
	}
	texts := strings.Join(userTexts(r.model.requests()[1]), "\n")
	if !strings.Contains(texts, "first question") || !strings.Contains(texts, "second question") {
		t.Fatalf("second request lacks the history: %q", texts)
	}
}

func TestResumeAfterEvictionFromDisk(t *testing.T) {
	r := newRig(t)
	old := r.mustCreate(nil)
	_, first := r.chat(old, map[string]any{"prompt": "remember the word pineapple"})
	r.do("DELETE", "/v1/sessions/"+old, "", nil)

	fresh := r.mustCreate(nil)
	code, out := r.chat(fresh, map[string]any{"prompt": "what was the word?", "resume_session_id": first["session_id"]})
	if code != 200 {
		t.Fatalf("%d %v", code, out)
	}
	if out["session_id"] != first["session_id"] {
		t.Fatalf("resume must continue the same agent session")
	}
	texts := strings.Join(userTexts(r.model.requests()[1]), "\n")
	if !strings.Contains(texts, "pineapple") {
		t.Fatalf("history missing after resume: %q", texts)
	}
}

func TestResumeUnknownOrHostileIDIs404(t *testing.T) {
	r := newRig(t)
	id := r.mustCreate(nil)
	for _, rid := range []string{"doesnotexist", "../../etc/passwd", `..\..\x`, strings.Repeat("a", 200)} {
		if code, out := r.chat(id, map[string]any{"prompt": "hi", "resume_session_id": rid}); code != 404 || !hasKey(out, "detail") {
			t.Errorf("resume %q: %d %v", rid, code, out)
		}
	}
	if n := len(r.model.requests()); n != 0 {
		t.Fatalf("model called %d times", n)
	}
}

func TestSessionsPersistUnderTheirOwnDirectory(t *testing.T) {
	r := newRig(t)
	sub := filepath.Join(r.srv.cfg.SessionDir, "callerA")
	id := r.mustCreate(map[string]any{"session_directory": sub})
	_, out := r.chat(id, map[string]any{"prompt": "hi"})
	if _, err := os.Stat(filepath.Join(sub, out["session_id"].(string)+".json")); err != nil {
		t.Fatalf("transcript not in the session's directory: %v", err)
	}
	// The key is never written to disk.
	b, _ := os.ReadFile(filepath.Join(sub, out["session_id"].(string)+".json"))
	if strings.Contains(string(b), "service-secret-key-123") {
		t.Fatal("API key written to the session file")
	}
	// A session in another directory cannot resume it by id.
	other := r.mustCreate(nil)
	if code, _ := r.chat(other, map[string]any{"prompt": "hi", "resume_session_id": out["session_id"]}); code != 404 {
		t.Fatalf("cross-directory resume: %d", code)
	}
}

func TestTurnsOnOneSessionAreSerialised(t *testing.T) {
	r := newRig(t)
	id := r.mustCreate(nil)
	var wg sync.WaitGroup
	for i := 0; i < 4; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			if code, _ := r.chat(id, map[string]any{"prompt": fmt.Sprintf("q%d", i)}); code != 200 {
				t.Errorf("chat %d: %d", i, code)
			}
		}(i)
	}
	wg.Wait()
	// Serialised turns each see all earlier ones: the last request holds all four prompts.
	reqs := r.model.requests()
	if len(reqs) != 4 {
		t.Fatalf("%d requests", len(reqs))
	}
	if n := len(userTexts(reqs[3])); n < 4 {
		t.Fatalf("last request has only %d user messages; turns overlapped", n)
	}
}

func TestIsLoopbackHost(t *testing.T) {
	for h, want := range map[string]bool{"127.0.0.1": true, "localhost": true, "::1": true, "[::1]": true,
		"0.0.0.0": false, "192.168.1.5": false, "example.com": false, "": false} {
		if IsLoopbackHost(h) != want {
			t.Errorf("%q: want %v", h, want)
		}
	}
}

func TestCommandRefusesNonLoopbackWithoutToken(t *testing.T) {
	var errOut bytes.Buffer
	env := &cli.Env{Stderr: &errOut, Stdout: io.Discard, Getenv: func(string) string { return "" }}
	if code := command(env, []string{"--host", "0.0.0.0"}); code != 2 || !strings.Contains(errOut.String(), "token") {
		t.Fatalf("%d %s", code, errOut.String())
	}
}

func TestCeilingDefaultsFollowTheToken(t *testing.T) {
	var tri triBool
	if tri.resolve("", false) || !tri.resolve("", true) {
		t.Fatal("default must follow trust")
	}
	if !tri.resolve("1", false) || tri.resolve("0", true) {
		t.Fatal("env must override the default")
	}
	if err := tri.Set("true"); err != nil || !tri.resolve("0", false) {
		t.Fatal("a flag must override the env")
	}
}
