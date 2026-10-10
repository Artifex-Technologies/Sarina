package council

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/tiers"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// redirectTransport sends requests for *.example hosts to one test server, so a
// member with a public-looking URL (and so "remote") can be exercised offline.
type redirectTransport struct{ to *url.URL }

func (r redirectTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	if !strings.HasSuffix(req.URL.Hostname(), ".example") {
		return http.DefaultTransport.RoundTrip(req) // loopback (local) members go where they say
	}
	c := req.Clone(req.Context())
	c.URL.Scheme, c.URL.Host = r.to.Scheme, r.to.Host
	return http.DefaultTransport.RoundTrip(c)
}

func toolConfig(t *testing.T, doc string) {
	t.Helper()
	p := filepath.Join(t.TempDir(), "council.json")
	if err := os.WriteFile(p, []byte(doc), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("SARINA_COUNCIL_FILE", p)
}

func callTool(t *testing.T, question string, confirm func(string) bool) (string, error) {
	t.Helper()
	ts := Tools()
	if len(ts) != 1 || ts[0].Spec.Name != "council_ask" {
		t.Fatalf("tools = %v", ts)
	}
	tc := &tools.Context{Runtimes: map[string]any{}}
	if confirm != nil {
		tc.Runtimes[tools.RuntimeOperatorConfirm] = confirm
	}
	out, _, err := ts[0].Handler(context.Background(), map[string]any{"question": question}, tc)
	return out, err
}

func setupRemote(t *testing.T) (local, remote *fakeModel) {
	t.Helper()
	local, remote = newFake(t, 200, "local view"), newFake(t, 200, "online view")
	u, _ := url.Parse(remote.srv.URL)
	old := toolHTTPClient
	toolHTTPClient = &http.Client{Transport: redirectTransport{to: u}}
	t.Cleanup(func() { toolHTTPClient = old })
	t.Setenv("GROQ_TEST_KEY", "groq-secret-key-value")
	toolConfig(t, `{"members":[
	  {"name":"mine","base_url":"`+local.srv.URL+`/v1","model":"m"},
	  {"name":"groq","base_url":"https://api.groq.example/v1","model":"m","api_key_env":"GROQ_TEST_KEY","auto":true}]}`)
	return local, remote
}

func TestToolExistsOnlyWhenACouncilIsConfigured(t *testing.T) {
	t.Setenv("SARINA_COUNCIL_FILE", filepath.Join(t.TempDir(), "absent.json"))
	if len(Tools()) != 0 {
		t.Fatal("no config must mean no council_ask tool (it costs context on every request)")
	}
	toolConfig(t, `{"members":[{"name":"a","base_url":"http://127.0.0.1:1/v1","model":"m","enabled":false}]}`)
	if len(Tools()) != 0 {
		t.Fatal("a council with nothing enabled must not offer the tool")
	}
	toolConfig(t, `{nope`)
	if len(Tools()) != 0 {
		t.Fatal("a broken config must not offer the tool")
	}
}

func TestToolWithNoWayToAskNeverContactsAnOnlineMember(t *testing.T) {
	local, remote := setupRemote(t)
	out, err := callTool(t, "why does nvidia-drm fail?", nil)
	if err != nil {
		t.Fatal(err)
	}
	if len(remote.sent()) != 0 {
		t.Fatal("an online member was contacted with no operator to ask")
	}
	if len(local.sent()) == 0 || !strings.Contains(out, "local view") || !strings.Contains(out, "no way to ask") {
		t.Fatalf("out=%s", out)
	}
}

func TestToolAsksEveryCallShowingTheExactTextAndHonoursNo(t *testing.T) {
	_, remote := setupRemote(t)
	var prompts []string
	deny := func(p string) bool { prompts = append(prompts, p); return false }
	q := "dkms build of nvidia fails with: implicit declaration of get_user_pages"
	out, err := callTool(t, q, deny)
	if err != nil {
		t.Fatal(err)
	}
	if len(remote.sent()) != 0 || !strings.Contains(out, "declined") {
		t.Fatalf("a declined member was contacted or not reported: %s", out)
	}
	if len(prompts) != 1 || !strings.Contains(prompts[0], q) || !strings.Contains(prompts[0], "groq") || !strings.Contains(prompts[0], "api.groq.example") {
		t.Fatalf("the prompt must show the member, the host and the exact text: %v", prompts)
	}
	// A second call asks again: there is no remembered approval.
	callTool(t, q, deny)
	if len(prompts) != 2 {
		t.Fatalf("asked %d times for 2 calls", len(prompts))
	}
}

func TestToolApprovedCallSendsRedactedTextAndTheMembersOwnKey(t *testing.T) {
	_, remote := setupRemote(t)
	asked := 0
	secret := "sk-abcdefghijklmnopqrstuvwxyz0123456789"
	out, err := callTool(t, "config has api_key="+secret+" -- why 401?", func(string) bool { asked++; return true })
	if err != nil {
		t.Fatal(err)
	}
	sent := remote.sent()
	if asked != 1 || len(sent) != 1 || !strings.Contains(out, "online view") {
		t.Fatalf("asked=%d sent=%d out=%s", asked, len(sent), out)
	}
	if strings.Contains(sent[0], secret) || remote.auths[0] != "Bearer groq-secret-key-value" {
		t.Fatalf("secret reached the member or wrong key: %s / %q", sent[0], remote.auths[0])
	}
}

func TestToolRejectsEmptyAndOversizeQuestions(t *testing.T) {
	setupRemote(t)
	if _, err := callTool(t, "   ", nil); err == nil {
		t.Fatal("empty question accepted")
	}
	if _, err := callTool(t, strings.Repeat("x", maxToolQuestion+1), nil); err == nil {
		t.Fatal("oversize question accepted")
	}
}

func TestToolNeverUsesTheApprover(t *testing.T) {
	// The Approver can carry "allow for this session", which would silently make a
	// per-call question a standing yes. The tool must use the confirm channel only.
	raw, err := os.ReadFile("tool.go")
	if err != nil {
		t.Fatal(err)
	}
	for _, line := range strings.Split(string(raw), "\n") {
		code := strings.TrimSpace(line)
		if strings.HasPrefix(code, "//") {
			continue
		}
		if strings.Contains(code, ".Approver") {
			t.Fatalf("tool.go uses the Approver: %s", code)
		}
	}
	var approved atomic.Bool
	ts := Tools
	_ = ts
	toolConfig(t, `{"members":[{"name":"a","base_url":"http://127.0.0.1:1/v1","model":"m"}]}`)
	tc := &tools.Context{Runtimes: map[string]any{}, Approver: func(context.Context, tools.ApprovalRequest) (bool, string) { approved.Store(true); return true, "" }}
	Tools()[0].Handler(context.Background(), map[string]any{"question": "q"}, tc)
	if approved.Load() {
		t.Fatal("the tool consulted the Approver")
	}
}

func TestLocalMembersRunOneAtATimeByDefault(t *testing.T) {
	var cur, peak int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.Copy(io.Discard, r.Body)
		n := atomic.AddInt32(&cur, 1)
		for {
			p := atomic.LoadInt32(&peak)
			if n <= p || atomic.CompareAndSwapInt32(&peak, p, n) {
				break
			}
		}
		time.Sleep(60 * time.Millisecond)
		atomic.AddInt32(&cur, -1)
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"x"}}]}`))
	}))
	defer srv.Close()
	mk := func(n string) Member {
		return Member{Name: n, Kind: KindOpenAICompat, BaseURL: srv.URL + "/v1", Model: "m", Local: true, Auto: true, Enabled: true, AllowData: tiers.AllowData{All: true}}
	}
	cfg := Config{Members: []Member{mk("a"), mk("b"), mk("c")}, MaxTokens: 16, TimeoutSeconds: 10, LocalConc: 1}
	Ask(context.Background(), Options{Config: cfg, NoSynthesis: true}, "q")
	if peak != 1 {
		t.Fatalf("peak concurrency %d with local_concurrency 1", peak)
	}
	cfg.LocalConc = 3
	peak = 0
	Ask(context.Background(), Options{Config: cfg, NoSynthesis: true}, "q")
	if peak < 2 {
		t.Fatalf("peak concurrency %d with local_concurrency 3: members did not overlap", peak)
	}
}

func TestMaxRemoteMembersCapsAPerRunFanOut(t *testing.T) {
	f := newFake(t, 200, "r")
	var ms []Member
	for _, n := range []string{"r1", "r2", "r3"} {
		m := member(n, f, false)
		m.Auto = true
		ms = append(ms, m)
	}
	cfg := Config{Members: ms, MaxTokens: 16, TimeoutSeconds: 10, MaxRemote: 2}
	res := Ask(context.Background(), Options{Config: cfg, Getenv: getenv(map[string]string{"REMOTE_KEY": "remote-api-key-value"}), NoSynthesis: true}, "q")
	if len(f.sent()) != 2 || !strings.Contains(res.Answers[2].Skipped, "max_remote_members") {
		t.Fatalf("sent=%d answers=%+v", len(f.sent()), res.Answers)
	}
}

func TestClaudeMemberSpeaksTheMessagesAPI(t *testing.T) {
	var path, key, ver string
	var body string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		path, key, ver, body = r.URL.Path, r.Header.Get("x-api-key"), r.Header.Get("anthropic-version"), string(b)
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(`{"type":"message","role":"assistant","content":[{"type":"text","text":"claude view"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":6}}`))
	}))
	defer srv.Close()
	cfg, err := Parse([]byte(`{"members":[{"name":"claude","kind":"claude","model":"claude-sonnet-5-5","api_key_env":"ANTHROPIC_TEST_KEY","base_url":"` + srv.URL + `","auto":true}]}`))
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Members[0].Local && !strings.HasPrefix(srv.URL, "http://127.") {
		t.Fatal("locality")
	}
	cfg.Members[0].Local = false // the fake is on loopback; treat it as the online service it stands for
	cfg.Members[0].AllowData = tiers.AllowData{Classes: []string{tiers.ClassNoSecrets}}
	res := Ask(context.Background(), Options{Config: cfg, Getenv: getenv(map[string]string{"ANTHROPIC_TEST_KEY": "anthropic-key-value"}), NoSynthesis: true}, "q")
	if res.Answers[0].Text != "claude view" || path != "/v1/messages" || key != "anthropic-key-value" || ver == "" {
		t.Fatalf("answer=%+v path=%s key=%s ver=%s", res.Answers[0], path, key, ver)
	}
	if strings.Contains(body, `"tools"`) {
		t.Fatalf("a Claude member was sent tool definitions: %s", body)
	}
	if res.Usage.InputTokens != 5 || res.Usage.OutputTokens != 6 || !strings.Contains(res.Text(), "Tokens used") {
		t.Fatalf("usage %+v", res.Usage)
	}
}

