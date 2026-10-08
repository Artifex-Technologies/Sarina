package gui

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/memory"
)

const testAPIKey = "gui-test-api-key-98765"

// ---- scripted backend ----------------------------------------------------------------

type reply struct {
	content string
	calls   []call
	sse     bool          // answer as a server-sent stream
	block   chan struct{} // when set, wait for it (or the client leaving) before answering
}

type call struct{ id, name, args string }

type model struct {
	*httptest.Server
	mu   sync.Mutex
	left []reply
	reqs []map[string]any
}

func newModel(t *testing.T, script ...reply) *model {
	t.Helper()
	m := &model{left: script}
	m.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasSuffix(r.URL.Path, "/models") {
			w.Write([]byte(`{"data":[{"id":"m"}]}`))
			return
		}
		b, _ := io.ReadAll(r.Body)
		var req map[string]any
		_ = json.Unmarshal(b, &req)
		m.mu.Lock()
		m.reqs = append(m.reqs, req)
		var rp reply
		if len(m.left) > 0 {
			rp, m.left = m.left[0], m.left[1:]
		} else {
			rp = reply{content: "(script exhausted)"}
		}
		m.mu.Unlock()
		if rp.block != nil {
			select {
			case <-rp.block:
			case <-r.Context().Done():
				return
			}
		}
		if rp.sse {
			w.Header().Set("Content-Type", "text/event-stream")
			for _, part := range strings.Split(rp.content, "|") {
				j, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"content": part}}}})
				fmt.Fprintf(w, "data: %s\n\n", j)
			}
			fmt.Fprint(w, "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":3}}\n\n")
			fmt.Fprint(w, "data: [DONE]\n\n")
			return
		}
		msg := map[string]any{"role": "assistant", "content": rp.content}
		finish := "stop"
		if len(rp.calls) > 0 {
			var tcs []any
			for _, c := range rp.calls {
				tcs = append(tcs, map[string]any{"id": c.id, "type": "function", "function": map[string]any{"name": c.name, "arguments": c.args}})
			}
			msg["tool_calls"] = tcs
			finish = "tool_calls"
		}
		json.NewEncoder(w).Encode(map[string]any{
			"choices": []any{map[string]any{"message": msg, "finish_reason": finish}},
			"usage":   map[string]any{"prompt_tokens": 7, "completion_tokens": 3},
		})
	}))
	t.Cleanup(m.Close)
	return m
}

func (m *model) count() int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return len(m.reqs)
}

func (m *model) request(i int) map[string]any {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.reqs[i]
}

// lastToolText is the content of the last tool message in the latest request.
func (m *model) lastToolText() string {
	m.mu.Lock()
	defer m.mu.Unlock()
	if len(m.reqs) == 0 {
		return ""
	}
	msgs, _ := m.reqs[len(m.reqs)-1]["messages"].([]any)
	for i := len(msgs) - 1; i >= 0; i-- {
		mm := msgs[i].(map[string]any)
		if mm["role"] == "tool" {
			return fmt.Sprint(mm["content"])
		}
	}
	return ""
}

// ---- rig ------------------------------------------------------------------------------

type rig struct {
	t    *testing.T
	srv  *Server
	ts   *httptest.Server
	m    *model
	work string
	home string
}

func newRig(t *testing.T, script []reply, mut ...func(*Config)) *rig {
	t.Helper()
	m := newModel(t, script...)
	work := t.TempDir()
	home := t.TempDir()
	st := DefaultState(func(string) string { return "" }, work)
	st.Model, st.BaseURL, st.APIKey = "test-model", m.URL, testAPIKey
	st.Stream = false
	st.TimeoutSeconds = 10
	cfg := Config{Initial: st, Home: home, Getenv: func(string) string { return "" }, ApprovalTimeout: 5 * time.Second}
	for _, f := range mut {
		f(&cfg)
	}
	srv := New(cfg)
	ts := httptest.NewServer(srv.Handler())
	t.Cleanup(ts.Close)
	t.Cleanup(func() { // never leave a run behind
		srv.mu.Lock()
		r := srv.run
		srv.mu.Unlock()
		if r != nil && r.running() {
			r.cancel()
			<-r.done
		}
	})
	return &rig{t: t, srv: srv, ts: ts, m: m, work: work, home: home}
}

func (r *rig) req(method, path string, body any, mod ...func(*http.Request)) *http.Response {
	r.t.Helper()
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	}
	req, _ := http.NewRequest(method, r.ts.URL+path, rd)
	req.Header.Set(tokenHeader, r.srv.Token())
	req.Header.Set("Content-Type", "application/json")
	for _, m := range mod {
		if m != nil {
			m(req)
		}
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		r.t.Fatal(err)
	}
	return resp
}

func (r *rig) do(method, path string, body any) (int, map[string]any) {
	r.t.Helper()
	resp := r.req(method, path, body)
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	var out map[string]any
	_ = json.Unmarshal(raw, &out)
	if out == nil {
		out = map[string]any{"_raw": string(raw)}
	}
	return resp.StatusCode, out
}

func (r *rig) doList(path string) []any {
	r.t.Helper()
	resp := r.req("GET", path, nil)
	defer resp.Body.Close()
	var out []any
	json.NewDecoder(resp.Body).Decode(&out)
	return out
}

func (r *rig) settle(patch map[string]any) {
	r.t.Helper()
	if code, out := r.do("POST", "/api/state", patch); code != 200 {
		r.t.Fatalf("state: %d %v", code, out)
	}
}

// sse is an open event stream.
type sse struct {
	resp   *http.Response
	events chan map[string]any
}

func (r *rig) openSSE(method, path string, body any) *sse {
	r.t.Helper()
	resp := r.req(method, path, body, func(q *http.Request) { q.Header.Set("Accept", "text/event-stream") })
	if resp.StatusCode != 200 {
		raw, _ := io.ReadAll(resp.Body)
		r.t.Fatalf("sse %s: %d %s", path, resp.StatusCode, raw)
	}
	s := &sse{resp: resp, events: make(chan map[string]any, 256)}
	go func() {
		defer close(s.events)
		sc := bufio.NewScanner(resp.Body)
		sc.Buffer(make([]byte, 1<<20), 8<<20)
		for sc.Scan() {
			line := sc.Text()
			if strings.HasPrefix(line, "data: ") {
				var e map[string]any
				if json.Unmarshal([]byte(line[6:]), &e) == nil {
					s.events <- e
				}
			}
		}
	}()
	r.t.Cleanup(func() { resp.Body.Close() })
	return s
}

