package agent

import (
	"encoding/json"
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

// The leak found 2026-10-26: a passphrase in a file read through a tool whose result is JSON was said back.
func TestSecretsInJSONToolResultsAreHidden(t *testing.T) {
	file := "SERVICE_URL=http://127.0.0.1:9000\npassword = \"hunter2-correct-horse-9\"\nLOG_LEVEL=info\n"
	raw, _ := json.Marshal(map[string]any{"tool": "read_file", "ok": true, "content": file})
	out, changed := hideSecretsRead("the value is hunter2-correct-horse-9", []session.Message{{Role: "tool", Content: string(raw)}})
	if !changed || strings.Contains(out, "hunter2") {
		t.Fatalf("not hidden: %q", out)
	}
	nested, _ := json.Marshal(map[string]any{"results": []any{map[string]any{"text": "api_token: abcd1234efgh"}}})
	if out, _ := hideSecretsRead("token abcd1234efgh", []session.Message{{Role: "tool", Content: string(nested)}}); strings.Contains(out, "abcd1234efgh") {
		t.Fatalf("nested JSON not hidden: %q", out)
	}
	if out, changed := hideSecretsRead("SERVICE_URL points at the local service", []session.Message{{Role: "tool", Content: string(raw)}}); changed {
		t.Fatalf("a setting name is not a secret: %q", out)
	}
}

func TestStreamedRepliesHideSecretsAsTheyGo(t *testing.T) {
	msgs := []session.Message{{Role: "tool", Content: `{"tool":"read_file","ok":true,"content":"user = matt\npassword = \"hunter2-correct-horse\"\n"}`}}
	red := newStreamRedactor(msgs)
	if red == nil {
		t.Fatal("no redactor although the output holds a password")
	}
	reply := "The file sets the user to matt and the password to hunter2-correct-horse, which you should rotate."
	var out strings.Builder
	for i := 0; i < len(reply); i += 3 { // token-sized pieces, the secret split across several
		out.WriteString(red.push(reply[i:min(i+3, len(reply))]))
		if strings.Contains(out.String(), "hunter2") {
			t.Fatalf("part of the secret went out: %q", out.String())
		}
	}
	out.WriteString(red.flush())
	if want := "The file sets the user to matt and the password to [hidden], which you should rotate."; out.String() != want {
		t.Fatalf("got %q", out.String())
	}
	if newStreamRedactor([]session.Message{{Role: "tool", Content: "nothing secret here"}}) != nil {
		t.Fatal("without secrets nothing is held back")
	}
}
