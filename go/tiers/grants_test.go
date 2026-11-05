package tiers

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/heimdall"
)

// grantGate switches Heimdall's gate on for the test with the given decisions, and counts requests filed with a fake
// Heimdall.
func grantGate(t *testing.T, body string) *int32 {
	t.Helper()
	dir := t.TempDir()
	os.Chmod(dir, 0o755)
	p := filepath.Join(dir, "grants.json")
	if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	oldP, oldO := heimdall.GrantsPath, heimdall.GrantsOwner
	heimdall.GrantsPath, heimdall.GrantsOwner = p, uint32(os.Getuid())
	t.Cleanup(func() { heimdall.GrantsPath, heimdall.GrantsOwner = oldP, oldO })
	var filed int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&filed, 1)
		w.Write([]byte(`{"id":"ab12","status":"pending","approve":"sudo heimdalld grant approve ab12"}`))
	}))
	t.Cleanup(srv.Close)
	t.Setenv(heimdall.EnvVar, srv.URL+"/v1/review")
	return &filed
}

// With the gate on, an auto hand-off to the claude CLI no longer happens by itself: a request is filed and the tier is
// skipped; with the owner's approval in the file it runs.
func TestCLIHandOffNeedsTheOwnersGrant(t *testing.T) {
	filed := grantGate(t, `{"access_tier": 0, "escalations": []}`)
	c := newCLIRig(t, deadURL(t), "ok", nil)
	runAgent(t, c.rig, "refactor the parser please")
	if c.ran() {
		t.Fatal("the claude CLI ran without the owner's approval")
	}
	if atomic.LoadInt32(filed) != 1 {
		t.Fatalf("one request should be filed with Heimdall, got %d", *filed)
	}

	until := time.Date(2026, 10, 5, 10, 0, 0, 0, time.UTC).Format(time.RFC3339) // the rig's clock is 2026-10-05 09:00
	grantGate(t, `{"access_tier": 0, "escalations": [{"tier": "claude-cli", "until": "`+until+`"}]}`)
	c = newCLIRig(t, deadURL(t), "ok", nil)
	res, _ := runAgent(t, c.rig, "refactor the parser please")
	if !c.ran() || res.FinalOutput != "done by claude" {
		t.Fatalf("approved hand-off did not run: %q", res.FinalOutput)
	}
}

func TestOffMachine(t *testing.T) {
	for _, c := range []struct {
		t    Tier
		want bool
	}{
		{Tier{Kind: KindClaudeCLI}, true},
		{Tier{Kind: KindClaude, BaseURL: "http://127.0.0.1:1"}, true},
		{Tier{Kind: KindOpenAICompat, BaseURL: "http://127.0.0.1:11434/v1"}, false},
		{Tier{Kind: KindOpenAICompat, BaseURL: "http://localhost:11434/v1"}, false},
		{Tier{Kind: KindOpenAICompat, BaseURL: "http://gpu-box.lan:11434/v1"}, true},
		{Tier{Kind: KindOpenAICompat, BaseURL: "::"}, true},
	} {
		if got := offMachine(c.t); got != c.want {
			t.Errorf("%+v: %v", c.t, got)
		}
	}
}