// until reads events until one of type typ arrives and returns everything read.
func (s *sse) until(t *testing.T, typ string) []map[string]any {
	t.Helper()
	var got []map[string]any
	timeout := time.After(15 * time.Second)
	for {
		select {
		case e, ok := <-s.events:
			if !ok {
				t.Fatalf("stream ended before %q; got %v", typ, types(got))
			}
			got = append(got, e)
			if e["type"] == typ {
				return got
			}
		case <-timeout:
			t.Fatalf("timed out waiting for %q; got %v", typ, types(got))
		}
	}
}

func types(es []map[string]any) []string {
	var out []string
	for _, e := range es {
		out = append(out, fmt.Sprint(e["type"]))
	}
	return out
}

func find(es []map[string]any, typ string) map[string]any {
	for _, e := range es {
		if e["type"] == typ {
			return e
		}
	}
	return nil
}

func (r *rig) waitApproval() map[string]any {
	r.t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if l := r.doList("/api/approvals"); len(l) > 0 {
			return l[0].(map[string]any)
		}
		time.Sleep(10 * time.Millisecond)
	}
	r.t.Fatal("no approval became pending")
	return nil
}

// ---- request checks -------------------------------------------------------------------

func TestHostOriginAndTokenChecks(t *testing.T) {
	r := newRig(t, nil)
	get := func(mod func(*http.Request)) int {
		resp := r.req("GET", "/api/state", nil, mod)
		resp.Body.Close()
		return resp.StatusCode
	}
	if c := get(nil); c != 200 {
		t.Fatalf("baseline: %d", c)
	}
	for name, mod := range map[string]func(*http.Request){
		"rebinding host":      func(q *http.Request) { q.Host = "evil.example:8765" },
		"host with port only": func(q *http.Request) { q.Host = "attacker.test" },
		"cross origin":        func(q *http.Request) { q.Header.Set("Origin", "http://evil.example") },
		"origin null":         func(q *http.Request) { q.Header.Set("Origin", "null") },
		"cross-site fetch":    func(q *http.Request) { q.Header.Set("Sec-Fetch-Site", "cross-site") },
	} {
		if c := get(mod); c != 403 {
			t.Errorf("%s: got %d, want 403", name, c)
		}
	}
	for name, mod := range map[string]func(*http.Request){
		"localhost host": func(q *http.Request) { q.Host = "localhost:8765" },
		"ipv6 host":      func(q *http.Request) { q.Host = "[::1]:8765" },
		"same origin":    func(q *http.Request) { q.Header.Set("Origin", "http://"+q.Host) },
	} {
		if c := get(mod); c != 200 {
			t.Errorf("%s: got %d, want 200", name, c)
		}
	}
	if c := get(func(q *http.Request) { q.Header.Del(tokenHeader) }); c != 401 {
		t.Errorf("no token: %d", c)
	}
	if c := get(func(q *http.Request) { q.Header.Set(tokenHeader, "wrong") }); c != 401 {
		t.Errorf("wrong token: %d", c)
	}
	// a hostile Host also gets nothing from the page or static files
	for _, p := range []string{"/", "/static/app.js"} {
		resp := r.req("GET", p+"?t="+r.srv.Token(), nil, func(q *http.Request) { q.Host = "evil.example" })
		resp.Body.Close()
		if resp.StatusCode != 403 {
			t.Errorf("%s with hostile host: %d", p, resp.StatusCode)
		}
	}
}

func TestMutationsMustBeJSON(t *testing.T) {
	r := newRig(t, nil)
	for _, ct := range []string{"text/plain", "application/x-www-form-urlencoded", "multipart/form-data", ""} {
		resp := r.req("POST", "/api/state", map[string]any{"model": "x"}, func(q *http.Request) {
			if ct == "" {
				q.Header.Del("Content-Type")
			} else {
				q.Header.Set("Content-Type", ct)
			}
		})
		resp.Body.Close()
		if resp.StatusCode != 415 {
			t.Errorf("content type %q: %d, want 415", ct, resp.StatusCode)
		}
	}
	if r.snapshotModel() != "test-model" {
		t.Fatal("a rejected request changed state")
	}
	resp := r.req("POST", "/api/state", map[string]any{"model": "x"}, func(q *http.Request) { q.Header.Set("Content-Type", "application/json; charset=utf-8") })
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("json with charset: %d", resp.StatusCode)
	}
	// DELETE is a mutation too
	resp = r.req("DELETE", "/api/sessions/abc", nil, func(q *http.Request) { q.Header.Set("Content-Type", "text/plain") })
	resp.Body.Close()
	if resp.StatusCode != 415 {
		t.Fatalf("delete content type: %d", resp.StatusCode)
	}
}

func (r *rig) snapshotModel() string {
	_, out := r.do("GET", "/api/state", nil)
	return fmt.Sprint(out["model"])
}

func TestIndexNeedsLaunchToken(t *testing.T) {
	r := newRig(t, nil)
	get := func(path string, mod func(*http.Request)) (int, string, []*http.Cookie) {
		req, _ := http.NewRequest("GET", r.ts.URL+path, nil)
		if mod != nil {
			mod(req)
		}
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		defer resp.Body.Close()
		b, _ := io.ReadAll(resp.Body)
		return resp.StatusCode, string(b), resp.Cookies()
	}
	if c, _, _ := get("/", nil); c != 403 {
		t.Fatalf("no token: %d", c)
	}
	if c, _, _ := get("/?t=wrong", nil); c != 403 {
		t.Fatalf("wrong token: %d", c)
	}
	c, body, cookies := get("/?t="+r.srv.Token(), nil)
	if c != 200 || !strings.Contains(body, r.srv.Token()) {
		t.Fatalf("with token: %d, token embedded: %v", c, strings.Contains(body, r.srv.Token()))
	}
	if strings.Contains(body, tokenMarker) {
		t.Fatal("the token placeholder was not replaced")
	}
	if len(cookies) != 1 || !cookies[0].HttpOnly || cookies[0].SameSite != http.SameSiteStrictMode {
		t.Fatalf("cookie: %+v", cookies)
	}
	// A reload without the query still works through the cookie.
	if c, _, _ := get("/", func(q *http.Request) { q.AddCookie(cookies[0]) }); c != 200 {
		t.Fatalf("cookie reload: %d", c)
	}
	// Security headers on everything.
	resp, _ := http.Get(r.ts.URL + "/static/app.css")
	resp.Body.Close()
	if !strings.Contains(resp.Header.Get("Content-Security-Policy"), "default-src 'none'") || resp.Header.Get("X-Content-Type-Options") != "nosniff" {
		t.Fatalf("headers: %v", resp.Header)
	}
	if resp.Header.Get("Access-Control-Allow-Origin") != "" {
		t.Fatal("no CORS headers are ever sent")
	}
}

