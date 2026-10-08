package cli

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

// tierBackend is a minimal OpenAI-compatible server that answers the health
// endpoint separately from model calls, so counts mean "conversation sent".
type tierBackend struct {
	*httptest.Server
	mu    sync.Mutex
	chats int
	auth  []string
}

func newTierBackend(t *testing.T, reply string) *tierBackend {
	b := &tierBackend{}
	b.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodGet {
			io.WriteString(w, `{"data":[]}`)
			return
		}
		io.Copy(io.Discard, r.Body)
		b.mu.Lock()
		b.chats++
		b.auth = append(b.auth, r.Header.Get("Authorization"))
		b.mu.Unlock()
		j, _ := json.Marshal(map[string]any{"choices": []any{map[string]any{
			"message": map[string]any{"role": "assistant", "content": reply}, "finish_reason": "stop"}},
			"usage": map[string]any{"prompt_tokens": 5, "completion_tokens": 2}})
		w.Write(j)
	}))
	t.Cleanup(b.Close)
	return b
}

func (b *tierBackend) count() int { b.mu.Lock(); defer b.mu.Unlock(); return b.chats }

func writeTiers(t *testing.T, h *harness, doc string) {
	t.Helper()
	p := filepath.Join(h.env.Home, ".sarina")
	os.MkdirAll(p, 0o755)
	if err := os.WriteFile(filepath.Join(p, "tiers.json"), []byte(doc), 0o600); err != nil {
		t.Fatal(err)
	}
}

func twoTierDoc(desktopURL string) string {
	return fmt.Sprintf(`{"tiers":[{"name":"local"},{"name":"desktop","base_url":%q,"model":"big"}]}`, desktopURL)
}

func chatArgs(h *harness, localURL string, extra ...string) []string {
	return append([]string{"chat", "--base-url", localURL, "--cwd", h.dir, "--disable-claude-md", "--no-auto-memory-review", "--api-key", "cli-test-key-31337"}, extra...)
}

func TestTierUpSlashStartsNextTurnOnDesktop(t *testing.T) {
	local, desktop := newTierBackend(t, "from local"), newTierBackend(t, "from desktop")
	h := newHarness(t, strings.NewReader("/tier up\nhello\n/exit\n"))
	writeTiers(t, h, twoTierDoc(desktop.URL))
	code := Main(chatArgs(h, local.URL, "--interactive"), h.env)
	if code != 0 {
		t.Fatalf("exit %d: %s", code, h.errb.String())
	}
	if local.count() != 0 || desktop.count() != 1 {
		t.Fatalf("local=%d desktop=%d", local.count(), desktop.count())
	}
	if !strings.Contains(h.out.String(), "The next turn will start on tier desktop") || !strings.Contains(h.out.String(), "from desktop") {
		t.Fatalf("stdout: %s", h.out.String())
	}
	if !strings.Contains(h.errb.String(), "[tier] escalated local -> desktop: operator ran /tier up") {
		t.Fatalf("stderr: %s", h.errb.String())
	}
	// the local API key is never sent to another host
	if desktop.auth[0] != "Bearer" && desktop.auth[0] != "Bearer " && desktop.auth[0] != "" {
		t.Fatalf("local key leaked to the desktop tier: %q", desktop.auth[0])
	}
}

func TestLocalDownFallsToDesktopAndIsRecorded(t *testing.T) {
	desktop := newTierBackend(t, "from desktop")
	dead := httptest.NewServer(http.NotFoundHandler())
	deadURL := dead.URL
	dead.Close()
	h := newHarness(t, strings.NewReader(""))
	writeTiers(t, h, twoTierDoc(desktop.URL))
	code := Main(chatArgs(h, deadURL, "hi"), h.env)
	if code != 0 || !strings.Contains(h.out.String(), "from desktop") {
		t.Fatalf("exit %d out=%s err=%s", code, h.out.String(), h.errb.String())
	}
	if !strings.Contains(h.errb.String(), "[tier] escalated local -> desktop") {
		t.Fatalf("stderr: %s", h.errb.String())
	}
	files, _ := filepath.Glob(filepath.Join(h.dir, ".port_sessions", "agent", "*.json"))
	if len(files) != 1 {
		t.Fatalf("session files: %v", files)
	}
	raw, _ := os.ReadFile(files[0])
	var doc map[string]any
	json.Unmarshal(raw, &doc)
	rec := doc["plugin_state"].(map[string]any)["compute_tiers"].(map[string]any)
	if rec["current_tier"] != "desktop" {
		t.Fatalf("%v", rec)
	}
	if strings.Contains(string(raw), "cli-test-key-31337") {
		t.Fatal("api key in the session file")
	}
}

