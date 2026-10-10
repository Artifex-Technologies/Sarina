// Package heimdall is the optional consultation with Heimdall's governance
// API (POST /v1/review). It is an out-of-process second opinion: a separate
// OS process this one cannot alter. (Heimdall was called Chymaera_Sentry.)
//
// Absence fails open: with HEIMDALL_REVIEW_URL unset, or Heimdall unreachable
// or misbehaving, Consult returns nil and callers rely on their own checks. An
// actual "deny" fails closed. The only caller is the shared scan gate in
// package guard; keep it that way.
package heimdall

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"strings"
	"time"
)

// EnvVar holds the full review URL, e.g. http://127.0.0.1:8930/v1/review.
const EnvVar = "HEIMDALL_REVIEW_URL"

// LegacyEnvVar is the pre-rename name, still honoured when EnvVar is unset so
// existing deployments keep working.
const LegacyEnvVar = "SENTRY_REVIEW_URL"

// URL returns the configured review URL through getenv: EnvVar, else LegacyEnvVar.
func URL(getenv func(string) string) string {
	if v := strings.TrimSpace(getenv(EnvVar)); v != "" {
		return v
	}
	return strings.TrimSpace(getenv(LegacyEnvVar))
}

// Timeout bounds one consultation.
const Timeout = 2 * time.Second

// Verdict is what Heimdall answered.
type Verdict struct {
	Denied  bool
	Reasons []string
}

// Consult asks Heimdall about text. It returns nil when Heimdall is not
// configured or gave no usable answer; it never fails.
func Consult(text, kind, sessionID string) *Verdict {
	url := URL(os.Getenv)
	if url == "" {
		return nil
	}
	payload, err := json.Marshal(map[string]string{"kind": kind, "session_id": sessionID, "text": text})
	if err != nil {
		return nil
	}
	req, err := http.NewRequest(http.MethodPost, url, bytes.NewReader(payload))
	if err != nil {
		return nil
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := (&http.Client{Timeout: Timeout}).Do(req)
	if err != nil {
		return nil
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil || resp.StatusCode >= 300 {
		return nil
	}
	var body map[string]any
	if json.Unmarshal(raw, &body) != nil || body == nil {
		return nil
	}
	v := &Verdict{Denied: body["decision"] == "deny"}
	if rs, ok := body["reasons"].([]any); ok {
		for _, r := range rs {
			if s, ok := r.(string); ok {
				v.Reasons = append(v.Reasons, s)
			} else if r != nil {
				b, _ := json.Marshal(r)
				v.Reasons = append(v.Reasons, string(b))
			}
		}
	}
	return v
}