func TestFrontEndLoadsNothingFromTheNetwork(t *testing.T) {
	url := regexp.MustCompile(`(?i)(https?:)?//[a-z0-9.-]+\.[a-z]{2,}`)
	entries, err := staticFS.ReadDir("static")
	if err != nil || len(entries) < 3 {
		t.Fatalf("static assets missing: %v", err)
	}
	for _, e := range entries {
		b, _ := staticFS.ReadFile("static/" + e.Name())
		if m := url.Find(b); m != nil {
			t.Errorf("%s references a network address: %q", e.Name(), m)
		}
		if e.Name() == "index.html" {
			s := string(b)
			if regexp.MustCompile(`(?i)<script(\s[^>]*)?>[^<]`).MatchString(s) || regexp.MustCompile(`(?i)\son[a-z]+=`).MatchString(s) {
				t.Errorf("index.html has inline script or handlers (the CSP forbids them)")
			}
			if strings.Contains(strings.ToLower(s), "<style") || strings.Contains(strings.ToLower(s), " style=") {
				t.Errorf("index.html has inline styles")
			}
		}
	}
}

// ---- settings -------------------------------------------------------------------------

func TestStateNeverShowsTheKeyAndPatchSemantics(t *testing.T) {
	r := newRig(t, nil)
	_, snap := r.do("GET", "/api/state", nil)
	if strings.Contains(fmt.Sprint(snap), testAPIKey) || snap["api_key_set"] != true {
		t.Fatalf("snapshot: %v", snap)
	}
	if _, has := snap["api_key"]; has {
		t.Fatal("api_key must not be a field")
	}
	for _, k := range []string{"model", "base_url", "cwd", "session_directory", "allow_shell", "allow_write", "temperature",
		"timeout_seconds", "stream_model_responses", "max_turns", "max_total_tokens", "max_total_cost_usd", "system_prompt",
		"response_schema", "auto_snip_threshold_tokens", "compact_preserve_messages", "disable_claude_md_discovery",
		"additional_working_directories", "active_session_id", "approval_mode", "permission_mode"} {
		if _, ok := snap[k]; !ok {
			t.Errorf("snapshot lacks %s", k)
		}
	}
	// omitted = unchanged; explicit null = clear
	code, out := r.do("POST", "/api/state", map[string]any{"max_total_tokens": 5000, "append_system_prompt": "be brief", "temperature": 0.3})
	if code != 200 || out["max_total_tokens"] != float64(5000) || out["append_system_prompt"] != "be brief" {
		t.Fatalf("%d %v", code, out)
	}
	_, out = r.do("POST", "/api/state", map[string]any{"model": "other"})
	if out["max_total_tokens"] != float64(5000) || out["temperature"] != 0.3 {
		t.Fatalf("an omitted key changed: %v", out)
	}
	_, out = r.do("POST", "/api/state", map[string]any{"max_total_tokens": nil, "append_system_prompt": ""})
	if out["max_total_tokens"] != nil || out["append_system_prompt"] != nil {
		t.Fatalf("null/empty must clear: %v", out)
	}
	// the key is write-only: setting it changes api_key_set only
	_, out = r.do("POST", "/api/state", map[string]any{"api_key": "brand-new-secret-key-1"})
	if strings.Contains(fmt.Sprint(out), "brand-new-secret-key-1") {
		t.Fatal("the key came back")
	}
}

func TestStateValidation(t *testing.T) {
	r := newRig(t, nil)
	for name, patch := range map[string]map[string]any{
		"cwd missing":       {"cwd": filepath.Join(r.work, "nope")},
		"extra dir missing": {"additional_working_directories": []string{filepath.Join(r.work, "nope")}},
		"negative temp":     {"temperature": -1},
		"zero timeout":      {"timeout_seconds": 0},
		"zero turns":        {"max_turns": 0},
		"zero budget":       {"max_total_tokens": 0},
		"fraction budget":   {"max_tool_calls": 2.5},
		"schema not object": {"response_schema": []int{1}},
		"empty schema name": {"response_schema_name": " "},
		"negative preserve": {"compact_preserve_messages": -1},
		"empty model":       {"model": ""},
		"bad approval mode": {"approval_mode": "yolo"},
	} {
		if code, _ := r.do("POST", "/api/state", patch); code != 400 {
			t.Errorf("%s: got %d, want 400", name, code)
		}
	}
	for name, patch := range map[string]map[string]any{
		"string for bool":   {"allow_write": "yes"},
		"string for number": {"temperature": "hot"},
		"number for string": {"model": 5},
	} {
		code, out := r.do("POST", "/api/state", patch)
		if code != 422 {
			t.Errorf("%s: got %d %v, want 422", name, code, out)
		}
		if _, ok := out["detail"].([]any); !ok {
			t.Errorf("%s: 422 detail must be a list: %v", name, out)
		}
	}
	// a rejected patch changes nothing, even its valid keys
	r.do("POST", "/api/state", map[string]any{"model": "changed", "temperature": -1})
	if r.snapshotModel() != "test-model" {
		t.Fatal("a partly valid patch was half applied")
	}
	// the workspace change moves the default session directory with it
	sub := filepath.Join(r.work, "proj")
	os.MkdirAll(sub, 0o755)
	_, out := r.do("POST", "/api/state", map[string]any{"cwd": sub})
	if !strings.HasSuffix(fmt.Sprint(out["session_directory"]), filepath.Join("proj", ".port_sessions", "agent")) {
		t.Fatalf("session dir %v", out["session_directory"])
	}
}

func TestWideningPermissionsNeedsConfirmation(t *testing.T) {
	r := newRig(t, nil)
	if code, out := r.do("POST", "/api/state", map[string]any{"allow_shell": true}); code != 400 || !strings.Contains(fmt.Sprint(out["detail"]), "confirm") {
		t.Fatalf("%d %v", code, out)
	}
	if code, _ := r.do("POST", "/api/state", map[string]any{"allow_write": true}); code != 400 {
		t.Fatal("write without confirm")
	}
	_, snap := r.do("GET", "/api/state", nil)
	if snap["allow_shell"] != false || snap["permission_mode"] != "read-only" {
		t.Fatalf("%v", snap)
	}
	code, out := r.do("POST", "/api/state", map[string]any{"allow_write": true, "confirm_permissions": true})
	if code != 200 || out["permission_mode"] != "write" {
		t.Fatalf("%d %v", code, out)
	}
	// narrowing never needs it
	if code, _ := r.do("POST", "/api/state", map[string]any{"allow_write": false}); code != 200 {
		t.Fatal("narrowing refused")
	}
	// dropping the per-action questions is a widening too
	r.settle(map[string]any{"allow_write": true, "confirm_permissions": true})
	if code, _ := r.do("POST", "/api/state", map[string]any{"approval_mode": "auto"}); code != 400 {
		t.Fatal("auto-approve without confirm")
	}
}

