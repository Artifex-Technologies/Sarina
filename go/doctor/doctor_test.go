package doctor

import (
	"bytes"
	"context"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

const canary = "CANARY-SECRET-0123456789"

// fakeEnv builds a Getenv from a map; nothing else leaks in from the real environment.
func fakeEnv(m map[string]string) func(string) string { return func(k string) string { return m[k] } }

func byID(t *testing.T, cs []Check, id string) Check {
	t.Helper()
	for _, c := range cs {
		if c.ID == id {
			return c
		}
	}
	t.Fatalf("no check %q in %s", id, JSON(cs))
	return Check{}
}

func run(t *testing.T, env map[string]string, o Options) []Check {
	t.Helper()
	if o.Home == "" {
		o.Home = t.TempDir()
	}
	if o.Cwd == "" {
		o.Cwd = t.TempDir()
	}
	if o.SystemSkillsDir == "" {
		o.SystemSkillsDir = t.TempDir()
	}
	o.Getenv = fakeEnv(env)
	o.Ports = map[string]int{}
	return Run(context.Background(), o)
}

// backendServer answers /v1/models with the given body.
func backendServer(t *testing.T, models string) *httptest.Server {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/v1/models" {
			w.Write([]byte(models))
			return
		}
		http.NotFound(w, r)
	}))
	t.Cleanup(srv.Close)
	return srv
}

func TestBackendReachableListsModelAndWindow(t *testing.T) {
	srv := backendServer(t, `{"data":[{"id":"qwen3:latest","max_model_len":32768}]}`)
	cs := run(t, map[string]string{"OPENAI_BASE_URL": srv.URL + "/v1", "OPENAI_MODEL": "qwen3", "OPENAI_API_KEY": canary}, Options{})
	if c := byID(t, cs, "backend"); c.Status != Pass || !strings.Contains(c.Finding, "qwen3") {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, cs, "context_window"); c.Status != Pass || !strings.Contains(c.Finding, "32768") {
		t.Fatalf("%+v", c)
	}
}

func TestSmallContextWindowWarnsWithTheOllamaHint(t *testing.T) {
	srv := backendServer(t, `{"data":[{"id":"qwen3","max_model_len":4096}]}`)
	cs := run(t, map[string]string{"OPENAI_BASE_URL": srv.URL + "/v1", "OPENAI_MODEL": "qwen3"}, Options{})
	c := byID(t, cs, "context_window")
	if c.Status != Warn || !strings.Contains(c.Remedy, "num_ctx") || !strings.Contains(c.Finding, "4096") {
		t.Fatalf("%+v", c)
	}
	// An explicit operator override wins and is reported as such.
	cs = run(t, map[string]string{"OPENAI_BASE_URL": srv.URL + "/v1", "OPENAI_MODEL": "qwen3", "SARINA_CONTEXT_WINDOW": "16384"}, Options{})
	if c := byID(t, cs, "context_window"); c.Status != Pass || !strings.Contains(c.Finding, "SARINA_CONTEXT_WINDOW") {
		t.Fatalf("%+v", c)
	}
}

func TestBackendProblems(t *testing.T) {
	srv := backendServer(t, `{"data":[{"id":"other"}]}`)
	cs := run(t, map[string]string{"OPENAI_BASE_URL": srv.URL + "/v1", "OPENAI_MODEL": "qwen3"}, Options{})
	if c := byID(t, cs, "backend"); c.Status != Warn || c.Remedy == "" {
		t.Fatalf("unlisted model: %+v", c)
	}
	dead := httptest.NewServer(http.NotFoundHandler())
	url := dead.URL
	dead.Close()
	cs = run(t, map[string]string{"OPENAI_BASE_URL": url + "/v1"}, Options{})
	if c := byID(t, cs, "backend"); c.Status != Fail || c.Remedy == "" {
		t.Fatalf("unreachable: %+v", c)
	}
	if c := byID(t, cs, "context_window"); c.Status != Skip {
		t.Fatalf("window should be skipped: %+v", c)
	}
	auth := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(401) }))
	defer auth.Close()
	cs = run(t, map[string]string{"OPENAI_BASE_URL": auth.URL + "/v1", "OPENAI_API_KEY": canary}, Options{})
	if c := byID(t, cs, "backend"); c.Status != Fail || !strings.Contains(c.Finding, "401") {
		t.Fatalf("auth: %+v", c)
	}
}

