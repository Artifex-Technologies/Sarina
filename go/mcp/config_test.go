package mcp

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type layout struct{ root, home, parent, ws string }

func newLayout(t *testing.T) layout {
	t.Helper()
	root := t.TempDir()
	l := layout{root: root, home: filepath.Join(root, "home"), parent: filepath.Join(root, "repos"), ws: filepath.Join(root, "repos", "proj")}
	for _, d := range []string{filepath.Join(l.home, ".sarina"), l.ws} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	return l
}

func put(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func (l layout) load(trust bool, env map[string]string) *Config {
	return Load(LoadOptions{Home: l.home, Workspace: l.ws, TrustWorkspace: trust,
		Getenv: func(k string) string { return env[k] }})
}

const oneServer = `{"mcpServers":{"tools":{"command":"srv","args":["-x"]}}}`

func TestWorkspaceDefinitionIsIgnoredWithoutTheTrustFlag(t *testing.T) {
	l := newLayout(t)
	for _, name := range WorkspaceFiles {
		put(t, filepath.Join(l.ws, name), oneServer)
	}
	c := l.load(false, nil)
	if len(c.Servers) != 0 || len(c.Files) != 0 {
		t.Fatalf("a workspace file was used without the flag: %+v", c.Servers)
	}
	if len(c.Notices) != len(WorkspaceFiles) {
		t.Fatalf("every ignored file should be reported: %v", c.Notices)
	}
	for _, n := range c.Notices {
		if !strings.Contains(n, "--trust-workspace-mcp") || !strings.Contains(n, "ignored") {
			t.Fatalf("notice = %q", n)
		}
	}
}

func TestWorkspaceDefinitionIsUsedWithTheFlagAndAnnounced(t *testing.T) {
	l := newLayout(t)
	put(t, filepath.Join(l.ws, ".mcp.json"), oneServer)
	c := l.load(true, nil)
	if len(c.Servers) != 1 || c.Servers[0].Source != SourceWorkspace || c.Servers[0].Command != "srv" {
		t.Fatalf("servers = %+v", c.Servers)
	}
	if len(c.Notices) != 1 || !strings.Contains(c.Notices[0], "may start programs") {
		t.Fatalf("notices = %v", c.Notices)
	}
}

func TestAncestorDefinitionsAreNeverReadEvenWithTheFlag(t *testing.T) {
	l := newLayout(t)
	for _, name := range WorkspaceFiles {
		put(t, filepath.Join(l.parent, name), oneServer) // the repository's parent directory
		put(t, filepath.Join(l.root, name), oneServer)   // and further up
	}
	put(t, filepath.Join(l.home, "mcp.json"), oneServer) // the home directory itself, not ~/.sarina
	for _, trust := range []bool{false, true} {
		c := l.load(trust, nil)
		if len(c.Servers) != 0 || len(c.Files) != 0 || len(c.Notices) != 0 {
			t.Fatalf("trust=%v: an ancestor definition leaked in: %+v %v", trust, c.Servers, c.Notices)
		}
	}
}

func TestOperatorFilesAreHomeAndEnvAndNeedNoFlag(t *testing.T) {
	l := newLayout(t)
	put(t, filepath.Join(l.home, ".sarina", "mcp.json"), `{"servers":[{"name":"a","command":"ca"}]}`)
	extra := filepath.Join(l.root, "extra.json")
	put(t, extra, `{"mcpServers":{"b":{"command":"cb"}}}`)
	c := l.load(false, map[string]string{EnvFile: extra})
	if len(c.Servers) != 2 || c.Servers[0].Name != "a" || c.Servers[1].Name != "b" {
		t.Fatalf("servers = %+v", c.Servers)
	}
	for _, s := range c.Servers {
		if s.Source != SourceOperator {
			t.Fatalf("source = %s", s.Source)
		}
	}
}

func TestOperatorDefinitionWinsOverAWorkspaceOneOfTheSameName(t *testing.T) {
	l := newLayout(t)
	put(t, filepath.Join(l.home, ".sarina", "mcp.json"), `{"mcpServers":{"Dup":{"command":"good"}}}`)
	put(t, filepath.Join(l.ws, "mcp.json"), `{"mcpServers":{"dup":{"command":"evil"}}}`)
	c := l.load(true, nil)
	if len(c.Servers) != 1 || c.Servers[0].Command != "good" {
		t.Fatalf("servers = %+v", c.Servers)
	}
	if len(c.Problems) != 1 || !strings.Contains(c.Problems[0], "already defined") {
		t.Fatalf("problems = %v", c.Problems)
	}
}

func TestServerFieldParsing(t *testing.T) {
	l := newLayout(t)
	put(t, filepath.Join(l.home, ".sarina", "mcp.json"), `{"mcpServers":{
		"full":{"command":"c","args":["a",1,"b"],"env":{"K":"v","N":3},"cwd":"sub","description":"d","read_only":true,
			"read_only_tools":["x"],"tools":{"y":{"read_only":true},"z":{"read_only":"yes"}},"timeout_seconds":7},
		"http":{"command":"c","transport":"sse"},
		"nocmd":{"args":[]},
		"stdio":{"command":"c","transport":"STDIO"}}}`)
	c := l.load(false, nil)
	if len(c.Servers) != 2 {
		t.Fatalf("servers = %+v problems = %v", c.Servers, c.Problems)
	}
	s := c.Servers[0]
	if s.Name != "full" || len(s.Args) != 2 || s.Env["K"] != "v" || len(s.Env) != 1 || !s.ReadOnly || s.TimeoutSeconds != 7 {
		t.Fatalf("%+v", s)
	}
	if want := filepath.Join(l.home, ".sarina", "sub"); s.Cwd != want {
		t.Fatalf("cwd = %q, want it resolved against the definition file: %q", s.Cwd, want)
	}
	if !s.ReadOnlyTools["x"] || !s.ReadOnlyTools["y"] || s.ReadOnlyTools["z"] {
		t.Fatalf("read-only tools = %v (only a literal true counts)", s.ReadOnlyTools)
	}
	if len(c.Problems) != 2 {
		t.Fatalf("problems = %v", c.Problems)
	}
}

func TestMalformedFilesAreReportedNotFatal(t *testing.T) {
	l := newLayout(t)
	put(t, filepath.Join(l.home, ".sarina", "mcp.json"), `{nope`)
	c := l.load(false, nil)
	if len(c.Servers) != 0 || len(c.Problems) != 1 {
		t.Fatalf("%+v", c)
	}
}