// ---- chat -----------------------------------------------------------------------------

func TestBlockingChatAndResult(t *testing.T) {
	r := newRig(t, []reply{{content: "hello operator"}, {content: "second"}})
	code, out := r.do("POST", "/api/chat", map[string]any{"prompt": "hi"})
	if code != 200 {
		t.Fatalf("%d %v", code, out)
	}
	if out["final_output"] != "hello operator" || out["stop_reason"] != "stop" || out["session_id"] == "" {
		t.Fatalf("%v", out)
	}
	for _, k := range []string{"turns", "tool_calls", "transcript", "usage", "total_cost_usd"} {
		if _, ok := out[k]; !ok {
			t.Errorf("missing %s", k)
		}
	}
	_, st := r.do("GET", "/api/state", nil)
	if st["active_session_id"] != out["session_id"] {
		t.Fatalf("active session not recorded: %v", st["active_session_id"])
	}
	// resume the saved session
	code, out2 := r.do("POST", "/api/chat", map[string]any{"prompt": "again", "resume_session_id": out["session_id"]})
	if code != 200 || out2["session_id"] != out["session_id"] {
		t.Fatalf("%d %v", code, out2)
	}
}

func TestChatValidation(t *testing.T) {
	r := newRig(t, nil)
	if code, _ := r.do("POST", "/api/chat", map[string]any{"prompt": ""}); code != 422 {
		t.Fatalf("empty: %d", code)
	}
	if code, _ := r.do("POST", "/api/chat", map[string]any{}); code != 422 {
		t.Fatalf("missing: %d", code)
	}
	if code, _ := r.do("POST", "/api/chat", map[string]any{"prompt": "  \n "}); code != 400 {
		t.Fatalf("blank: %d", code)
	}
	for _, id := range []string{"missing", "../../etc/passwd", `..\x`} {
		if code, _ := r.do("POST", "/api/chat", map[string]any{"prompt": "hi", "resume_session_id": id}); code != 404 {
			t.Errorf("resume %q: %d", id, code)
		}
	}
	if r.m.count() != 0 {
		t.Fatal("invalid requests reached the model")
	}
	// a failed start gives the run slot back
	if code, _ := r.do("POST", "/api/chat", map[string]any{"prompt": "hi"}); code != 200 {
		t.Fatalf("slot not released: %d", code)
	}
}

func TestPastedContentIsExpanded(t *testing.T) {
	r := newRig(t, []reply{{content: "ok"}})
	r.do("POST", "/api/chat", map[string]any{"prompt": "summarise [Pasted text #1 +3 lines] now",
		"pasted_contents": map[string]any{"1": map[string]any{"type": "text", "content": "LINE-ONE\nLINE-TWO"}}})
	req := r.m.request(0)
	msgs := req["messages"].([]any)
	last := msgs[len(msgs)-1].(map[string]any)["content"].(string)
	if !strings.Contains(last, "summarise LINE-ONE\nLINE-TWO now") {
		t.Fatalf("prompt: %q", last)
	}
}

func TestSSEStreamOfARunWithATool(t *testing.T) {
	r := newRig(t, []reply{
		{content: "looking", calls: []call{{"c1", "list_dir", `{"path":"."}`}}},
		{content: "all done"},
	})
	os.WriteFile(filepath.Join(r.work, "marker.txt"), []byte("x"), 0o644)
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "list the files"})
	got := s.until(t, "done")
	ty := types(got)
	order := map[string]int{}
	for i, x := range ty {
		if _, seen := order[x]; !seen {
			order[x] = i
		}
	}
	for _, need := range []string{"run_start", "turn_start", "assistant_message", "tool_call", "tool_result", "usage", "done"} {
		if _, ok := order[need]; !ok {
			t.Fatalf("missing %s in %v", need, ty)
		}
	}
	if !(order["run_start"] < order["tool_call"] && order["tool_call"] < order["tool_result"] && order["tool_result"] < order["done"]) {
		t.Fatalf("bad order: %v", ty)
	}
	tc := find(got, "tool_call")
	if tc["name"] != "list_dir" || tc["effect"] != "read" || tc["arguments"].(map[string]any)["path"] != "." {
		t.Fatalf("tool_call: %v", tc)
	}
	tr := find(got, "tool_result")
	if tr["ok"] != true || !strings.Contains(fmt.Sprint(tr["content"]), "marker.txt") {
		t.Fatalf("tool_result: %v", tr)
	}
	done := find(got, "done")["result"].(map[string]any)
	if done["final_output"] != "all done" || done["tool_calls"] != float64(1) {
		t.Fatalf("done: %v", done)
	}
	// seq numbers only go up
	last := 0.0
	for _, e := range got {
		if e["seq"].(float64) <= last {
			t.Fatalf("seq not increasing: %v", ty)
		}
		last = e["seq"].(float64)
	}
}

func TestSSEStreamsTextDeltas(t *testing.T) {
	r := newRig(t, []reply{{content: "Hel|lo wor|ld", sse: true}})
	r.settle(map[string]any{"stream_model_responses": true})
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "hi"})
	got := s.until(t, "done")
	var text string
	n := 0
	for _, e := range got {
		if e["type"] == "text_delta" {
			text += e["delta"].(string)
			n++
		}
	}
	if text != "Hello world" || n < 3 {
		t.Fatalf("deltas %d %q (%v)", n, text, types(got))
	}
}

func TestReattachReplaysMissedEvents(t *testing.T) {
	r := newRig(t, []reply{{content: "done quickly"}})
	r.do("POST", "/api/chat", map[string]any{"prompt": "hi"})
	s := r.openSSE("GET", "/api/events?after=0", nil)
	got := s.until(t, "done")
	if find(got, "run_start") == nil {
		t.Fatalf("replay lacks run_start: %v", types(got))
	}
	// and a client that already saw part of it gets only the rest
	first := int(got[0]["seq"].(float64))
	s2 := r.openSSE("GET", fmt.Sprintf("/api/events?after=%d", first), nil)
	got2 := s2.until(t, "done")
	if got2[0]["seq"].(float64) <= float64(first) {
		t.Fatalf("replayed an event it had already seen")
	}
	_, status := r.do("GET", "/api/run", nil)
	if status["state"] != StateFinished {
		t.Fatalf("status %v", status)
	}
}

