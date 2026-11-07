package heimdall

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
)

// DraftRule sends a YARA rule draft for a sample Heimdall holds (POST /v1/rules/drafts, Heimdall's docs/IMMUNITY.md). Heimdall
// checks its form and keeps it as a draft; testing it needs root and turning it on needs the owner, so a draft changes nothing
// by itself. It returns the draft's id and the owner's next command.
func DraftRule(sampleSHA256, rule, note string) (id, next string, err error) {
	base := "http://127.0.0.1:8930"
	if u, perr := url.Parse(URL(os.Getenv)); perr == nil && u.Host != "" {
		base = u.Scheme + "://" + u.Host
	}
	payload, _ := json.Marshal(map[string]string{"sample_sha256": sampleSHA256, "rule": rule, "note": note})
	req, err := http.NewRequest(http.MethodPost, base+"/v1/rules/drafts", bytes.NewReader(payload))
	if err != nil {
		return "", "", err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := (&http.Client{Timeout: Timeout}).Do(req)
	if err != nil {
		return "", "", errors.New("Heimdall is not reachable: " + err.Error())
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(io.LimitReader(resp.Body, 64*1024))
	if resp.StatusCode != http.StatusOK {
		return "", "", errors.New("Heimdall answered " + resp.Status + ": " + strings.TrimSpace(string(raw)))
	}
	var out struct{ ID, Next string }
	if json.Unmarshal(raw, &out) != nil || out.ID == "" {
		return "", "", errors.New("Heimdall's answer could not be read")
	}
	return out.ID, out.Next, nil
}
