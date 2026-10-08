package doctor

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

const minUsableWindow = 8192

const ollamaHint = "raise it: `PARAMETER num_ctx 16384` in the Modelfile or OLLAMA_CONTEXT_LENGTH=16384, or set SARINA_CONTEXT_WINDOW if the server's real window is larger"

// modelListed accepts an exact id or Ollama's "name:tag" form of the configured name.
func modelListed(ids []string, model string) bool {
	for _, id := range ids {
		if id == model || strings.HasPrefix(id, model+":") || strings.HasPrefix(model, id+":") {
			return true
		}
	}
	return false
}

func (r *runner) backend(ctx context.Context) {
	cfg := config.ModelFromEnv(r.env)
	where := hostOf(cfg.BaseURL)
	key := "API key " + secretState(r.env("OPENAI_API_KEY"))
	hdr := http.Header{}
	if cfg.APIKey != "" {
		hdr.Set("Authorization", "Bearer "+cfg.APIKey)
	}
	status, body, err := r.get(ctx, strings.TrimRight(cfg.BaseURL, "/")+"/models", hdr)
	switch {
	case err != nil:
		r.add("backend", Fail, fmt.Sprintf("model server at %s is not reachable (%s)", where, key),
			"start the model server, or point OPENAI_BASE_URL at it (Ollama: http://127.0.0.1:11434/v1)")
		r.add("context_window", Skip, "not checked: the backend is unreachable", "")
		return
	case status == http.StatusUnauthorized || status == http.StatusForbidden:
		r.add("backend", Fail, fmt.Sprintf("model server at %s refused the credentials (HTTP %d; %s)", where, status, key),
			"set OPENAI_API_KEY to a key the server accepts (Ollama accepts any non-empty value)")
	case status < 200 || status > 299:
		r.add("backend", Fail, fmt.Sprintf("model server at %s answered HTTP %d on /models", where, status),
			"check the server log; OPENAI_BASE_URL should end in /v1")
	default:
		var doc struct {
			Data []struct {
				ID string `json:"id"`
			} `json:"data"`
		}
		_ = json.Unmarshal(body, &doc)
		ids := make([]string, 0, len(doc.Data))
		for _, d := range doc.Data {
			ids = append(ids, d.ID)
		}
		switch {
		case len(ids) == 0:
			r.add("backend", Pass, fmt.Sprintf("%s reachable; it did not list models, so %q was not confirmed (%s)", where, cfg.Model, key), "")
		case modelListed(ids, cfg.Model):
			r.add("backend", Pass, fmt.Sprintf("%s reachable and lists model %q (%s)", where, cfg.Model, key), "")
		default:
			r.add("backend", Warn, fmt.Sprintf("%s reachable but does not list model %q (%d other models)", where, cfg.Model, len(ids)),
				"set OPENAI_MODEL to a listed name, or pull/load the model on the server")
		}
	}
	if status < 200 || status > 299 {
		r.add("context_window", Skip, "not checked: the backend did not answer /models normally", "")
		return
	}
	n, source := config.ContextWindowFromEnv(r.env), "SARINA_CONTEXT_WINDOW"
	if n == 0 {
		d := &llm.Discoverer{HTTPClient: r.hc, Timeout: ShortTimeout}
		n, source = d.ContextWindow(ctx, cfg)
	}
	switch {
	case n == 0:
		r.add("context_window", Pass, "the backend does not report its context window; Sarina infers it from the model name", "")
	case n < minUsableWindow:
		r.add("context_window", Warn, fmt.Sprintf("context window is %d tokens (from %s); Sarina needs at least %d", n, source, minUsableWindow), ollamaHint)
	default:
		r.add("context_window", Pass, fmt.Sprintf("context window is %d tokens (from %s)", n, source), "")
	}
}