func TestSecondRunIsRefusedAndCancelStopsTheFirst(t *testing.T) {
	gate := make(chan struct{})
	r := newRig(t, []reply{{content: "slow", block: gate}})
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "first"})
	s.until(t, "turn_start")
	_, status := r.do("GET", "/api/run", nil)
	if status["state"] != StateRunning {
		t.Fatalf("status %v", status)
	}
	if code, out := r.do("POST", "/api/chat", map[string]any{"prompt": "second"}); code != 409 {
		t.Fatalf("second run: %d %v", code, out)
	}
	if code, out := r.do("POST", "/api/state", map[string]any{"model": "changed-mid-run"}); code != 200 {
		t.Fatalf("settings during a run: %d %v", code, out)
	}
	code, out := r.do("POST", "/api/chat/cancel", map[string]any{})
	if code != 200 || out["cancelled"] != true {
		t.Fatalf("cancel: %d %v", code, out)
	}
	got := s.until(t, "done")
	res := find(got, "done")["result"].(map[string]any)
	if res["stop_reason"] != "interrupted" {
		t.Fatalf("stop_reason %v", res["stop_reason"])
	}
	if _, st := r.do("GET", "/api/run", nil); st["state"] != StateFinished {
		t.Fatalf("after cancel: %v", st)
	}
	// nothing left to cancel
	if _, out := r.do("POST", "/api/chat/cancel", map[string]any{}); out["cancelled"] != false {
		t.Fatalf("%v", out)
	}
	// and the slot is free again
	close(gate)
	r.m.mu.Lock()
	r.m.left = []reply{{content: "free again"}}
	r.m.mu.Unlock()
	if code, out := r.do("POST", "/api/chat", map[string]any{"prompt": "third"}); code != 200 || out["final_output"] != "free again" {
		t.Fatalf("%d %v", code, out)
	}
}

// ---- approvals ------------------------------------------------------------------------

func writeCall(id, path, content string) call {
	b, _ := json.Marshal(map[string]any{"path": path, "content": content})
	return call{id, "write_file", string(b)}
}

func (r *rig) allowWrite() {
	r.settle(map[string]any{"allow_write": true, "confirm_permissions": true})
}

func TestApprovalAllowOnceRunsTheTool(t *testing.T) {
	r := newRig(t, []reply{{calls: []call{writeCall("w1", "out.txt", "hello")}}, {content: "wrote it"}})
	r.allowWrite()
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "write a file"})
	ap := r.waitApproval()
	if ap["kind"] != "tool" || ap["tool_name"] != "write_file" || !strings.Contains(fmt.Sprint(ap["risk"]), "medium") {
		t.Fatalf("approval: %v", ap)
	}
	if ap["arguments"].(map[string]any)["content"] != "hello" || ap["summary"] == "" {
		t.Fatalf("approval must show the full arguments: %v", ap)
	}
	if _, err := os.Stat(filepath.Join(r.work, "out.txt")); err == nil {
		t.Fatal("the tool ran before the operator answered")
	}
	if _, st := r.do("GET", "/api/run", nil); st["state"] != StateWaiting || len(st["pending_approvals"].([]any)) != 1 {
		t.Fatalf("status while waiting: %v", st)
	}
	if code, _ := r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "bogus"}); code != 400 {
		t.Fatal("bogus decision accepted")
	}
	if code, out := r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "allow_once"}); code != 200 {
		t.Fatalf("%d %v", code, out)
	}
	got := s.until(t, "done")
	if find(got, "approval_request") == nil || find(got, "approval_resolved") == nil {
		t.Fatalf("events: %v", types(got))
	}
	if b, err := os.ReadFile(filepath.Join(r.work, "out.txt")); err != nil || string(b) != "hello" {
		t.Fatalf("file not written: %v %q", err, b)
	}
	if code, _ := r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "allow_once"}); code != 404 {
		t.Fatalf("answering twice: %d", code)
	}
}

func TestApprovalDenyReachesTheModelAsAToolResult(t *testing.T) {
	r := newRig(t, []reply{{calls: []call{writeCall("w1", "no.txt", "x")}}, {content: "understood"}})
	r.allowWrite()
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "write"})
	ap := r.waitApproval()
	r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "deny", "reason": "not that file"})
	got := s.until(t, "done")
	if _, err := os.Stat(filepath.Join(r.work, "no.txt")); err == nil {
		t.Fatal("a denied tool ran")
	}
	tr := find(got, "tool_result")
	if tr["ok"] != false || !strings.Contains(fmt.Sprint(tr["content"]), "not that file") {
		t.Fatalf("tool_result: %v", tr)
	}
	if !strings.Contains(r.m.lastToolText(), "not that file") {
		t.Fatalf("the model was not told why: %q", r.m.lastToolText())
	}
}

func TestApprovalAllowForSessionSkipsLaterQuestions(t *testing.T) {
	r := newRig(t, []reply{
		{calls: []call{writeCall("w1", "a.txt", "1")}},
		{calls: []call{writeCall("w2", "b.txt", "2")}},
		{content: "both"},
	})
	r.allowWrite()
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "write two"})
	ap := r.waitApproval()
	r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "allow_session"})
	got := s.until(t, "done")
	n := 0
	for _, e := range got {
		if e["type"] == "approval_request" {
			n++
		}
	}
	if n != 1 {
		t.Fatalf("asked %d times, want once", n)
	}
	for _, f := range []string{"a.txt", "b.txt"} {
		if _, err := os.Stat(filepath.Join(r.work, f)); err != nil {
			t.Errorf("%s missing", f)
		}
	}
}

func TestUnansweredApprovalTimesOutAsDeny(t *testing.T) {
	r := newRig(t, []reply{{calls: []call{writeCall("w1", "late.txt", "x")}}, {content: "ok"}},
		func(c *Config) { c.ApprovalTimeout = 150 * time.Millisecond })
	r.allowWrite()
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "write"})
	got := s.until(t, "done")
	if _, err := os.Stat(filepath.Join(r.work, "late.txt")); err == nil {
		t.Fatal("an unanswered request ran")
	}
	if tr := find(got, "tool_result"); tr["ok"] != false || !strings.Contains(fmt.Sprint(tr["content"]), "timeout") {
		t.Fatalf("%v", tr)
	}
}