// ---- sessions ---------------------------------------------------------------

func saveSession(t *testing.T, dir string, mutate func(*session.Session)) string {
	t.Helper()
	s := &session.Session{ID: session.NewID(), Model: config.ModelConfig{Model: "m", APIKey: canary}, Runtime: config.DefaultRuntime(dir)}
	s.Append(session.Message{Role: "user", Content: "hi", MessageID: "user_1"})
	if mutate != nil {
		mutate(s)
	}
	p, err := s.Save(dir)
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func TestSessionDirectoryCounts(t *testing.T) {
	dir := t.TempDir()
	saveSession(t, dir, nil)
	saveSession(t, dir, nil)
	os.WriteFile(filepath.Join(dir, "broken.json"), []byte("{not json"), 0o600)
	cs := run(t, nil, Options{SessionDir: dir})
	c := byID(t, cs, "sessions")
	if c.Status != Warn || !strings.Contains(c.Finding, "3 session files, 2 parse") || !strings.Contains(c.Finding, "broken.json") {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, cs, "session_retired_tools"); c.Status != Pass {
		t.Fatalf("%+v", c)
	}
	// Written only by Sarina itself: the doctor created nothing.
	if es, _ := os.ReadDir(dir); len(es) != 3 {
		t.Fatalf("the doctor changed the directory: %v", es)
	}
}

func TestMissingSessionDirectoryIsAWarningNotAFailure(t *testing.T) {
	cs := run(t, nil, Options{SessionDir: filepath.Join(t.TempDir(), "nope")})
	if c := byID(t, cs, "sessions"); c.Status != Warn || c.Remedy == "" {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, cs, "session_contract"); c.Status != Skip {
		t.Fatalf("%+v", c)
	}
}

func TestRetiredToolNamesWarnAndNameTheMigration(t *testing.T) {
	dir := t.TempDir()
	saveSession(t, dir, func(s *session.Session) {
		s.Append(session.Message{Role: "assistant", MessageID: "a", ToolCalls: []llm.WireToolCall{{ID: "c1", Type: "function",
			Function: llm.FunctionCall{Name: "task_get", Arguments: `{"task_id":"a"}`}}}})
		s.Append(session.Message{Role: "tool", Name: "task_get", ToolCallID: "c1", Content: session.ToolResultBody("task_get", true, "ok", nil)})
	})
	before, _ := os.ReadFile(filepath.Join(dir, mustOne(t, dir)))
	cs := run(t, nil, Options{SessionDir: dir})
	c := byID(t, cs, "session_retired_tools")
	if c.Status != Warn || !strings.Contains(c.Remedy, "sarina session-migrate") {
		t.Fatalf("%+v", c)
	}
	after, _ := os.ReadFile(filepath.Join(dir, mustOne(t, dir)))
	if !bytes.Equal(before, after) {
		t.Fatal("the doctor rewrote a session file")
	}
}

func mustOne(t *testing.T, dir string) string {
	t.Helper()
	es, _ := os.ReadDir(dir)
	if len(es) != 1 {
		t.Fatalf("want exactly one file, got %d", len(es))
	}
	return es[0].Name()
}

func TestHeimdallContractShapeAndKeyLeak(t *testing.T) {
	dir := t.TempDir()
	saveSession(t, dir, nil)
	cs := run(t, nil, Options{SessionDir: dir})
	if c := byID(t, cs, "session_contract"); c.Status != Pass {
		t.Fatalf("a normal session must pass (the key is never written): %+v", c)
	}

	// messages as bare strings is the shape Heimdall cannot decode as objects with a role.
	os.WriteFile(filepath.Join(dir, "old.json"), []byte(`{"session_id":"old","messages":["plain string"]}`), 0o600)
	cs = run(t, nil, Options{SessionDir: dir})
	if c := byID(t, cs, "session_contract"); c.Status != Warn || !strings.Contains(c.Finding, "old.json") {
		t.Fatalf("%+v", c)
	}

	// A stored api_key is a secret leak: FAIL, file named, value never shown.
	os.WriteFile(filepath.Join(dir, "leaky.json"),
		[]byte(`{"session_id":"x","messages":[{"role":"user","content":"hi"}],"model_config":{"api_key":"`+canary+`"}}`), 0o600)
	cs = run(t, nil, Options{SessionDir: dir})
	c := byID(t, cs, "session_contract")
	if c.Status != Fail || !strings.Contains(c.Finding, "leaky.json") {
		t.Fatalf("%+v", c)
	}
	if strings.Contains(JSON(cs), canary) {
		t.Fatal("the leaked key appears in the report")
	}
	if !HasFailure(cs) {
		t.Fatal("a leak must make the run fail")
	}
}

// ---- tiers and extension files -------------------------------------------------

func writeHome(t *testing.T, home, name, content string) {
	t.Helper()
	p := filepath.Join(home, ".sarina", name)
	os.MkdirAll(filepath.Dir(p), 0o755)
	if err := os.WriteFile(p, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestTiersAbsentValidInvalid(t *testing.T) {
	home := t.TempDir()
	if c := byID(t, run(t, nil, Options{Home: home}), "tiers_config"); c.Status != Pass || !strings.Contains(c.Finding, "absent") {
		t.Fatalf("%+v", c)
	}
	writeHome(t, home, "tiers.json", `{not json`)
	if c := byID(t, run(t, nil, Options{Home: home}), "tiers_config"); c.Status != Warn || c.Remedy == "" {
		t.Fatalf("%+v", c)
	}

	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte(`{"data":[]}`)) }))
	defer up.Close()
	writeHome(t, home, "tiers.json", `{"tiers":[
	  {"name":"local","kind":"openai_compat","base_url":"`+up.URL+`/v1","model":"m"},
	  {"name":"cli","kind":"claude_cli","cli_path":"definitely-not-installed-claude-xyz"},
	  {"name":"cloud","kind":"claude","model":"x","api_key_env":"MY_KEY"}]}`)
	cs := run(t, map[string]string{"MY_KEY": canary}, Options{Home: home})
	if c := byID(t, cs, "tiers_config"); c.Status != Pass || !strings.Contains(c.Finding, "3 tier") {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, cs, "tier_local"); c.Status != Pass {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, cs, "tier_cli"); c.Status != Warn || !strings.Contains(c.Finding, "not found") {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, cs, "tier_cloud"); !strings.Contains(c.Finding, "MY_KEY set") {
		t.Fatalf("%+v", c)
	}
	if strings.Contains(JSON(cs), canary) {
		t.Fatal("a tier key leaked into the report")
	}
}

