package heimdall

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// useGrants points the reader at a decisions file in a fresh folder the test owns (standing in for root).
func useGrants(t *testing.T, body string, mode os.FileMode) string {
	t.Helper()
	dir := t.TempDir()
	os.Chmod(dir, 0o755)
	p := filepath.Join(dir, "grants.json")
	if body != "" {
		if err := os.WriteFile(p, []byte(body), mode); err != nil {
			t.Fatal(err)
		}
		os.Chmod(p, mode)
	}
	oldP, oldO := GrantsPath, GrantsOwner
	GrantsPath, GrantsOwner = p, uint32(os.Getuid())
	t.Cleanup(func() { GrantsPath, GrantsOwner = oldP, oldO })
	return p
}

func TestGrantsGate(t *testing.T) {
	now := time.Date(2026, 11, 5, 12, 0, 0, 0, time.UTC)
	until := now.Add(time.Hour).Format(time.RFC3339)

	useGrants(t, "", 0)
	if g := LoadGrants(); g.On || g.TierCap(3) != 3 {
		t.Fatalf("no file: gate off, own setting kept: %+v", g)
	}

	useGrants(t, `{"access_tier": 1, "escalations": [{"tier": "claude-cli", "until": "`+until+`"}]}`, 0o644)
	g := LoadGrants()
	if !g.On || g.TierCap(3) != 1 || g.TierCap(0) != 0 {
		t.Fatalf("the cap is the lower of the two: %+v", g)
	}
	if !g.EscalationAllowed("claude-cli", "any", now) || g.EscalationAllowed("claude-cli", "", now.Add(2*time.Hour)) || g.EscalationAllowed("claude", "", now) {
		t.Fatal("an approval covers its tier and its time only")
	}

	useGrants(t, `{"escalations": []}`, 0o644)
	if g := LoadGrants(); g.TierCap(3) != 0 {
		t.Fatal("gate on without a cap: read-only")
	}

	for _, c := range []struct {
		body string
		mode os.FileMode
	}{
		{`{"access_tier": 3, "escalations": [{"tier": "claude-cli", "until": "` + until + `"}]}`, 0o666}, // others could write it
		{`not json`, 0o644},
	} {
		useGrants(t, c.body, c.mode)
		g := LoadGrants()
		if !g.On || g.Problem == "" || g.TierCap(3) != 0 || g.EscalationAllowed("claude-cli", "", now) {
			t.Fatalf("an untrusted or damaged file must allow nothing: %q %+v", c.body, g)
		}
	}
}

func TestRequestGrantFilesAndNeverApproves(t *testing.T) {
	var got map[string]string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/requests" || r.Method != http.MethodPost {
			http.NotFound(w, r)
			return
		}
		json.NewDecoder(r.Body).Decode(&got)
		w.Write([]byte(`{"id":"ab12","status":"pending","approve":"sudo heimdalld grant approve ab12"}`))
	}))
	defer srv.Close()
	t.Setenv(EnvVar, srv.URL+"/v1/review")
	how, err := RequestGrant("escalation", "claude-cli", "", "stuck")
	if err != nil || !strings.Contains(how, "grant approve ab12") {
		t.Fatalf("%q %v", how, err)
	}
	if got["kind"] != "escalation" || got["detail"] != "claude-cli" {
		t.Fatalf("%v", got)
	}
	t.Setenv(EnvVar, "http://127.0.0.1:1/v1/review")
	if _, err := RequestGrant("escalation", "claude-cli", "", ""); err == nil {
		t.Fatal("an unreachable Heimdall is an error, never an approval")
	}
}