func TestCancelWhileWaitingForApproval(t *testing.T) {
	r := newRig(t, []reply{{calls: []call{writeCall("w1", "c.txt", "x")}}, {content: "never"}})
	r.allowWrite()
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "write"})
	r.waitApproval()
	r.do("POST", "/api/chat/cancel", map[string]any{})
	got := s.until(t, "done")
	if _, err := os.Stat(filepath.Join(r.work, "c.txt")); err == nil {
		t.Fatal("cancelled tool ran")
	}
	if find(got, "done")["result"].(map[string]any)["stop_reason"] != "interrupted" {
		t.Fatalf("%v", find(got, "done"))
	}
	if l := r.doList("/api/approvals"); len(l) != 0 {
		t.Fatalf("approval left pending: %v", l)
	}
}

func TestReadOnlyToolsAndForbiddenWritesAskNothing(t *testing.T) {
	// no write permission: the call is denied outright, with no prompt
	r := newRig(t, []reply{{calls: []call{writeCall("w1", "x.txt", "x")}}, {content: "denied"}})
	got := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "write"}).until(t, "done")
	if find(got, "approval_request") != nil {
		t.Fatal("asked about something the toggles forbid")
	}
	if tr := find(got, "tool_result"); tr["ok"] != false {
		t.Fatalf("%v", tr)
	}
}

func TestAutoApproveRunsWithoutAsking(t *testing.T) {
	r := newRig(t, []reply{{calls: []call{writeCall("w1", "auto.txt", "yes")}}, {content: "done"}})
	r.settle(map[string]any{"allow_write": true, "approval_mode": "auto", "confirm_permissions": true})
	got := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "write"}).until(t, "done")
	if find(got, "approval_request") != nil {
		t.Fatal("asked in auto mode")
	}
	if _, err := os.Stat(filepath.Join(r.work, "auto.txt")); err != nil {
		t.Fatal("not written")
	}
}

func TestPlanApprovalFlow(t *testing.T) {
	plan, _ := json.Marshal(map[string]any{"plan": "1. read the code\n2. fix the bug"})
	script := func() []reply {
		return []reply{
			{calls: []call{{"p1", "enter_plan_mode", "{}"}}},
			{calls: []call{{"p2", "exit_plan_mode", string(plan)}}},
			{content: "carrying on"},
		}
	}
	// approve
	r := newRig(t, script())
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "plan it"})
	ap := r.waitApproval()
	if ap["kind"] != "plan" || !strings.Contains(fmt.Sprint(ap["plan"]), "fix the bug") {
		t.Fatalf("plan approval: %v", ap)
	}
	_, pv := r.do("GET", "/api/plan", nil)
	if !strings.Contains(fmt.Sprint(pv["pending"]), "fix the bug") || pv["approval_id"] != ap["approval_id"] {
		t.Fatalf("plan view: %v", pv)
	}
	// the tool-approval decisions are not valid for a plan
	if code, _ := r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "allow_once"}); code != 400 {
		t.Fatal("tool decision accepted for a plan")
	}
	if code, _ := r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "revise"}); code != 400 {
		t.Fatal("revise without feedback accepted")
	}
	r.do("POST", "/api/approvals/"+ap["approval_id"].(string), map[string]any{"decision": "approve"})
	got := s.until(t, "done")
	res := find(got, "done")["result"].(map[string]any)
	if res["final_output"] != "carrying on" || res["stop_reason"] == "plan_pending" {
		t.Fatalf("approve: %v", res)
	}
	if !strings.Contains(r.m.lastToolText(), "pprov") {
		t.Fatalf("model not told of the approval: %q", r.m.lastToolText())
	}

	// revise keeps plan mode on and carries the feedback
	r2 := newRig(t, script())
	s2 := r2.openSSE("POST", "/api/chat", map[string]any{"prompt": "plan it"})
	ap2 := r2.waitApproval()
	r2.do("POST", "/api/approvals/"+ap2["approval_id"].(string), map[string]any{"decision": "revise", "reason": "also add tests"})
	s2.until(t, "done")
	if !strings.Contains(r2.m.lastToolText(), "also add tests") {
		t.Fatalf("feedback lost: %q", r2.m.lastToolText())
	}

	// no answer (timeout): the run ends with plan_pending, never an approval
	r3 := newRig(t, script(), func(c *Config) { c.ApprovalTimeout = 100 * time.Millisecond })
	got3 := r3.openSSE("POST", "/api/chat", map[string]any{"prompt": "plan it"}).until(t, "done")
	if find(got3, "done")["result"].(map[string]any)["stop_reason"] != "plan_pending" {
		t.Fatalf("unanswered plan: %v", find(got3, "done"))
	}
}

// ---- sessions, history, files ---------------------------------------------------------

func TestSessionListGetDeleteAndSearch(t *testing.T) {
	r := newRig(t, []reply{{content: "one"}, {content: "two"}})
	_, a := r.do("POST", "/api/chat", map[string]any{"prompt": "tell me about pineapples"})
	time.Sleep(20 * time.Millisecond)
	r.settle(map[string]any{}) // keep active
	_, b := r.do("POST", "/api/chat", map[string]any{"prompt": "and about walnuts"})
	list := r.doList("/api/sessions")
	if len(list) != 2 {
		t.Fatalf("%v", list)
	}
	first := list[0].(map[string]any)
	if first["session_id"] != b["session_id"] || !strings.Contains(fmt.Sprint(first["preview"]), "walnuts") {
		t.Fatalf("newest first: %v", first)
	}
	for _, k := range []string{"turns", "tool_calls", "preview", "modified_at"} {
		if _, ok := first[k]; !ok {
			t.Errorf("session row lacks %s", k)
		}
	}
	if l := r.doList("/api/sessions?q=pineapple"); len(l) != 1 || l[0].(map[string]any)["session_id"] != a["session_id"] {
		t.Fatalf("search: %v", l)
	}
	code, sess := r.do("GET", "/api/sessions/"+a["session_id"].(string), nil)
	if code != 200 || len(sess["messages"].([]any)) < 3 || sess["model"] != "test-model" {
		t.Fatalf("%d %v", code, sess)
	}
	if code, _ := r.do("GET", "/api/sessions/nope", nil); code != 404 {
		t.Fatal("unknown session")
	}
	for _, id := range []string{"..%2f..%2fetc%2fpasswd", "..", "a%2fb"} {
		if code, _ := r.do("GET", "/api/sessions/"+id, nil); code != 404 {
			t.Errorf("hostile id %q: %d", id, code)
		}
		if code, _ := r.do("DELETE", "/api/sessions/"+id, nil); code != 404 {
			t.Errorf("hostile delete %q: %d", id, code)
		}
	}
	if code, _ := r.do("DELETE", "/api/sessions/"+b["session_id"].(string), nil); code != 200 {
		t.Fatal("delete")
	}
	if l := r.doList("/api/sessions"); len(l) != 1 {
		t.Fatalf("after delete: %v", l)
	}
	if code, _ := r.do("DELETE", "/api/sessions/"+b["session_id"].(string), nil); code != 404 {
		t.Fatal("delete twice")
	}
	if _, st := r.do("GET", "/api/state", nil); st["active_session_id"] != nil {
		t.Fatalf("deleting the active session must clear it: %v", st["active_session_id"])
	}
}

