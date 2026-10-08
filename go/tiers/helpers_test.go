package tiers

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

const (
	localKey  = "local-key-0123456789"
	claudeKey = "sk-ant-api03-TESTKEY-do-not-leak-0123456789"
)

// fakeBackend serves an OpenAI-compatible API (/models, /chat/completions) or
// the Anthropic one (/v1/models, /v1/messages) and records what it is sent.
type fakeBackend struct {
	*httptest.Server
	claude bool
	reply  string
	down   atomic.Bool
	delay  atomic.Int64 // nanoseconds, applied to the model call only

	mu      sync.Mutex
	bodies  []string // raw model-call bodies
	headers []http.Header
	health  int
}

func newBackend(t *testing.T, claude bool, reply string) *fakeBackend {
	t.Helper()
	f := &fakeBackend{claude: claude, reply: reply}
	f.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if f.down.Load() {
			w.WriteHeader(503)
			return
		}
		switch {
		case r.Method == http.MethodGet && strings.HasSuffix(r.URL.Path, "/models"):
			f.mu.Lock()
			f.health++
			f.mu.Unlock()
			io.WriteString(w, `{"data":[]}`)
		case r.Method == http.MethodPost:
			b, _ := io.ReadAll(r.Body)
			f.mu.Lock()
			f.bodies = append(f.bodies, string(b))
			f.headers = append(f.headers, r.Header.Clone())
			f.mu.Unlock()
			if d := f.delay.Load(); d > 0 {
				time.Sleep(time.Duration(d))
			}
			var out any
			if f.claude {
				out = map[string]any{"type": "message", "role": "assistant", "stop_reason": "end_turn",
					"content": []any{map[string]any{"type": "text", "text": f.reply}},
					"usage":   map[string]any{"input_tokens": 7, "output_tokens": 3}}
			} else {
				out = map[string]any{"choices": []any{map[string]any{"message": map[string]any{"role": "assistant", "content": f.reply}, "finish_reason": "stop"}},
					"usage": map[string]any{"prompt_tokens": 5, "completion_tokens": 2}}
			}
			j, _ := json.Marshal(out)
			w.Write(j)
		default:
			w.WriteHeader(404)
		}
	}))
	t.Cleanup(f.Close)
	return f
}

func (f *fakeBackend) calls() int { f.mu.Lock(); defer f.mu.Unlock(); return len(f.bodies) }
func (f *fakeBackend) healthChecks() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.health
}
func (f *fakeBackend) lastBody() string {
	f.mu.Lock()
	defer f.mu.Unlock()
	if len(f.bodies) == 0 {
		return ""
	}
	return f.bodies[len(f.bodies)-1]
}

// deadURL is an address nothing listens on.
func deadURL(t *testing.T) string {
	t.Helper()
	s := httptest.NewServer(http.NotFoundHandler())
	u := s.URL
	s.Close()
	return u
}

type fakeSampler struct{ s Sample }

func (f *fakeSampler) Sample(context.Context) Sample { return f.s }

type clock struct {
	mu sync.Mutex
	t  time.Time
}

func (c *clock) Now() time.Time { c.mu.Lock(); defer c.mu.Unlock(); return c.t }
func (c *clock) Advance(d time.Duration) {
	c.mu.Lock()
	c.t = c.t.Add(d)
	c.mu.Unlock()
}

func newClock() *clock { return &clock{t: time.Date(2026, 10, 5, 9, 0, 0, 0, time.UTC)} }

func envOf(m map[string]string) func(string) string {
	return func(k string) string { return m[k] }
}

func localTier(url string) Tier {
	return Tier{Name: "local", Kind: KindOpenAICompat, BaseURL: url, Model: "local-model", AllowData: AllowData{All: true}, Enabled: true, Auto: true}
}

func desktopTier(url string) Tier {
	return Tier{Name: "desktop", Kind: KindOpenAICompat, BaseURL: url, Model: "big-model",
		AllowData: AllowData{Classes: []string{ClassNoSecrets}}, Enabled: true, Auto: true}
}

func claudeTier(url string) Tier {
	return Tier{Name: "claude", Kind: KindClaude, BaseURL: url, Model: "claude-test", APIKeyEnv: "ANTHROPIC_TEST_KEY",
		AllowData: AllowData{Classes: []string{ClassNoSecrets, ClassNoCaptures}}, Enabled: true, Auto: true}
}

type rig struct {
	r   *Router
	clk *clock
	smp *fakeSampler
	cfg Config
}

func newRig(t *testing.T, tiers ...Tier) *rig {
	t.Helper()
	g := &rig{clk: newClock(), smp: &fakeSampler{Sample{MemPercent: 20, MemOK: true}}}
	g.cfg = Config{Tiers: tiers, Resource: Thresholds{MaxMemoryPercent: 90, MaxGPUMemoryPercent: 90}}
	var err error
	g.r, err = New(Options{Config: g.cfg,
		Base:    config.ModelConfig{Model: "local-model", BaseURL: tiers[0].BaseURL, APIKey: localKey, TimeoutSeconds: 5},
		Getenv:  envOf(map[string]string{"ANTHROPIC_TEST_KEY": claudeKey}),
		Sampler: g.smp, Now: g.clk.Now, HealthTimeout: time.Second, WakePoll: 5 * time.Millisecond})
	if err != nil {
		t.Fatal(err)
	}
	return g
}

func userReq(text string) llm.Request {
	return llm.Request{Messages: []llm.Message{{Role: "system", Content: "be brief"}, {Role: "user", Content: text}}}
}

// runAgent drives one full agent turn through the router and returns the
// result and the workspace directory.
func runAgent(t *testing.T, g *rig, prompt string) (*agent.Result, string) {
	t.Helper()
	dir := t.TempDir()
	rt := config.DefaultRuntime(dir)
	rt.DisableClaudeMdDiscovery = true
	a, err := agent.New(agent.Options{Model: config.ModelConfig{Model: "local-model", BaseURL: g.cfg.Tiers[0].BaseURL, APIKey: localKey, TimeoutSeconds: 5},
		Runtime: rt, Client: g.r, Tiers: g.r, Home: t.TempDir(), Getenv: func(string) string { return "" }})
	if err != nil {
		t.Fatal(err)
	}
	res, err := a.Run(context.Background(), prompt)
	if res == nil {
		t.Fatalf("no result: %v", err)
	}
	return res, dir
}

func readJSON(t *testing.T, path string) map[string]any {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatal(err)
	}
	return m
}

func tierRecord(t *testing.T, sessionPath string) map[string]any {
	t.Helper()
	ps, _ := readJSON(t, sessionPath)["plugin_state"].(map[string]any)
	rec, _ := ps[SessionKey].(map[string]any)
	return rec
}

func eventsOfType(res *agent.Result, typ string) []map[string]any {
	var out []map[string]any
	for _, e := range res.Events {
		if e["type"] == typ {
			out = append(out, e)
		}
	}
	return out
}

// filesContain walks dir and returns every file whose bytes contain needle.
func filesContain(t *testing.T, dir, needle string) []string {
	t.Helper()
	var hits []string
	filepath.Walk(dir, func(p string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() {
			if b, rerr := os.ReadFile(p); rerr == nil && strings.Contains(string(b), needle) {
				hits = append(hits, p)
			}
		}
		return nil
	})
	return hits
}
