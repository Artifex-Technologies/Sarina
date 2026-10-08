package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// Discovery defaults.
const (
	DefaultDiscoveryTimeout = 3 * time.Second
	DefaultDiscoveryTTL     = 5 * time.Minute
	minPlausibleWindow      = 256
	maxPlausibleWindow      = 100_000_000
)

// Discoverer asks a backend how large its context window is. Results (including
// "the server would not say") are cached per base URL and model for TTL, so the
// answer costs at most one short round of probes per interval.
type Discoverer struct {
	HTTPClient *http.Client  // optional
	Timeout    time.Duration // total budget for one discovery; 0 = DefaultDiscoveryTimeout
	TTL        time.Duration // 0 = DefaultDiscoveryTTL
	Now        func() time.Time

	mu    sync.Mutex
	cache map[string]discovered
}

type discovered struct {
	n      int
	source string
	at     time.Time
}

// DefaultDiscoverer is shared by every agent in the process.
var DefaultDiscoverer = &Discoverer{}

// ContextWindow returns the window the backend reports for cfg.Model and where
// the number came from ("ollama", "llama.cpp", "models"). n == 0 means the
// backend did not say (or was unreachable); the caller falls back to its own
// inference.
func (d *Discoverer) ContextWindow(ctx context.Context, cfg config.ModelConfig) (n int, source string) {
	now := time.Now
	if d.Now != nil {
		now = d.Now
	}
	ttl := d.TTL
	if ttl <= 0 {
		ttl = DefaultDiscoveryTTL
	}
	key := cfg.BaseURL + "\x00" + cfg.Model
	d.mu.Lock()
	if hit, ok := d.cache[key]; ok && now().Sub(hit.at) < ttl {
		d.mu.Unlock()
		return hit.n, hit.source
	}
	d.mu.Unlock()

	timeout := d.Timeout
	if timeout <= 0 {
		timeout = DefaultDiscoveryTimeout
	}
	cctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	n, source = d.probe(cctx, cfg)
	if ctx.Err() != nil {
		return n, source // cancelled by the caller: do not cache a half answer
	}
	d.mu.Lock()
	if d.cache == nil {
		d.cache = map[string]discovered{}
	}
	d.cache[key] = discovered{n, source, now()}
	d.mu.Unlock()
	return n, source
}

var errTransport = errors.New("transport failure")

// get/post return the body of a 2xx reply; a non-2xx status is (nil, nil) so
// the next probe is tried, while a transport failure aborts the whole discovery
// (an unreachable server will not answer the next probe either).
func (d *Discoverer) do(ctx context.Context, cfg config.ModelConfig, method, u string, body any) ([]byte, error) {
	var rd io.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	}
	req, err := http.NewRequestWithContext(ctx, method, u, rd)
	if err != nil {
		return nil, nil
	}
	req.Header.Set("Authorization", "Bearer "+cfg.APIKey)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	hc := d.HTTPClient
	if hc == nil {
		hc = http.DefaultClient
	}
	resp, err := hc.Do(req)
	if err != nil {
		return nil, errTransport
	}
	defer resp.Body.Close()
	b, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return nil, errTransport
	}
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return nil, nil
	}
	return b, nil
}

// serverRoot strips a trailing /v1 (or /v1/...) so native endpoints such as
// /api/show and /props can be addressed.
func serverRoot(baseURL string) string {
	u, err := url.Parse(strings.TrimRight(baseURL, "/"))
	if err != nil || u.Host == "" {
		return strings.TrimRight(baseURL, "/")
	}
	p := u.Path
	if i := strings.Index(p, "/v1"); i >= 0 {
		p = p[:i]
	}
	u.Path, u.RawQuery, u.Fragment = p, "", ""
	return strings.TrimRight(u.String(), "/")
}

func plausible(n int) bool { return n >= minPlausibleWindow && n <= maxPlausibleWindow }

var numCtxRe = regexp.MustCompile(`(?m)^\s*num_ctx\s+(\d+)`)

