package cli

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func reqTools(b *backend, i int) []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	var out []string
	list, _ := b.reqs[i]["tools"].([]any)
	for _, t := range list {
		fn, _ := t.(map[string]any)["function"].(map[string]any)
		out = append(out, fn["name"].(string))
	}
	return out
}

func writeAt(t *testing.T, path, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

// A server definition that can only fail to start: the point is whether the
// CLI TRIES, which it must do only under the trust gate.
const deadServer = `{"mcpServers":{"probe":{"command":"definitely-not-a-real-program-sarina"}}}`

func TestWorkspaceMCPFileIsIgnoredUnlessTrusted(t *testing.T) {
	for _, trust := range []bool{false, true} {
		b := newBackend(t, "ok")
		h := newHarness(t, strings.NewReader(""))
		writeAt(t, filepath.Join(h.dir, ".mcp.json"), deadServer)
		writeAt(t, filepath.Join(filepath.Dir(h.dir), ".mcp.json"), deadServer) // an ancestor too
		args := append([]string{"chat"}, h.common(b)...)
		if trust {
			args = append(args, "--trust-workspace-mcp")
		}
		if code := h.run(append(args, "hi")...); code != 0 {
			t.Fatalf("trust=%v code=%d err=%s", trust, code, h.errb.String())
		}
		errText := h.errb.String()
		attempted := strings.Contains(errText, `MCP server "probe" is unavailable`)
		if trust {
			if !attempted || !strings.Contains(errText, "may start programs") {
				t.Fatalf("with the flag the server should be tried and announced:\n%s", errText)
			}
		} else if attempted || !strings.Contains(errText, "--trust-workspace-mcp") {
			t.Fatalf("without the flag nothing may start and the operator must be told:\n%s", errText)
		}
		if n := strings.Count(errText, "is unavailable"); (trust && n != 1) || (!trust && n != 0) {
			t.Fatalf("trust=%v: %d start attempts (an ancestor's file must never add one):\n%s", trust, n, errText)
		}
	}
}

func TestOperatorMCPFileInHomeNeedsNoFlag(t *testing.T) {
	b := newBackend(t, "ok")
	h := newHarness(t, strings.NewReader(""))
	writeAt(t, filepath.Join(h.env.Home, ".sarina", "mcp.json"), deadServer)
	h.run(append(append([]string{"chat"}, h.common(b)...), "hi")...)
	if !strings.Contains(h.errb.String(), `MCP server "probe" is unavailable`) {
		t.Fatalf("the operator's own definition should be tried:\n%s", h.errb.String())
	}
}

func TestPolicyDenyListRemovesToolsAndAgentToolIsOfferedByDefault(t *testing.T) {
	b := newBackend(t, "ok")
	h := newHarness(t, strings.NewReader(""))
	writeAt(t, filepath.Join(h.env.Home, ".sarina", "policy.json"), `{"denyTools":["bash"],"denyToolPrefixes":["task_"]}`)
	if code := h.run(append(append([]string{"chat"}, h.common(b)...), "hi")...); code != 0 {
		t.Fatalf("code=%d %s", code, h.errb.String())
	}
	tools := reqTools(b, 0)
	if slices.Contains(tools, "bash") || slices.ContainsFunc(tools, func(n string) bool { return strings.HasPrefix(n, "task_") }) {
		t.Fatalf("denied tools were offered: %v", tools)
	}
	if !slices.Contains(tools, "read_file") || !slices.Contains(tools, "agent") {
		t.Fatalf("tools = %v", tools)
	}
}

func TestWorkspacePolicyTightensButCannotLoosen(t *testing.T) {
	b := newBackend(t, "ok")
	h := newHarness(t, strings.NewReader(""))
	writeAt(t, filepath.Join(h.env.Home, ".sarina", "policy.json"), `{"denyTools":["bash"]}`)
	writeAt(t, filepath.Join(h.dir, ".sarina-policy.json"), `{"denyTools":["write_file"],"trusted":true,"safeEnv":["PATH"]}`)
	h.run(append(append([]string{"chat"}, h.common(b)...), "hi")...)
	tools := reqTools(b, 0)
	if slices.Contains(tools, "bash") || slices.Contains(tools, "write_file") {
		t.Fatalf("tools = %v", tools)
	}
	if !strings.Contains(h.errb.String(), "tightens") || !strings.Contains(h.errb.String(), "safeEnv ignored") {
		t.Fatalf("the operator should see what the workspace file did:\n%s", h.errb.String())
	}
}

func TestPolicyBudgetOverridesFlagsAndOperatorMustFixABrokenFile(t *testing.T) {
	b := newBackend(t, "ok")
	h := newHarness(t, strings.NewReader(""))
	writeAt(t, filepath.Join(h.env.Home, ".sarina", "policy.json"), `{not json`)
	if code := h.run(append(append([]string{"chat"}, h.common(b)...), "hi")...); code != exitUsage {
		t.Fatalf("a broken operator policy must stop the run, code=%d", code)
	}
	if b.count() != 0 || !strings.Contains(h.errb.String(), "unusable") {
		t.Fatalf("requests=%d err=%s", b.count(), h.errb.String())
	}
	// a valid one: untrusted turns MCP off with a notice
	writeAt(t, filepath.Join(h.env.Home, ".sarina", "policy.json"), `{"trusted":false}`)
	writeAt(t, filepath.Join(h.env.Home, ".sarina", "mcp.json"), deadServer)
	h.errb.Reset()
	h.run(append(append([]string{"chat"}, h.common(b)...), "hi")...)
	if !strings.Contains(h.errb.String(), "trusted=false") || strings.Contains(h.errb.String(), "unavailable") {
		t.Fatalf("err = %s", h.errb.String())
	}
}

func TestNoToolsStillMeansNoToolsWithTheAgentToolInstalled(t *testing.T) {
	b := newBackend(t, "ok")
	h := newHarness(t, strings.NewReader(""))
	h.run(append(append([]string{"chat", "--no-tools"}, h.common(b)...), "hi")...)
	if _, has := b.reqs[0]["tools"]; has {
		t.Fatalf("an empty allowlist must omit tools: %v", b.reqs[0]["tools"])
	}
}

func TestMCPAndAgentsListCommands(t *testing.T) {
	h := newHarness(t, strings.NewReader(""))
	writeAt(t, filepath.Join(h.dir, "mcp.json"), deadServer)
	writeAt(t, filepath.Join(h.env.Home, ".sarina", "agents", "scout.md"),
		"---\ndescription: reads logs\ntools: read_file\n---\nLook at the logs.\n")
	if code := h.run("mcp", "list", "--cwd", h.dir); code != 0 {
		t.Fatalf("code=%d", code)
	}
	if !strings.Contains(h.out.String(), "NOTICE: ignored MCP definition file") || !strings.Contains(h.out.String(), "No MCP servers configured") {
		t.Fatalf("out = %s", h.out.String())
	}
	h.out.Reset()
	h.run("mcp", "list", "--cwd", h.dir, "--trust-workspace-mcp")
	if !strings.Contains(h.out.String(), "probe") || !strings.Contains(h.out.String(), "source: workspace") {
		t.Fatalf("out = %s", h.out.String())
	}
	h.out.Reset()
	h.run("agents", "list", "--cwd", h.dir)
	for _, want := range []string{"explorer", "planner", "verifier", "scout", "source: operator"} {
		if !strings.Contains(h.out.String(), want) {
			t.Errorf("agents list lacks %q:\n%s", want, h.out.String())
		}
	}
}