func TestOperatorPolicyThatWouldStopStartupIsAFailure(t *testing.T) {
	home := t.TempDir()
	if c := byID(t, run(t, nil, Options{Home: home}), "ext_policy"); c.Status != Pass || !strings.Contains(c.Finding, "absent") {
		t.Fatalf("%+v", c)
	}
	writeHome(t, home, "policy.json", `{ this is not json`)
	cs := run(t, nil, Options{Home: home})
	c := byID(t, cs, "ext_policy")
	if c.Status != Fail || c.Remedy == "" || !HasFailure(cs) {
		t.Fatalf("%+v", c)
	}
	// A broken workspace policy only warns: it can never stop startup.
	home2, ws := t.TempDir(), t.TempDir()
	os.WriteFile(filepath.Join(ws, ".sarina-policy.json"), []byte(`nope`), 0o600)
	if c := byID(t, run(t, nil, Options{Home: home2, Cwd: ws}), "ext_policy"); c.Status != Warn {
		t.Fatalf("%+v", c)
	}
}

func TestMCPAndSkillsFilesAreChecked(t *testing.T) {
	home := t.TempDir()
	writeHome(t, home, "mcp.json", `{broken`)
	writeHome(t, home, filepath.Join("skills", "bad.md"), "no front matter")
	cs := run(t, nil, Options{Home: home})
	if c := byID(t, cs, "ext_mcp"); c.Status != Warn || !strings.Contains(c.Finding, "present") {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, cs, "ext_skills"); c.Status != Warn || !strings.Contains(c.Finding, "bad.md") {
		t.Fatalf("%+v", c)
	}
}