func (d *Discoverer) probe(ctx context.Context, cfg config.ModelConfig) (int, string) {
	root := serverRoot(cfg.BaseURL)

	// Ollama. /api/ps reports the context the loaded model really runs with;
	// /api/show gives the configured num_ctx, else the model's trained maximum.
	if b, err := d.do(ctx, cfg, http.MethodGet, root+"/api/ps", nil); err != nil {
		return 0, ""
	} else if b != nil {
		var ps struct {
			Models []struct {
				Name, Model   string
				ContextLength int `json:"context_length"`
			} `json:"models"`
		}
		if json.Unmarshal(b, &ps) == nil {
			for _, m := range ps.Models {
				if sameModel(m.Name, cfg.Model) || sameModel(m.Model, cfg.Model) {
					if plausible(m.ContextLength) {
						return m.ContextLength, "ollama"
					}
				}
			}
		}
	}
	if b, err := d.do(ctx, cfg, http.MethodPost, root+"/api/show", map[string]any{"model": cfg.Model, "name": cfg.Model}); err != nil {
		return 0, ""
	} else if b != nil {
		var show struct {
			Parameters string         `json:"parameters"`
			ModelInfo  map[string]any `json:"model_info"`
		}
		if json.Unmarshal(b, &show) == nil {
			if m := numCtxRe.FindStringSubmatch(show.Parameters); m != nil {
				if n, _ := strconv.Atoi(m[1]); plausible(n) {
					return n, "ollama"
				}
			}
			for k, v := range show.ModelInfo {
				if strings.HasSuffix(k, ".context_length") {
					if n, _ := config.AsInt(v); plausible(n) {
						return n, "ollama"
					}
				}
			}
		}
	}

	// llama.cpp server: the context it was launched with.
	if b, err := d.do(ctx, cfg, http.MethodGet, root+"/props", nil); err != nil {
		return 0, ""
	} else if b != nil {
		var props map[string]any
		if json.Unmarshal(b, &props) == nil {
			for _, v := range []any{config.AsMap(props["default_generation_settings"])["n_ctx"], props["n_ctx"]} {
				if n, _ := config.AsInt(v); plausible(n) {
					return n, "llama.cpp"
				}
			}
		}
	}

	// OpenAI-compatible model listing (vLLM, LM Studio, OpenRouter, ...).
	b, err := d.do(ctx, cfg, http.MethodGet, strings.TrimRight(cfg.BaseURL, "/")+"/models", nil)
	if err != nil || b == nil {
		return 0, ""
	}
	var list struct {
		Data []map[string]any `json:"data"`
	}
	if json.Unmarshal(b, &list) != nil {
		return 0, ""
	}
	pickEntry := func() map[string]any {
		for _, m := range list.Data {
			if id, _ := m["id"].(string); sameModel(id, cfg.Model) {
				return m
			}
		}
		if len(list.Data) == 1 {
			return list.Data[0]
		}
		return nil
	}
	if m := pickEntry(); m != nil {
		fields := []any{m["max_model_len"], m["context_length"], m["context_window"], m["max_context_length"],
			m["loaded_context_length"], m["n_ctx"], config.AsMap(m["meta"])["n_ctx"], config.AsMap(m["meta"])["n_ctx_train"],
			config.AsMap(m["top_provider"])["context_length"]}
		for _, v := range fields {
			if _, isBool := v.(bool); isBool {
				continue
			}
			if n, _ := config.AsInt(v); plausible(n) {
				return n, "models"
			}
		}
	}
	return 0, ""
}

// sameModel compares model names the way servers differ on them: case-insensitive,
// and an Ollama ":latest" tag is optional.
func sameModel(a, b string) bool {
	norm := func(s string) string {
		s = strings.ToLower(strings.TrimSpace(s))
		return strings.TrimSuffix(s, ":latest")
	}
	return a != "" && b != "" && norm(a) == norm(b)
}
