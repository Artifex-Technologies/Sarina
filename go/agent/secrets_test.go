package agent

import (
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/session"
)

func TestHideSecretsRead(t *testing.T) {
	file := "SERVICE_URL=http://127.0.0.1:9000\npassword=hunter2-correct-horse-9\nexport API_KEY=\"abcd1234efgh\"\nLOG_LEVEL=info\nDB_PASSWORD: s3cr3t!x\n"
	msgs := []session.Message{{Role: "tool", Content: file}}
	ans := "password is hunter2-correct-horse-9, the key abcd1234efgh, db s3cr3t!x, log level info, url http://127.0.0.1:9000"
	got, changed := hideSecretsRead(ans, msgs)
	if !changed || strings.Contains(got, "hunter2") || strings.Contains(got, "abcd1234efgh") || strings.Contains(got, "s3cr3t!x") {
		t.Fatalf("%q", got)
	}
	if !strings.Contains(got, "info") || !strings.Contains(got, "127.0.0.1:9000") {
		t.Fatalf("ordinary values must stay: %q", got)
	}
	if _, changed := hideSecretsRead("nothing secret here", msgs); changed {
		t.Fatal("changed an answer with no secret")
	}
}
