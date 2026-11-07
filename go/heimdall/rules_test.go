package heimdall

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestDraftRuleSendsTheDraftAndReadsTheAnswer(t *testing.T) {
	var got map[string]string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/rules/drafts" || r.Method != http.MethodPost {
			http.NotFound(w, r)
			return
		}
		json.NewDecoder(r.Body).Decode(&got)
		if got["sample_sha256"] == "unknown" {
			http.Error(w, "sample_sha256 is not a sample Heimdall holds", http.StatusBadRequest)
			return
		}
		w.Write([]byte(`{"id":"r-1","status":"draft","next":"sudo heimdalld immunity rule test r-1"}`))
	}))
	defer srv.Close()
	t.Setenv(EnvVar, srv.URL+"/v1/review")
	id, next, err := DraftRule("abc", "rule x { condition: true }", "test")
	if err != nil || id != "r-1" || !strings.Contains(next, "rule test r-1") || got["rule"] != "rule x { condition: true }" {
		t.Fatalf("%q %q %v %v", id, next, err, got)
	}
	if _, _, err := DraftRule("unknown", "r", ""); err == nil || !strings.Contains(err.Error(), "not a sample Heimdall holds") {
		t.Fatalf("Heimdall's refusal is passed on: %v", err)
	}
}
