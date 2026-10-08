package agent

import (
	"fmt"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/memory"
)

func seedMemory(t *testing.T, a *Agent, n int) {
	t.Helper()
	rt := memory.FromWorkspace(a.opts.Runtime.CWD, a.opts.Home)
	for i := 0; i < n; i++ {
		if _, err := rt.Add("agent", fmt.Sprintf("filler note %d about nothing at all", i)); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := rt.Add("agent", "the deploy pipeline needs the vault token rotated monthly"); err != nil {
		t.Fatal(err)
	}
}

func contextMessage(res *Result) string {
	for _, m := range res.Transcript {
		if m.MessageID == "user_context_0" {
			return m.Content
		}
	}
	return ""
}

// The ranking must use the REAL first prompt: the original never applied it.
func TestCuratedMemoryIsRankedAgainstTheActualFirstPrompt(t *testing.T) {
	f := newFakeServer(t, reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f, withDiscovery)
	seedMemory(t, a, 10)
	res := mustRun(t, a, "please rotate the vault token for the deploy pipeline")
	ctx := contextMessage(res)
	if !strings.Contains(ctx, "# curatedMemory") || !strings.Contains(ctx, "vault token rotated monthly") {
		t.Fatalf("relevant entry missing from context:\n%s", ctx)
	}
	if !strings.Contains(ctx, "less-relevant entries omitted") || strings.Contains(ctx, "filler note 9") {
		t.Fatalf("expected ranking to drop irrelevant entries:\n%s", ctx)
	}
}

func TestCuratedMemoryFullBlockWhenSmallAndAbsentWhenEmpty(t *testing.T) {
	f := newFakeServer(t, reply("ok", "stop", use(1, 1)), reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f, withDiscovery)
	if strings.Contains(contextMessage(mustRun(t, a, "hello")), "curatedMemory") {
		t.Fatal("no memory, no key")
	}
	seedMemory(t, a, 2)
	ctx := contextMessage(mustRun(t, a, "hello"))
	if !strings.Contains(ctx, "filler note 0") || strings.Contains(ctx, "omitted") {
		t.Fatalf("small memory must be injected whole:\n%s", ctx)
	}
}

func TestCuratedMemoryNotInjectedWhenGuidanceDiscoveryIsOff(t *testing.T) {
	f := newFakeServer(t, reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.DisableClaudeMdDiscovery = true })
	seedMemory(t, a, 2)
	if strings.Contains(contextMessage(mustRun(t, a, "hello")), "curatedMemory") {
		t.Fatal("never produced when CLAUDE.md discovery is disabled")
	}
}

func withDiscovery(o *Options) { o.Runtime.DisableClaudeMdDiscovery = false }