func TestNoTiersFileChangesNothing(t *testing.T) {
	local := newTierBackend(t, "from local")
	h := newHarness(t, strings.NewReader("/tier status\n/exit\n"))
	code := Main(chatArgs(h, local.URL, "--interactive"), h.env)
	if code != 0 || strings.Contains(h.errb.String(), "[tier]") {
		t.Fatalf("exit %d err=%s", code, h.errb.String())
	}
	if !strings.Contains(h.out.String(), "Compute tiers are not configured") || local.count() != 0 {
		t.Fatalf("out=%s", h.out.String())
	}
}

func TestBrokenTiersFileWarnsAndStaysLocal(t *testing.T) {
	local := newTierBackend(t, "from local")
	h := newHarness(t, strings.NewReader(""))
	writeTiers(t, h, `{"tiers":[{"name":"a"},{"name":"b"}]}`)
	code := Main(chatArgs(h, local.URL, "hi"), h.env)
	if code != 0 || local.count() != 1 || !strings.Contains(h.errb.String(), "staying on the local backend only") {
		t.Fatalf("exit %d count=%d err=%s", code, local.count(), h.errb.String())
	}
}

func TestTiersStatusCommand(t *testing.T) {
	local, desktop := newTierBackend(t, "x"), newTierBackend(t, "x")
	h := newHarness(t, strings.NewReader(""))
	secret := "sk-ant-api03-STATUS-SHOULD-NOT-PRINT-0123456789"
	base := h.env.Getenv
	h.env.Getenv = func(k string) string {
		switch k {
		case "OPENAI_BASE_URL":
			return local.URL
		case "ANTHROPIC_TEST_KEY":
			return secret
		}
		return base(k)
	}
	writeTiers(t, h, fmt.Sprintf(`{"tiers":[{"name":"local"},{"name":"desktop","base_url":%q,"model":"big","wake":"wakeonlan aa"},
		{"name":"claude","kind":"claude","model":"claude-test","api_key_env":"ANTHROPIC_TEST_KEY","enabled":false}]}`, desktop.URL))
	code := Main([]string{"tiers", "status"}, h.env)
	out := h.out.String()
	if code != 0 {
		t.Fatalf("exit %d: %s", code, h.errb.String())
	}
	for _, want := range []string{"1. local", "2. desktop", "healthy", "wake configured", "3. claude", "disabled", "$ANTHROPIC_TEST_KEY (set)", "Resources:"} {
		if !strings.Contains(out, want) {
			t.Errorf("status lacks %q:\n%s", want, out)
		}
	}
	if strings.Contains(out, secret) || strings.Contains(h.errb.String(), secret) {
		t.Fatal("status printed an API key")
	}
	if local.count() != 0 || desktop.count() != 0 {
		t.Fatal("status must send no conversation data")
	}
	if Main([]string{"tiers", "bogus"}, h.env) != exitUsage {
		t.Fatal("unknown subcommand")
	}
}

func TestTierSlashUnknownOption(t *testing.T) {
	desktop, local := newTierBackend(t, "x"), newTierBackend(t, "x")
	h := newHarness(t, strings.NewReader("/tier sideways\n/tier down\n/tier status\n/exit\n"))
	writeTiers(t, h, twoTierDoc(desktop.URL))
	Main(chatArgs(h, local.URL, "--interactive"), h.env)
	out := h.out.String()
	for _, want := range []string{`Unknown /tier option "sideways"`, "Back on tier local", "2. desktop"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
}