// ---- Limbo ---------------------------------------------------------------------

func TestLimboUnsetIsSkipped(t *testing.T) {
	if c := byID(t, run(t, nil, Options{}), "limbo"); c.Status != Skip {
		t.Fatalf("%+v", c)
	}
}

func fakeLimbo(t *testing.T, status int, body string) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "dl")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(dir) })
	path := filepath.Join(dir, "agent.sock")
	l, err := net.Listen("unix", path)
	if err != nil {
		t.Skipf("unix sockets unavailable: %v", err)
	}
	srv := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(status)
		w.Write([]byte(body))
	})}
	go srv.Serve(l)
	t.Cleanup(func() { srv.Close() })
	return path
}

func TestLimboHealthyCountsProfiles(t *testing.T) {
	sock := fakeLimbo(t, 200, `{"profiles":["arch-update","other"]}`)
	c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": sock}, Options{}), "limbo")
	if c.Status != Pass || !strings.Contains(c.Finding, "2 profile") {
		t.Fatalf("%+v", c)
	}
}

func TestLimboNoProfilesWarnsAboutTheSilentlyAbsentInvalidProfile(t *testing.T) {
	sock := fakeLimbo(t, 200, `{"profiles":[],"details":[]}`)
	c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": sock}, Options{}), "limbo")
	if c.Status != Warn || !strings.Contains(c.Remedy, "Limbo docs/INTEGRATION-FAILURES.md") {
		t.Fatalf("%+v", c)
	}
}

func TestLimboRunbookCodesAreMappedAndCited(t *testing.T) {
	sock := fakeLimbo(t, 500, `{"error":"virt_unavailable: the virtualization stack is unavailable on the host. Remedy: x"}`)
	c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": sock}, Options{}), "limbo")
	if c.Status != Fail || !strings.Contains(c.Finding, "virt_unavailable") || !strings.Contains(c.Remedy, "limbo doctor") ||
		!strings.Contains(c.Remedy, "Limbo docs/INTEGRATION-FAILURES.md") {
		t.Fatalf("%+v", c)
	}
}

func TestLimboWrongOrMissingSocket(t *testing.T) {
	c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": "/run/limbo/admin.sock"}, Options{}), "limbo")
	if c.Status != Fail || !strings.Contains(c.Remedy, "agent.sock") {
		t.Fatalf("admin socket: %+v", c)
	}
	c = byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": filepath.Join(t.TempDir(), "agent.sock")}, Options{}), "limbo")
	if c.Status != Fail || !strings.Contains(c.Remedy, "systemctl start limbo") || !strings.Contains(c.Remedy, "Limbo docs/INTEGRATION-FAILURES.md") {
		t.Fatalf("missing: %+v", c)
	}
	// A regular file with the right name is not a socket.
	f := filepath.Join(t.TempDir(), "agent.sock")
	os.WriteFile(f, nil, 0o600)
	if c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": f}, Options{}), "limbo"); c.Status != Fail || !strings.Contains(c.Finding, "not a unix socket") {
		t.Fatalf("file: %+v", c)
	}
}

