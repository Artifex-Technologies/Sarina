package council

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

const probeTimeout = 5 * time.Second

// Probe checks that a member's endpoint answers, with one cheap GET of its models
// list. No question or conversation text is sent; a remote member is sent only its
// own API key, as a header. It is run only when the operator asks (`council
// status --probe`), never as a side effect of a question.
func Probe(ctx context.Context, opt Options, m Member) (bool, string) {
	target, hdr := "", http.Header{}
	key := opt.key(m)
	switch m.Kind {
	case KindClaude:
		if key == "" {
			return false, "environment variable " + m.APIKeyEnv + " is not set"
		}
		ac := &llm.AnthropicClient{Config: config.ModelConfig{Model: m.Model, BaseURL: m.BaseURL, APIKey: key}}
		target = ac.ModelsEndpoint()
		ac.SetHeaders(hdr)
	default:
		target = strings.TrimRight(m.BaseURL, "/") + "/models"
		if m.APIKeyEnv != "" {
			if key == "" {
				return false, "environment variable " + m.APIKeyEnv + " is not set"
			}
			hdr.Set("Authorization", "Bearer "+key)
		}
	}
	host := target
	if u, err := url.Parse(target); err == nil && u.Host != "" {
		host = u.Host
	}
	cctx, cancel := context.WithTimeout(ctx, probeTimeout)
	defer cancel()
	req, err := http.NewRequestWithContext(cctx, http.MethodGet, target, nil)
	if err != nil {
		return false, "bad base_url"
	}
	for k, v := range hdr {
		req.Header[k] = v
	}
	hc := opt.HTTPClient
	if hc == nil {
		hc = http.DefaultClient
	}
	resp, err := hc.Do(req)
	if err != nil {
		return false, "unreachable (" + host + ")"
	}
	resp.Body.Close()
	switch {
	case resp.StatusCode == 401 || resp.StatusCode == 403:
		return false, fmt.Sprintf("key rejected (HTTP %d)", resp.StatusCode)
	case resp.StatusCode >= 500:
		return false, fmt.Sprintf("unhealthy (HTTP %d from %s)", resp.StatusCode, host)
	}
	return true, ""
}
