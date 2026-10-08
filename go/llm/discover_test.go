package llm

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// backend is a fake server whose routes are a map of "METHOD /path" to a
// status and JSON body; anything else is a 404.
type backend struct {
	*httptest.Server
	mu   sync.Mutex
	hits []string
}

type route struct {
	status int
	body   any
	delay  time.Duration
}

func newBackend(t *testing.T, routes map[string]route) *backend {
	t.Helper()
	b := &backend{}
	b.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		key := r.Method + " " + r.URL.Path
		b.mu.Lock()
		b.hits = append(b.hits, key)
		b.mu.Unlock()
		rt, ok := routes[key]
		if !ok {
			http.NotFound(w, r)
			return
		}
		if rt.delay > 0 {
			time.Sleep(rt.delay)
		}
		if rt.status != 0 {
			w.WriteHeader(rt.status)
		}
		_ = json.NewEncoder(w).Encode(rt.body)
	}))
	t.Cleanup(b.Close)
	return b
}

func (b *backend) hitCount() int {
	b.mu.Lock()
	defer b.mu.Unlock()
	return len(b.hits)
}

func cfgFor(b *backend, model string) config.ModelConfig {
	return config.ModelConfig{Model: model, BaseURL: b.URL + "/v1", APIKey: "k"}
}

func TestDiscoverOllamaLoadedModelWins(t *testing.T) {
	b := newBackend(t, map[string]route{
		"GET /api/ps": {body: map[string]any{"models": []any{
			map[string]any{"name": "other:latest", "context_length": 999999},
			map[string]any{"name": "qwen3:8b", "model": "qwen3:8b", "context_length": 16384}}}},
		"POST /api/show": {body: map[string]any{"parameters": "num_ctx 4096", "model_info": map[string]any{"qwen3.context_length": 40960}}},
	})
	n, src := (&Discoverer{}).ContextWindow(t.Context(), cfgFor(b, "qwen3:8b"))
	if n != 16384 || src != "ollama" {
		t.Fatalf("got %d %s", n, src)
	}
}

func TestDiscoverOllamaShowNumCtxThenModelInfo(t *testing.T) {
	show := func(params string, info map[string]any) map[string]route {
		return map[string]route{"POST /api/show": {body: map[string]any{"parameters": params, "model_info": info}}}
	}
	n, src := (&Discoverer{}).ContextWindow(t.Context(),
		cfgFor(newBackend(t, show("temperature 0.7\nnum_ctx                        8192\nstop \"x\"", map[string]any{"llama.context_length": 131072})), "llama3"))
	if n != 8192 || src != "ollama" {
		t.Fatalf("num_ctx: %d %s", n, src)
	}
	n, src = (&Discoverer{}).ContextWindow(t.Context(),
		cfgFor(newBackend(t, show("", map[string]any{"general.architecture": "llama", "llama.context_length": float64(131072)})), "llama3"))
	if n != 131072 || src != "ollama" {
		t.Fatalf("model_info: %d %s", n, src)
	}
}

func TestDiscoverOllamaShowReceivesTheModelName(t *testing.T) {
	var got map[string]any
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/api/show" {
			_ = json.NewDecoder(r.Body).Decode(&got)
			json.NewEncoder(w).Encode(map[string]any{"parameters": "num_ctx 2048"})
			return
		}
		http.NotFound(w, r)
	}))
	defer srv.Close()
	n, _ := (&Discoverer{}).ContextWindow(t.Context(), config.ModelConfig{Model: "m:1", BaseURL: srv.URL + "/v1"})
	if n != 2048 || got["model"] != "m:1" {
		t.Fatalf("n=%d body=%v", n, got)
	}
}

func TestDiscoverLlamaCppProps(t *testing.T) {
	b := newBackend(t, map[string]route{"GET /props": {body: map[string]any{"default_generation_settings": map[string]any{"n_ctx": 32768}}}})
	n, src := (&Discoverer{}).ContextWindow(t.Context(), cfgFor(b, "whatever.gguf"))
	if n != 32768 || src != "llama.cpp" {
		t.Fatalf("got %d %s", n, src)
	}
	b2 := newBackend(t, map[string]route{"GET /props": {body: map[string]any{"n_ctx": 4096}}})
	if n, _ := (&Discoverer{}).ContextWindow(t.Context(), cfgFor(b2, "m")); n != 4096 {
		t.Fatalf("older shape: %d", n)
	}
}

func TestDiscoverOpenAIModelListing(t *testing.T) {
	cases := map[string]struct {
		model string
		data  []any
		want  int
	}{
		"vllm":              {"org/m", []any{map[string]any{"id": "org/m", "max_model_len": 65536}}, 65536},
		"openrouter":        {"a/b", []any{map[string]any{"id": "x/y", "context_length": 1}, map[string]any{"id": "a/b", "context_length": 200000}}, 200000},
		"context_window":    {"m", []any{map[string]any{"id": "M", "context_window": 8192}}, 8192},
		"single entry":      {"alias", []any{map[string]any{"id": "real-name", "max_model_len": 12000}}, 12000},
		"llama.cpp meta":    {"m", []any{map[string]any{"id": "m", "meta": map[string]any{"n_ctx_train": 32768}}}, 32768},
		"two, none matches": {"zzz", []any{map[string]any{"id": "a", "max_model_len": 5000}, map[string]any{"id": "b", "max_model_len": 6000}}, 0},
		"no field":          {"m", []any{map[string]any{"id": "m"}}, 0},
		"implausible":       {"m", []any{map[string]any{"id": "m", "max_model_len": 3}}, 0},
		"bool is not a num": {"m", []any{map[string]any{"id": "m", "max_model_len": true}}, 0},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			b := newBackend(t, map[string]route{"GET /v1/models": {body: map[string]any{"data": c.data}}})
			n, src := (&Discoverer{}).ContextWindow(t.Context(), cfgFor(b, c.model))
			if n != c.want || (c.want > 0 && src != "models") {
				t.Fatalf("got %d %q, want %d", n, src, c.want)
			}
		})
	}
}