func TestLimboWorldWritableSocketFails(t *testing.T) {
	if os.PathSeparator == '\\' {
		t.Skip("POSIX permission bits")
	}
	sock := fakeLimbo(t, 200, `{"profiles":["a"]}`)
	if err := os.Chmod(sock, 0o666); err != nil {
		t.Fatal(err)
	}
	c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": sock}, Options{}), "limbo")
	if c.Status != Fail || !strings.Contains(c.Finding, "world-writable") {
		t.Fatalf("%+v", c)
	}
}

// ---- Heimdall ----------------------------------------------------------------------

func TestHeimdallUnsetIsSkippedAndSaysFailOpen(t *testing.T) {
	c := byID(t, run(t, nil, Options{}), "heimdall")
	if c.Status != Skip || !strings.Contains(c.Finding, "fail-open") {
		t.Fatalf("%+v", c)
	}
}

func TestHeimdallHealthUsesHealthzAndSendsNoMemoryText(t *testing.T) {
	var paths []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.Method+" "+r.URL.String())
		if r.Method == http.MethodGet && r.URL.Path == "/healthz" {
			w.Write([]byte("ok"))
			return
		}
		w.WriteHeader(500)
	}))
	defer srv.Close()
	c := byID(t, run(t, map[string]string{"HEIMDALL_REVIEW_URL": srv.URL + "/v1/review"}, Options{}), "heimdall")
	if c.Status != Pass {
		t.Fatalf("%+v", c)
	}
	if len(paths) != 1 || paths[0] != "GET /healthz" {
		t.Fatalf("requests: %v (only GET /healthz is allowed)", paths)
	}
}

func TestHeimdallProblemsWarnAndCiteTheRunbook(t *testing.T) {
	dead := httptest.NewServer(http.NotFoundHandler())
	url := dead.URL
	dead.Close()
	c := byID(t, run(t, map[string]string{"HEIMDALL_REVIEW_URL": url + "/v1/review"}, Options{}), "heimdall")
	if c.Status != Warn || !strings.Contains(c.Remedy, "Heimdall docs/INTEGRATION-FAILURES.md") || !strings.Contains(c.Finding, "fail-open") {
		t.Fatalf("%+v", c)
	}
	notHeimdall := httptest.NewServer(http.NotFoundHandler())
	defer notHeimdall.Close()
	if c := byID(t, run(t, map[string]string{"HEIMDALL_REVIEW_URL": notHeimdall.URL + "/v1/review"}, Options{}), "heimdall"); c.Status != Warn || !strings.Contains(c.Finding, "404") {
		t.Fatalf("%+v", c)
	}
	if c := byID(t, run(t, map[string]string{"HEIMDALL_REVIEW_URL": "not a url"}, Options{}), "heimdall"); c.Status != Warn {
		t.Fatalf("%+v", c)
	}
}

// ---- ports ---------------------------------------------------------------------------

func TestPortsReportFreeAndInUse(t *testing.T) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Skip(err)
	}
	defer l.Close()
	busy := l.Addr().(*net.TCPAddr).Port
	l2, _ := net.Listen("tcp", "127.0.0.1:0")
	free := l2.Addr().(*net.TCPAddr).Port
	l2.Close()
	o := Options{Home: t.TempDir(), Cwd: t.TempDir(), SystemSkillsDir: t.TempDir(), Getenv: fakeEnv(nil), Ports: map[string]int{"a": busy, "b": free}}
	c := byID(t, Run(context.Background(), o), "ports")
	if c.Status != Pass || !strings.Contains(c.Finding, "in use") || !strings.Contains(c.Finding, "free") {
		t.Fatalf("%+v", c)
	}
}

// ---- no secrets, rendering, command, tool ---------------------------------------------

