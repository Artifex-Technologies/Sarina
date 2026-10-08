// Package sentry is the optional consultation with Chymaera Sentry's
// governance API (POST /v1/review). It is an out-of-process second opinion:
// a separate OS process this one cannot alter.
//
// Absence fails open: with SENTRY_REVIEW_URL unset, or Sentry unreachable or
// misbehaving, Consult returns nil and callers rely on their own checks. An
// actual "deny" fails closed. The only caller is the shared scan gate in
// package guard; keep it that way.
package sentry

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
const EnvVar = "SENTRY_REVIEW_URL"

// Timeout bounds one consultation.
const Timeout = 2 * time.Second

// Verdict is what Sentry answered.
type Verdict struct {
	Denied  bool
	Reasons []string
}

// Consult asks Sentry about text. It returns nil when Sentry is not
// configured or gave no usable answer; it never fails.
func Consult(text, kind, sessionID string) *Verdict {
	url := strings.TrimSpace(os.Getenv(EnvVar))
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