func TestParseKinds(t *testing.T) {
	for name, doc := range map[string]string{
		"claude needs a key":  `{"members":[{"name":"c","kind":"claude","model":"m"}]}`,
		"claude needs model":  `{"members":[{"name":"c","kind":"claude","api_key_env":"K"}]}`,
		"unknown kind":        `{"members":[{"name":"c","kind":"gemini","model":"m","base_url":"https://x.example/v1"}]}`,
		"negative concurrent": `{"local_concurrency":-1,"members":[]}`,
	} {
		if _, err := Parse([]byte(doc)); err == nil {
			t.Errorf("%s: expected an error", name)
		}
	}
	cfg, err := Parse([]byte(`{"members":[{"name":"c","kind":"claude","model":"m","api_key_env":"K"}]}`))
	if err != nil || cfg.Members[0].Local || cfg.Members[0].Auto || cfg.Members[0].BaseURL != "" {
		t.Fatalf("a claude member with no base_url is online and asks first: %+v %v", cfg.Members[0], err)
	}
	if cfg.LocalConc != 1 || cfg.MaxRemote != 3 {
		t.Fatalf("defaults: %+v", cfg)
	}
}

func TestProbe(t *testing.T) {
	var hits int32
	var gotAuth string
	var gotBody atomic.Value
	status := 200
	var mu sync.Mutex
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&hits, 1)
		b, _ := io.ReadAll(r.Body)
		gotBody.Store(string(b))
		mu.Lock()
		gotAuth = r.Header.Get("Authorization")
		st := status
		mu.Unlock()
		w.WriteHeader(st)
		json.NewEncoder(w).Encode(map[string]any{"data": []any{}})
	}))
	defer srv.Close()
	m := Member{Name: "x", Kind: KindOpenAICompat, BaseURL: srv.URL + "/v1", Model: "m", APIKeyEnv: "K", Enabled: true}
	opt := Options{Getenv: getenv(map[string]string{"K": "key-value-123"})}
	if ok, why := Probe(context.Background(), opt, m); !ok || why != "" {
		t.Fatalf("healthy: %v %s", ok, why)
	}
	if gotAuth != "Bearer key-value-123" || gotBody.Load().(string) != "" {
		t.Fatalf("a probe sends the key and no body: auth=%q body=%q", gotAuth, gotBody.Load())
	}
	mu.Lock()
	status = 401
	mu.Unlock()
	if ok, why := Probe(context.Background(), opt, m); ok || !strings.Contains(why, "rejected") {
		t.Fatalf("401: %v %s", ok, why)
	}
	mu.Lock()
	status = 503
	mu.Unlock()
	if ok, why := Probe(context.Background(), opt, m); ok || !strings.Contains(why, "unhealthy") {
		t.Fatalf("503: %v %s", ok, why)
	}
	if ok, why := Probe(context.Background(), Options{}, m); ok || !strings.Contains(why, "not set") {
		t.Fatalf("missing key: %v %s", ok, why)
	}
	m.APIKeyEnv, m.BaseURL = "", "http://127.0.0.1:1/v1"
	if ok, why := Probe(context.Background(), Options{}, m); ok || !strings.Contains(why, "unreachable") {
		t.Fatalf("down: %v %s", ok, why)
	}
}

func TestStatusProbeSendsNoQuestion(t *testing.T) {
	a := newFake(t, 200, "x")
	home := writeCouncil(t, `{"members":[{"name":"a","base_url":"`+a.srv.URL+`/v1","model":"m"}]}`)
	code, out, errs := runCmd(t, home, "", "status", "--probe")
	if code != 0 || !strings.Contains(out, "reachable") || strings.Contains(out, "NOT reachable") {
		t.Fatalf("code=%d out=%s err=%s", code, out, errs)
	}
	if !strings.Contains(out, "no question was sent") {
		t.Fatal("status --probe must say it sent no question")
	}
}

// A configured council adds this tool to every request, which a small local
// context window pays for. Keep it small; shorten the text rather than raise this.
func TestToolDefinitionIsSmall(t *testing.T) {
	toolConfig(t, `{"members":[{"name":"a","base_url":"http://127.0.0.1:1/v1","model":"m"}]}`)
	b, err := json.Marshal(Tools()[0].Spec)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("council_ask definition: %d JSON chars", len(b))
	if len(b) > 520 {
		t.Fatalf("council_ask is %d chars; keep it under 520", len(b))
	}
}