func TestNoSecretsAnywhereInTheReport(t *testing.T) {
	srv := backendServer(t, `{"data":[{"id":"m"}]}`)
	heimdallTok := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte("ok")) }))
	defer heimdallTok.Close()
	env := map[string]string{
		"OPENAI_BASE_URL":   strings.Replace(srv.URL, "http://", "http://user:"+canary+"@", 1) + "/v1",
		"OPENAI_MODEL":      "m",
		"OPENAI_API_KEY":    canary,
		"HEIMDALL_REVIEW_URL": strings.Replace(heimdallTok.URL, "http://", "http://u:"+canary+"@", 1) + "/v1/review?k=" + canary,
	}
	dir := t.TempDir()
	saveSession(t, dir, nil)
	cs := run(t, env, Options{SessionDir: dir})
	for _, out := range []string{JSON(cs), Text(cs)} {
		if strings.Contains(out, canary) {
			t.Fatalf("a secret reached the report:\n%s", out)
		}
	}
	if c := byID(t, cs, "backend"); !strings.Contains(c.Finding, "set, 24 chars") {
		t.Fatalf("expected only state and length: %+v", c)
	}
}

func TestJSONIsAStableArrayAndTextIsAligned(t *testing.T) {
	cs := run(t, nil, Options{SessionDir: filepath.Join(t.TempDir(), "x")})
	var rows []map[string]string
	if err := json.Unmarshal([]byte(JSON(cs)), &rows); err != nil {
		t.Fatal(err)
	}
	for _, r := range rows {
		if len(r) != 4 {
			t.Fatalf("row keys: %v", r)
		}
		for _, k := range []string{"id", "status", "finding", "remedy"} {
			if _, ok := r[k]; !ok {
				t.Fatalf("row lacks %s: %v", k, r)
			}
		}
	}
	wantIDs := []string{"backend", "sessions", "tiers_config", "ext_skills", "ext_mcp", "ext_policy", "limbo", "heimdall", "ports"}
	have := map[string]bool{}
	for _, r := range rows {
		have[r["id"]] = true
	}
	for _, id := range wantIDs {
		if !have[id] {
			t.Errorf("missing check %s", id)
		}
	}
	txt := Text(cs)
	if !strings.Contains(txt, "WARN") || !strings.Contains(txt, "->") || !strings.Contains(txt, " pass, ") {
		t.Fatalf("text:\n%s", txt)
	}
}

func cliRun(t *testing.T, env map[string]string, args ...string) (int, string, string) {
	t.Helper()
	var out, errb bytes.Buffer
	sig := make(chan os.Signal, 1)
	code := cli.Main(append([]string{"doctor"}, args...), cli.Env{Stdout: &out, Stderr: &errb, Stdin: strings.NewReader(""),
		Getenv: fakeEnv(env), Home: t.TempDir(), Interrupts: sig})
	return code, out.String(), errb.String()
}

func TestCommandExitCodes(t *testing.T) {
	good := backendServer(t, `{"data":[{"id":"m","max_model_len":32768}]}`)
	env := map[string]string{"OPENAI_BASE_URL": good.URL + "/v1", "OPENAI_MODEL": "m"}
	ws := t.TempDir()
	code, out, _ := cliRun(t, env, "--cwd", ws, "--session-dir", ws)
	if code != 0 || !strings.Contains(out, "PASS") {
		t.Fatalf("code %d\n%s", code, out)
	}
	code, out, _ = cliRun(t, env, "--json", "--cwd", ws, "--session-dir", ws)
	var rows []Check
	if code != 0 || json.Unmarshal([]byte(out), &rows) != nil || len(rows) == 0 {
		t.Fatalf("json: code %d\n%s", code, out)
	}
	// A failing backend gives exit 1.
	code, _, _ = cliRun(t, map[string]string{"OPENAI_BASE_URL": "http://127.0.0.1:1/v1"}, "--cwd", ws, "--session-dir", ws)
	if code != 1 {
		t.Fatalf("code %d, want 1", code)
	}
	// Usage errors give 2.
	if code, _, _ = cliRun(t, env, "--bogus"); code != 2 {
		t.Fatalf("bad flag: %d", code)
	}
	if code, _, _ = cliRun(t, env, "extra"); code != 2 {
		t.Fatalf("stray argument: %d", code)
	}
}