func TestDiscoverFallsBackWhenTheServerSaysNothing(t *testing.T) {
	// every route 404s, errors, or answers garbage: the caller gets 0 and infers
	for name, routes := range map[string]map[string]route{
		"404s":    {},
		"garbage": {"GET /v1/models": {body: "not an object"}, "GET /props": {body: []int{1}}, "POST /api/show": {body: 5}, "GET /api/ps": {body: nil}},
		"errors":  {"GET /v1/models": {status: 500, body: "x"}, "GET /props": {status: 503, body: "x"}},
	} {
		t.Run(name, func(t *testing.T) {
			b := newBackend(t, routes)
			if n, src := (&Discoverer{}).ContextWindow(t.Context(), cfgFor(b, "m")); n != 0 || src != "" {
				t.Fatalf("got %d %q", n, src)
			}
		})
	}
}

func TestDiscoverUnreachableServerIsFastAndZero(t *testing.T) {
	b := newBackend(t, nil)
	url := b.URL
	b.Close()
	start := time.Now()
	n, _ := (&Discoverer{}).ContextWindow(t.Context(), config.ModelConfig{Model: "m", BaseURL: url + "/v1"})
	if n != 0 {
		t.Fatalf("n = %d", n)
	}
	if time.Since(start) > 2*time.Second {
		t.Fatalf("took %s", time.Since(start))
	}
}

func TestDiscoverHonoursItsTimeout(t *testing.T) {
	b := newBackend(t, map[string]route{
		"GET /api/ps": {delay: 700 * time.Millisecond, body: map[string]any{}},
	})
	start := time.Now()
	n, _ := (&Discoverer{Timeout: 100 * time.Millisecond}).ContextWindow(t.Context(), cfgFor(b, "m"))
	if n != 0 || time.Since(start) > 600*time.Millisecond {
		t.Fatalf("n=%d after %s", n, time.Since(start))
	}
}

func TestDiscoverCachesAnswersAndNegativeAnswers(t *testing.T) {
	now := time.Unix(1000, 0)
	d := &Discoverer{TTL: time.Minute, Now: func() time.Time { return now }}
	b := newBackend(t, map[string]route{"GET /props": {body: map[string]any{"n_ctx": 9000}}})
	cfg := cfgFor(b, "m")
	d.ContextWindow(t.Context(), cfg)
	first := b.hitCount()
	if n, _ := d.ContextWindow(t.Context(), cfg); n != 9000 || b.hitCount() != first {
		t.Fatalf("second call hit the server (%d -> %d)", first, b.hitCount())
	}
	// a different model is a different entry
	d.ContextWindow(t.Context(), cfgFor(b, "other"))
	if b.hitCount() == first {
		t.Fatal("cache ignored the model")
	}
	// expiry
	now = now.Add(2 * time.Minute)
	before := b.hitCount()
	d.ContextWindow(t.Context(), cfg)
	if b.hitCount() == before {
		t.Fatal("entry did not expire")
	}
	// a silent server is cached too, so it is not re-probed every turn
	silent := newBackend(t, nil)
	scfg := cfgFor(silent, "m")
	d.ContextWindow(t.Context(), scfg)
	h := silent.hitCount()
	if n, _ := d.ContextWindow(t.Context(), scfg); n != 0 || silent.hitCount() != h {
		t.Fatal("negative answer not cached")
	}
}

func TestDiscoverDoesNotCacheACancelledAttempt(t *testing.T) {
	d := &Discoverer{}
	b := newBackend(t, map[string]route{"GET /props": {body: map[string]any{"n_ctx": 9000}}})
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	d.ContextWindow(ctx, cfgFor(b, "m"))
	if n, _ := d.ContextWindow(t.Context(), cfgFor(b, "m")); n != 9000 {
		t.Fatalf("n = %d", n)
	}
}

func TestDiscoverSendsTheAPIKeyAndUsesTheServerRoot(t *testing.T) {
	var auth, path string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/props" {
			auth, path = r.Header.Get("Authorization"), r.URL.Path
			fmt.Fprint(w, `{"n_ctx": 6000}`)
			return
		}
		http.NotFound(w, r)
	}))
	defer srv.Close()
	n, _ := (&Discoverer{}).ContextWindow(t.Context(), config.ModelConfig{Model: "m", APIKey: "secret-k", BaseURL: srv.URL + "/v1"})
	if n != 6000 || path != "/props" || auth != "Bearer secret-k" {
		t.Fatalf("n=%d path=%s auth=%s", n, path, auth)
	}
}

func TestServerRoot(t *testing.T) {
	for in, want := range map[string]string{
		"http://h:8000/v1":       "http://h:8000",
		"http://h:8000/v1/":      "http://h:8000",
		"http://h:8000":          "http://h:8000",
		"http://h/proxy/v1":      "http://h/proxy",
		"http://h:11434/v1/chat": "http://h:11434",
	} {
		if got := serverRoot(in); got != want {
			t.Errorf("%s -> %s, want %s", in, got, want)
		}
	}
	if !strings.HasPrefix(serverRoot("nonsense"), "nonsense") {
		t.Error("garbage must pass through")
	}
}
