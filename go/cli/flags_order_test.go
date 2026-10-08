package cli

import (
	"encoding/json"
	"strings"
	"testing"
)

// Flags after the prompt used to be swallowed into the prompt text:
// `sarina chat "do it" --allow-write` ran without write access and sent
// "do it --allow-write" to the model.
func TestFlagsAfterThePromptAreFlags(t *testing.T) {
	b := newBackend(t, "plain")
	h := newHarness(t, strings.NewReader(""))
	args := append([]string{"chat", "list the files"}, append(h.common(b), "--no-tools")...)
	if code := h.run(args...); code != 0 {
		t.Fatalf("exit %d: %s", code, h.errb.String())
	}
	req := b.reqs[0]
	if _, ok := req["tools"]; ok {
		t.Fatal("--no-tools after the prompt was ignored")
	}
	raw, _ := json.Marshal(req["messages"])
	if strings.Contains(string(raw), "--no-tools") || strings.Contains(string(raw), "--base-url") {
		t.Fatalf("a flag leaked into the prompt: %s", raw)
	}
	if !strings.Contains(string(raw), "list the files") {
		t.Fatalf("the prompt did not reach the model: %s", raw)
	}
}

func TestUnknownFlagAfterThePromptIsAnError(t *testing.T) {
	b := newBackend(t, "plain")
	h := newHarness(t, strings.NewReader(""))
	args := append([]string{"chat", "hi"}, append(h.common(b), "--no-such-flag")...)
	if code := h.run(args...); code == 0 {
		t.Fatal("an unknown flag after the prompt must fail, not become prompt text")
	}
	if b.count() != 0 {
		t.Fatalf("nothing should have been sent to the model, got %d request(s)", b.count())
	}
}

func TestDoubleDashKeepsFlagLikeTextInThePrompt(t *testing.T) {
	b := newBackend(t, "plain")
	h := newHarness(t, strings.NewReader(""))
	args := append(append([]string{"chat"}, h.common(b)...), "--", "explain", "--allow-write", "to me")
	if code := h.run(args...); code != 0 {
		t.Fatalf("exit %d: %s", code, h.errb.String())
	}
	raw, _ := json.Marshal(b.reqs[0]["messages"])
	if !strings.Contains(string(raw), "explain --allow-write to me") {
		t.Fatalf("text after -- must stay in the prompt: %s", raw)
	}
}