func TestFileHistoryAndTasks(t *testing.T) {
	r := newRig(t, []reply{{calls: []call{writeCall("w1", "h.txt", "x")}}, {content: "ok"}})
	r.settle(map[string]any{"allow_write": true, "approval_mode": "auto", "confirm_permissions": true})
	r.do("POST", "/api/chat", map[string]any{"prompt": "write"})
	code, fh := r.do("GET", "/api/file-history?limit=10", nil)
	if code != 200 || fh["total"] != float64(1) || len(fh["entries"].([]any)) != 1 {
		t.Fatalf("%d %v", code, fh)
	}
	e := fh["entries"].([]any)[0].(map[string]any)
	if e["session_id"] == nil || e["timestamp"] == nil {
		t.Fatalf("%v", e)
	}
	_, tasks := r.do("GET", "/api/tasks", nil)
	if len(tasks["tasks"].([]any)) != 0 {
		t.Fatalf("%v", tasks)
	}
	os.MkdirAll(filepath.Join(r.work, ".port_sessions"), 0o755)
	os.WriteFile(filepath.Join(r.work, ".port_sessions", "task_runtime.json"),
		[]byte(`{"tasks":[{"task_id":"t1","title":"do it","status":"pending"},{"task_id":"t2","title":"done","status":"completed"}]}`), 0o644)
	_, tasks = r.do("GET", "/api/tasks", nil)
	if len(tasks["tasks"].([]any)) != 2 || tasks["counts"].(map[string]any)["pending"] != float64(1) {
		t.Fatalf("%v", tasks)
	}
}

func TestSlashCommandsList(t *testing.T) {
	r := newRig(t, nil)
	l := r.doList("/api/slash-commands")
	found := false
	for _, c := range l {
		if c.(map[string]any)["primary"] == "help" {
			found = true
		}
	}
	if !found {
		t.Fatalf("%v", l)
	}
}

// ---- instruction files --------------------------------------------------------------

func TestInstructionFilesAreSandboxed(t *testing.T) {
	r := newRig(t, nil)
	claude := filepath.Join(r.work, "CLAUDE.md")
	os.WriteFile(claude, []byte("# rules\nbe nice"), 0o644)
	outside := filepath.Join(t.TempDir(), "secret.txt")
	os.WriteFile(outside, []byte("do not read"), 0o644)

	code, list := r.do("GET", "/api/instructions", nil)
	if code != 200 {
		t.Fatal(code)
	}
	var names []string
	for _, f := range list["files"].([]any) {
		names = append(names, f.(map[string]any)["path"].(string))
	}
	if !contains(names, claude) {
		t.Fatalf("workspace CLAUDE.md not listed: %v", names)
	}
	q := func(p string) string { return "/api/instructions/file?path=" + urlEscape(p) }
	if code, out := r.do("GET", q(claude), nil); code != 200 || out["content"] != "# rules\nbe nice" {
		t.Fatalf("%d %v", code, out)
	}
	// reads are limited to discovered files, not any readable path
	for _, p := range []string{outside, filepath.Join(r.work, "notes.txt"), filepath.Join(r.work, "..", "x", "CLAUDE.md"),
		filepath.Join(r.work, "sub", "..", "..", filepath.Base(filepath.Dir(outside)), "secret.txt"), "/etc/passwd", `C:\Windows\win.ini`} {
		if code, _ := r.do("GET", q(p), nil); code != 403 {
			t.Errorf("read %q: got %d, want 403", p, code)
		}
	}
	// writes
	if code, _ := r.do("PUT", q(claude), map[string]any{"content": "# new"}); code != 200 {
		t.Fatalf("write: %d", code)
	}
	if b, _ := os.ReadFile(claude); string(b) != "# new" {
		t.Fatalf("content %q", b)
	}
	if code, _ := r.do("PUT", q(outside), map[string]any{"content": "pwn"}); code != 403 {
		t.Fatal("write outside")
	}
	if b, _ := os.ReadFile(outside); string(b) != "do not read" {
		t.Fatal("file outside the sandbox changed")
	}
	if code, _ := r.do("PUT", q(claude), map[string]any{"content": 5}); code != 422 {
		t.Fatalf("wrong type: %d", code)
	}
	// a new rules file inside the workspace is fine
	rule := filepath.Join(r.work, ".claude", "rules", "style.md")
	if code, _ := r.do("PUT", q(rule), map[string]any{"content": "tabs"}); code != 200 {
		t.Fatalf("new rule: %d", code)
	}
	if code, out := r.do("DELETE", q(rule), nil); code != 200 {
		t.Fatalf("delete: %d %v", code, out)
	}
	if code, _ := r.do("DELETE", q(outside), nil); code != 403 {
		t.Fatal("delete outside")
	}
	// the global config dir is writable, a parent directory's file is read-only
	global := filepath.Join(r.home, ".claude", "CLAUDE.md")
	if code, _ := r.do("PUT", q(global), map[string]any{"content": "global"}); code != 200 {
		t.Fatalf("global write: %d", code)
	}
	parent := filepath.Join(filepath.Dir(r.work), "CLAUDE.md")
	os.WriteFile(parent, []byte("parent rules"), 0o644)
	defer os.Remove(parent)
	if code, _ := r.do("GET", q(parent), nil); code != 200 {
		t.Fatalf("ancestor read: %d", code)
	}
	if code, _ := r.do("PUT", q(parent), map[string]any{"content": "x"}); code != 403 {
		t.Fatalf("ancestor write: %d", code)
	}
}

func contains(l []string, s string) bool {
	for _, x := range l {
		if x == s {
			return true
		}
	}
	return false
}

func urlEscape(s string) string {
	return strings.NewReplacer("%", "%25", " ", "%20", "&", "%26", "#", "%23", "+", "%2B", "?", "%3F").Replace(s)
}