func TestToolIsReadOnlyAndInTheDefaultRegistry(t *testing.T) {
	var tool tools.Tool
	found := false
	for _, x := range tools.Builtins() {
		if x.Spec.Name == ToolName {
			tool, found = x, true
		}
	}
	if !found {
		t.Fatal("sarina_doctor is not in the default registry")
	}
	if tool.Class != tools.ClassReadOnly || tool.Requires != tools.PermNone {
		t.Fatalf("class %v requires %v", tool.Class, tool.Requires)
	}
	if len(tool.Spec.Description) > 260 {
		t.Fatalf("description is %d chars; keep it terse", len(tool.Spec.Description))
	}
	// Plan mode keeps it.
	reg := tools.NewRegistry()
	reg.Register(tool)
	if got := reg.ForMode(true).Names(); len(got) != 1 {
		t.Fatalf("plan mode dropped the tool: %v", got)
	}
	// It runs against the workspace and returns the JSON report.
	ws := t.TempDir()
	t.Setenv("OPENAI_BASE_URL", "http://127.0.0.1:1/v1")
	t.Setenv("SARINA_LIMBO_SOCKET", "")
	t.Setenv("HEIMDALL_REVIEW_URL", "")
	t.Setenv("SENTRY_REVIEW_URL", "")
	res := reg.Execute(context.Background(), ToolName, map[string]any{}, &tools.Context{Root: ws, SessionDirectory: filepath.Join(ws, "s")})
	var rows []Check
	if !res.OK || json.Unmarshal([]byte(res.Content), &rows) != nil || len(rows) == 0 {
		t.Fatalf("result: %+v", res)
	}
}

func TestNoCheckModifiesTheWorkspace(t *testing.T) {
	ws := t.TempDir()
	dir := filepath.Join(ws, ".port_sessions", "agent")
	os.MkdirAll(dir, 0o755)
	saveSession(t, dir, nil)
	snapshot := func() string {
		var b strings.Builder
		filepath.Walk(ws, func(p string, info os.FileInfo, err error) error {
			if err == nil {
				b.WriteString(p + "|" + info.ModTime().String() + "|")
			}
			return nil
		})
		return b.String()
	}
	before := snapshot()
	run(t, nil, Options{Cwd: ws})
	if snapshot() != before {
		t.Fatal("the doctor changed the workspace")
	}
}

func TestLimboSkippedProfilesWarnWithNamesCodesAndRunbook(t *testing.T) {
	sock := fakeLimbo(t, 200, `{"profiles":["good"],"skipped":[{"name":"bad","code":"profile_invalid","reason":"x","remedy":"fix the toml"},{"name":"","code":"image_missing","reason":"y","remedy":"z"}]}`)
	c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": sock}, Options{}), "limbo")
	if c.Status != Warn || !strings.Contains(c.Finding, "bad [profile_invalid]") || !strings.Contains(c.Finding, "(unnamed) [image_missing]") ||
		!strings.Contains(c.Remedy, "fix the toml") || !strings.Contains(c.Remedy, "Limbo docs/INTEGRATION-FAILURES.md") {
		t.Fatalf("%+v", c)
	}
}

func TestLimboSkippedListInDoctorIsCapped(t *testing.T) {
	var parts []string
	for i := 0; i < 30; i++ {
		parts = append(parts, `{"name":"p","code":"profile_invalid","reason":"r","remedy":"m"}`)
	}
	sock := fakeLimbo(t, 200, `{"profiles":[],"skipped":[`+strings.Join(parts, ",")+`]}`)
	c := byID(t, run(t, map[string]string{"SARINA_LIMBO_SOCKET": sock}, Options{}), "limbo")
	if c.Status != Warn || !strings.Contains(c.Finding, "skipped 20 invalid") || !strings.Contains(c.Finding, "and 15 more") || len(c.Finding) > 600 {
		t.Fatalf("%+v", c)
	}
}
