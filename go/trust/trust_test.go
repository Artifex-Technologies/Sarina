package trust

import (
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Fixture writes a trust folder the way Heimdall does: mode, key, and a signed list of the given files' current content.
func Fixture(t *testing.T, mode string, files ...string) string {
	t.Helper()
	dir := t.TempDir()
	pub, priv, _ := ed25519.GenerateKey(rand.Reader)
	list := map[string]string{}
	for _, f := range files {
		b, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		s := sha256.Sum256(b)
		list[f] = hex.EncodeToString(s[:])
	}
	body, _ := json.MarshalIndent(map[string]any{"version": 1, "files": list}, "", "  ")
	os.WriteFile(filepath.Join(dir, "manifest.json"), body, 0o644)
	os.WriteFile(filepath.Join(dir, "manifest.sig"), []byte(base64.StdEncoding.EncodeToString(ed25519.Sign(priv, body))), 0o644)
	os.WriteFile(filepath.Join(dir, "owner.pub"), []byte(base64.StdEncoding.EncodeToString(pub)), 0o644)
	os.WriteFile(filepath.Join(dir, "mode"), []byte(mode+"\n"), 0o644)
	return dir
}

func TestOnlyEnforceRefusesAndOnlyUnapprovedFiles(t *testing.T) {
	f := filepath.Join(t.TempDir(), "policy.json")
	os.WriteFile(f, []byte(`{"deny":["bash"]}`), 0o644)
	other := filepath.Join(t.TempDir(), "mcp.json")
	os.WriteFile(other, []byte(`{}`), 0o644)
	for _, mode := range []string{"off", "watch"} {
		if why := New(Fixture(t, mode, f)).Refuse(other); why != "" {
			t.Fatalf("%s refuses nothing: %s", mode, why)
		}
	}
	c := New(Fixture(t, "enforce", f))
	if why := c.Refuse(f); why != "" {
		t.Fatal(why)
	}
	if why := c.Refuse(other); !strings.Contains(why, "not on the signed list") {
		t.Fatal(why)
	}
	os.WriteFile(f, []byte(`{}`), 0o644)
	if why := New(Fixture2(t, f)).Refuse(f); why != "" {
		t.Fatal("re-signed content is approved")
	}
	if why := c.Refuse(f); !strings.Contains(why, "changed since it was signed") {
		t.Fatal(why)
	}
	if why := c.Refuse("/usr/share/sarina/skills/x/SKILL.md"); why != "" {
		t.Fatal("package files are not checked")
	}
}

func Fixture2(t *testing.T, f string) string { return Fixture(t, "enforce", f) }

func TestABrokenSignatureRefusesEverything(t *testing.T) {
	f := filepath.Join(t.TempDir(), "policy.json")
	os.WriteFile(f, []byte(`{}`), 0o644)
	dir := Fixture(t, "enforce", f)
	os.WriteFile(filepath.Join(dir, "manifest.json"), []byte(`{"version":1,"files":{}}`), 0o644)
	if why := New(dir).Refuse(f); !strings.Contains(why, "signature") {
		t.Fatal(why)
	}
}