func TestInstructionFileSecretsAreRedactedAndNotWrittenBack(t *testing.T) {
	r := newRig(t, nil)
	claude := filepath.Join(r.work, "CLAUDE.md")
	os.WriteFile(claude, []byte("deploy with token sk-ant-api03-ABCDEFGHIJKLMNOP"), 0o644)
	_, out := r.do("GET", "/api/instructions/file?path="+urlEscape(claude), nil)
	if strings.Contains(fmt.Sprint(out["content"]), "ABCDEFGHIJKLMNOP") || out["redacted"] != true {
		t.Fatalf("%v", out)
	}
	if code, _ := r.do("PUT", "/api/instructions/file?path="+urlEscape(claude), map[string]any{"content": fmt.Sprint(out["content"])}); code != 400 {
		t.Fatal("saving redacted text back would destroy the secret")
	}
}

// ---- memory review --------------------------------------------------------------------

func TestMemoryReviewQueue(t *testing.T) {
	r := newRig(t, nil)
	q := memory.QueueFromWorkspace(r.work)
	text := "The build uses make all."
	why := "seen twice"
	if err := q.Add([]memory.Proposal{
		{EntryID: "e1", SessionID: "s", Scope: "agent", Action: "add", Content: &text, Rationale: why, Status: memory.StatusPending},
		{EntryID: "e2", SessionID: "s", Scope: "agent", Action: "add", Content: &why, Rationale: "r", Status: memory.StatusPending},
	}); err != nil {
		t.Fatal(err)
	}
	code, out := r.do("GET", "/api/memory", nil)
	if code != 200 || len(out["pending_review"].([]any)) != 2 {
		t.Fatalf("%d %v", code, out)
	}
	if code, _ := r.do("POST", "/api/memory/review/e1", map[string]any{"decision": "maybe"}); code != 400 {
		t.Fatal("bad decision")
	}
	if code, o := r.do("POST", "/api/memory/review/e1", map[string]any{"decision": "approve"}); code != 200 {
		t.Fatalf("approve: %d %v", code, o)
	}
	if code, _ := r.do("POST", "/api/memory/review/e2", map[string]any{"decision": "reject"}); code != 200 {
		t.Fatal("reject")
	}
	if code, _ := r.do("POST", "/api/memory/review/e1", map[string]any{"decision": "approve"}); code != 409 {
		t.Fatal("approving twice")
	}
	_, out = r.do("GET", "/api/memory", nil)
	if len(out["pending_review"].([]any)) != 0 {
		t.Fatalf("%v", out["pending_review"])
	}
	entries := out["scopes"].(map[string]any)["agent"].(map[string]any)["entries"].([]any)
	if len(entries) != 1 || entries[0] != text {
		t.Fatalf("approved entry not in memory: %v", entries)
	}
}

// ---- redaction ------------------------------------------------------------------------

func TestSecretsNeverLeaveInResponsesOrEvents(t *testing.T) {
	leak := "the key is " + testAPIKey + " and sk-ant-api03-ZZZZYYYYXXXXWWWW and Bearer abcdefghijklmnopqrstuv"
	r := newRig(t, []reply{{content: leak}})
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "what is the key"})
	got := s.until(t, "done")
	all, _ := json.Marshal(got)
	for _, secret := range []string{testAPIKey, "ZZZZYYYYXXXXWWWW", "abcdefghijklmnopqrstuv"} {
		if strings.Contains(string(all), secret) {
			t.Fatalf("%q leaked in the stream", secret)
		}
	}
	id := find(got, "done")["result"].(map[string]any)["session_id"].(string)
	_, sess := r.do("GET", "/api/sessions/"+id, nil)
	if strings.Contains(fmt.Sprint(sess), testAPIKey) || strings.Contains(fmt.Sprint(r.doList("/api/sessions")), testAPIKey) {
		t.Fatal("key leaked through the session views")
	}
	// and a request body quoting the token header is not echoed either
	_, st := r.do("GET", "/api/state", nil)
	if strings.Contains(fmt.Sprint(st), r.srv.Token()) {
		t.Fatal("console token echoed")
	}
}

// ---- status endpoints -----------------------------------------------------------------

func TestBackendAndTiersStatus(t *testing.T) {
	r := newRig(t, nil)
	code, out := r.do("GET", "/api/backend", nil)
	if code != 200 || out["ok"] != true || out["model"] != "test-model" {
		t.Fatalf("%d %v", code, out)
	}
	r.settle(map[string]any{"base_url": "http://127.0.0.1:1/v1"})
	_, out = r.do("GET", "/api/backend", nil)
	if out["ok"] != false || out["error"] == nil {
		t.Fatalf("unreachable backend: %v", out)
	}
	code, tr := r.do("GET", "/api/tiers", nil)
	if code != 200 || tr["configured"] != false {
		t.Fatalf("%d %v", code, tr)
	}
}

func TestIdleStatus(t *testing.T) {
	r := newRig(t, nil)
	_, st := r.do("GET", "/api/run", nil)
	if st["state"] != StateIdle || st["run_id"] != nil {
		t.Fatalf("%v", st)
	}
	if code, out := r.do("POST", "/api/clear", map[string]any{}); code != 200 || out["active_session_id"] != nil {
		t.Fatalf("%d %v", code, out)
	}
}

func TestUnknownRoutes(t *testing.T) {
	r := newRig(t, nil)
	for _, p := range []string{"/api/nothing", "/api/worktree", "/api/accounts", "/api/mcp/tools/call"} {
		if code, _ := r.do("GET", p, nil); code != 404 {
			t.Errorf("%s: %d (dropped panels must not exist)", p, code)
		}
	}
}

func TestSlashCommandIsAnsweredWithoutTheModel(t *testing.T) {
	r := newRig(t, nil)
	s := r.openSSE("POST", "/api/chat", map[string]any{"prompt": "/help"})
	got := s.until(t, "done")
	am := find(got, "assistant_message")
	if am == nil || !strings.Contains(fmt.Sprint(am["content"]), "Slash Commands") {
		t.Fatalf("%v", types(got))
	}
	if r.m.count() != 0 {
		t.Fatal("a slash command reached the model")
	}
	// the slot is free afterwards
	if code, out := r.do("POST", "/api/chat", map[string]any{"prompt": "/help"}); code != 200 || out["final_output"] == "" {
		t.Fatalf("%d %v", code, out)
	}
}

func TestNonLoopbackBindIsRefused(t *testing.T) {
	var errOut bytes.Buffer
	code := command(&cli.Env{Stderr: &errOut, Stdout: io.Discard, Getenv: func(string) string { return "" }}, []string{"--host", "0.0.0.0", "--no-browser"})
	if code != 2 || !strings.Contains(errOut.String(), "loopback") {
		t.Fatalf("%d %s", code, errOut.String())
	}
}
