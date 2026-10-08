package tools

import (
	"context"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// ---- helpers ------------------------------------------------------------------

type env struct {
	t    *testing.T
	root string
	reg  *Registry
	tc   *Context
}

func newEnv(t *testing.T, perms config.Permissions) *env {
	t.Helper()
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	return &env{t: t, root: root, reg: NewRegistry(DefaultTools()...),
		tc: &Context{Root: root, MaxOutputChars: 12000, Permissions: perms}}
}

func writeAll() config.Permissions { return config.Permissions{AllowFileWrite: true} }
func shellAll() config.Permissions {
	return config.Permissions{AllowFileWrite: true, AllowShellCommands: true}
}

func (e *env) run(name string, args map[string]any) Result {
	e.t.Helper()
	return e.reg.Execute(context.Background(), name, args, e.tc)
}

func (e *env) ok(name string, args map[string]any) Result {
	e.t.Helper()
	r := e.run(name, args)
	if !r.OK {
		e.t.Fatalf("%s %v failed: %s (%v)", name, args, r.Content, r.Metadata)
	}
	return r
}

func (e *env) fails(name string, args map[string]any, kind string) Result {
	e.t.Helper()
	r := e.run(name, args)
	if r.OK {
		e.t.Fatalf("%s %v should have failed, got %q", name, args, r.Content)
	}
	if kind != "" && r.Metadata["error_kind"] != kind {
		e.t.Fatalf("%s %v error_kind = %v, want %s (%s)", name, args, r.Metadata["error_kind"], kind, r.Content)
	}
	return r
}

func (e *env) put(rel, content string) string {
	e.t.Helper()
	p := filepath.Join(e.root, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		e.t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		e.t.Fatal(err)
	}
	return p
}

func (e *env) read(rel string) string {
	e.t.Helper()
	b, err := os.ReadFile(filepath.Join(e.root, filepath.FromSlash(rel)))
	if err != nil {
		e.t.Fatal(err)
	}
	return string(b)
}

func (e *env) exists(rel string) bool {
	_, err := os.Lstat(filepath.Join(e.root, filepath.FromSlash(rel)))
	return err == nil
}

// ---- permissions -----------------------------------------------------------------

func TestEveryToolDeclaresItsPermission(t *testing.T) {
	want := map[string]Permission{
		"write_file": PermWrite, "edit_file": PermWrite,
		"task_create": PermWrite, "task_update": PermWrite,
		"todo_write": PermWrite, "update_plan": PermWrite,
		"bash": PermShell, "TaskStop": PermShell,
		// read-only
		"read_file": PermNone, "list_dir": PermNone, "glob_search": PermNone, "grep_search": PermNone, "TaskOutput": PermNone,
		"task_list": PermNone, "plan_get": PermNone,
		// plan-mode switches (they request a state change; the agent owns the state)
		"enter_plan_mode": PermNone, "exit_plan_mode": PermNone,
	}
	for _, tool := range DefaultTools() {
		got, known := want[tool.Spec.Name]
		if !known {
			t.Errorf("tool %s has no entry in this test: decide its permission and add it", tool.Spec.Name)
			continue
		}
		if tool.Requires != got {
			t.Errorf("%s requires %v, want %v", tool.Spec.Name, tool.Requires, got)
		}
	}
	if len(DefaultTools()) != len(want) {
		t.Errorf("tool count %d != %d", len(DefaultTools()), len(want))
	}
}

func TestGatedToolsAreDeniedWithoutFlags(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	e.put("a.txt", "x")
	calls := map[string]map[string]any{
		"write_file":    {"path": "n.txt", "content": "x"},
		"edit_file":     {"path": "a.txt", "old_text": "x", "new_text": "y"},
		"task_create":   {"title": "t"},
		"task_update":   {"task_id": "t1", "title": "t"},
		"todo_write":    {"items": []any{map[string]any{"title": "t"}}},
		"update_plan":   {"items": []any{map[string]any{"step": "s"}}},
		"bash":          {"command": "echo hi"},
		"TaskStop":      {"task_id": "bg_x"},
	}
	for name, args := range calls {
		r := e.fails(name, args, "permission_denied")
		flag := "--allow-write"
		if name == "bash" || name == "TaskStop" {
			flag = "--allow-shell"
		}
		if !strings.Contains(r.Content, flag) {
			t.Errorf("%s: the refusal must name %s: %q", name, flag, r.Content)
		}
	}
	if e.exists("n.txt") || e.read("a.txt") != "x" || e.exists(".port_sessions") {
		t.Fatal("a denied call must change nothing")
	}
	// write permission does not grant shell, and shell does not grant write
	e.tc.Permissions = config.Permissions{AllowFileWrite: true}
	e.fails("bash", map[string]any{"command": "echo hi"}, "permission_denied")
	e.tc.Permissions = config.Permissions{AllowShellCommands: true}
	e.fails("write_file", map[string]any{"path": "n.txt", "content": "x"}, "permission_denied")
	// the destructive flag alone is not shell permission
	e.tc.Permissions = config.Permissions{AllowDestructiveShellCommands: true}
	e.fails("bash", map[string]any{"command": "echo hi"}, "permission_denied")
}

func TestReadersWorkWithoutFlags(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	e.put("a.txt", "hello")
	for name, args := range map[string]map[string]any{
		"read_file": {"path": "a.txt"}, "list_dir": {}, "glob_search": {"pattern": "*.txt"}, "grep_search": {"pattern": "hel"},
		"task_list": {}, "plan_get": {},
	} {
		e.ok(name, args)
	}
}

// ---- schema -------------------------------------------------------------------------

func TestSchemaBoundsAreEnforced(t *testing.T) {
	e := newEnv(t, shellAll())
	e.put("a.txt", "x\ny\n")
	bad := []struct {
		tool string
		args map[string]any
	}{
		{"read_file", map[string]any{"path": "a.txt", "start_line": 0}},
		{"read_file", map[string]any{"path": "a.txt", "start_line": -3}},
		{"read_file", map[string]any{"path": "a.txt", "end_line": 0}},
		{"read_file", map[string]any{"path": "a.txt", "start_line": true}},
		{"read_file", map[string]any{"path": "a.txt", "start_line": 1.5}},
		{"read_file", map[string]any{"path": "a.txt", "start_line": "abc"}},
		{"read_file", map[string]any{"path": ""}},
		{"read_file", map[string]any{}},
		{"read_file", map[string]any{"path": 7}},
		{"grep_search", map[string]any{"pattern": "x", "max_matches": 501}},
		{"grep_search", map[string]any{"pattern": "x", "max_matches": 0}},
		{"grep_search", map[string]any{"pattern": "x", "literal": "maybe"}},
		{"list_dir", map[string]any{"max_entries": 0}},
		{"list_dir", map[string]any{"max_entries": 100000}},
		{"bash", map[string]any{"command": "echo", "timeout_seconds": 0}},
		{"bash", map[string]any{"command": "echo", "timeout_seconds": 601}},
		{"bash", map[string]any{"command": ""}},
		{"TaskOutput", map[string]any{"task_id": "x", "timeout": -1}},
		{"TaskOutput", map[string]any{"task_id": "x", "timeout": 600001}},
		{"edit_file", map[string]any{"path": "a.txt", "old_text": "", "new_text": "y"}},
		{"write_file", map[string]any{"path": "a.txt"}},
		{"todo_write", map[string]any{"items": "nope"}},
		{"todo_write", map[string]any{"items": []any{map[string]any{"description": "no title"}}}},
		{"task_list", map[string]any{"max_tasks": 501}},
	}
	for _, c := range bad {
		r := e.run(c.tool, c.args)
		if r.OK || r.Metadata["error_kind"] != "tool_execution_error" {
			t.Errorf("%s %v should be refused as invalid: %+v", c.tool, c.args, r)
		}
	}
	// the boundaries themselves are fine, and loosely-typed numbers from local models are accepted
	for _, c := range []struct {
		tool string
		args map[string]any
	}{
		{"read_file", map[string]any{"path": "a.txt", "start_line": 1}},
		{"read_file", map[string]any{"path": "a.txt", "start_line": "2", "end_line": 2.0}},
		{"grep_search", map[string]any{"pattern": "x", "max_matches": 500}},
		{"grep_search", map[string]any{"pattern": "x", "max_matches": 1, "literal": "true"}},
		{"list_dir", map[string]any{"max_entries": 1}},
	} {
		e.ok(c.tool, c.args)
	}
}

func TestValidateArgsCoercionAndNesting(t *testing.T) {
	schema := obj(map[string]any{
		"n": prop("integer", "", "minimum", 1, "maximum", 10),
		"f": prop("number", ""),
		"b": prop("boolean", ""),
		"e": prop("string", "", "enum", []string{"a", "b"}),
		"l": map[string]any{"type": "array", "items": obj(map[string]any{"k": prop("string", "", "minLength", 1)}, "k")},
	}, "n")
	got, err := ValidateArgs(schema, map[string]any{"n": "3", "f": "1.5", "b": "false", "e": "a", "extra": 1})
	if err != nil {
		t.Fatal(err)
	}
	if got["n"] != 3.0 || got["f"] != 1.5 || got["b"] != false || got["extra"] != 1 {
		t.Fatalf("coerced = %v", got)
	}
	for _, a := range []map[string]any{
		{}, {"n": nil}, {"n": 11}, {"n": 0}, {"n": true}, {"n": 2, "e": "c"}, {"n": 2, "b": 1}, {"n": 2, "f": false},
		{"n": 2, "l": []any{map[string]any{}}}, {"n": 2, "l": []any{map[string]any{"k": ""}}}, {"n": 2, "l": []any{"x"}}, {"n": 2, "l": map[string]any{}},
	} {
		if _, err := ValidateArgs(schema, a); err == nil {
			t.Errorf("%v should be invalid", a)
		}
	}
	if _, err := ValidateArgs(nil, map[string]any{"x": 1}); err != nil {
		t.Fatal("no schema means no checks")
	}
}

// ---- secret guard --------------------------------------------------------------------

func TestIsSecretPath(t *testing.T) {
	for _, p := range []string{
		".env", ".env.local", ".env.example", ".env.production", "a/b/.env", "./.env", "/abs/.env", `c:\proj\.env`, `sub\.env`,
		".ENV", ".Env.Local", ".env ", ".env.", ".env::$DATA", ".env:stream", ".envrc",
		"server.pem", "tls.KEY", "a/b/cert.pem", "x.p12", "x.pfx", "putty.ppk",
		"id_rsa", "id_rsa.pub", "id_ed25519", "id_ecdsa", "id_dsa.pub", ".ssh/id_rsa",
		"credentials", "credentials.json", "credentials.yaml", "Credentials.YML", ".aws/credentials",
		"secret", "secrets", "secrets.json", "secret.yaml", "secrets.toml", "SECRETS.TOML",
		".netrc", "_netrc", ".git-credentials", ".pgpass",
	} {
		if !IsSecretPath(p) {
			t.Errorf("%q must be secret-bearing", p)
		}
	}
	for _, p := range []string{
		"env", "environment.go", "env.go", "main.go", "README.md", "dotenv/loader.go", "x.env.go", "key.go", "keys.txt", "monkey",
		"credentials.go", "credentials.toml", "secretary.txt", "secrets.txt", "id_rsa_notes.txt", "pem", "a.pem.txt", ".environment", "envrc",
		".env/readme.txt", // a directory named .env is not a secret file; its entries are judged by their own names
	} {
		if IsSecretPath(p) {
			t.Errorf("%q must not be flagged", p)
		}
	}
}

func TestSecretGuardOnEveryFileTool(t *testing.T) {
	e := newEnv(t, writeAll())
	e.put(".env", "API_KEY=TOPSECRET\n")
	e.put("sub/.env.local", "X=TOPSECRET\n")
	e.put("k.pem", "TOPSECRET")
	e.put("ok.txt", "visible TOPSECRET-FREE")

	for _, p := range []string{".env", "./.env", "sub/../.env", filepath.Join(e.root, ".env"), "sub/.env.local", "k.pem", ".ENV"} {
		for tool, args := range map[string]map[string]any{
			"read_file":   {"path": p},
			"edit_file":   {"path": p, "old_text": "TOPSECRET", "new_text": "x"},
			"write_file":  {"path": p, "content": "x"},
			"grep_search": {"pattern": "TOPSECRET", "path": p},
		} {
			r := e.run(tool, args)
			if r.OK {
				t.Errorf("%s %q must be refused", tool, p)
			}
			if strings.Contains(r.Content, "TOPSECRET") {
				t.Errorf("%s %q leaked the secret: %q", tool, p, r.Content)
			}
		}
	}
	if e.read(".env") != "API_KEY=TOPSECRET\n" || e.read("k.pem") != "TOPSECRET" {
		t.Fatal("a refused write or edit changed the secret file")
	}
	// directory grep skips secret files silently and still finds the rest
	r := e.ok("grep_search", map[string]any{"pattern": "TOPSECRET"})
	if strings.Contains(r.Content, ".env") || strings.Contains(r.Content, "k.pem") {
		t.Fatalf("directory grep must skip secret files: %q", r.Content)
	}
	if !strings.Contains(r.Content, "ok.txt:1:") {
		t.Fatalf("grep missed the ordinary file: %q", r.Content)
	}
}

func TestWriteFileCannotCreateSecretFiles(t *testing.T) {
	e := newEnv(t, writeAll())
	for _, p := range []string{".env", ".env.local", "config/.env.production", "server.key", "id_rsa", ".aws/credentials", "credentials.json", ".netrc", "secrets.toml"} {
		e.fails("write_file", map[string]any{"path": p, "content": "x"}, "tool_execution_error")
		if e.exists(p) {
			t.Errorf("%s was created", p)
		}
	}
	if e.exists(".aws") || e.exists("config") {
		t.Error("a refused write must not leave directories behind")
	}
	e.ok("write_file", map[string]any{"path": "config/app.toml", "content": "x"})
}

func TestSecretGuardSymlinksAndHardLinks(t *testing.T) {
	e := newEnv(t, writeAll())
	secret := e.put(".env", "TOPSECRET")
	if err := os.Symlink(secret, filepath.Join(e.root, "innocent.txt")); err == nil {
		e.fails("read_file", map[string]any{"path": "innocent.txt"}, "tool_execution_error")
		e.fails("edit_file", map[string]any{"path": "innocent.txt", "old_text": "TOPSECRET", "new_text": "x"}, "")
		e.fails("write_file", map[string]any{"path": "innocent.txt", "content": "x"}, "")
		e.fails("grep_search", map[string]any{"pattern": "TOP", "path": "innocent.txt"}, "")
		r := e.ok("grep_search", map[string]any{"pattern": "TOPSECRET"})
		if strings.Contains(r.Content, "innocent") {
			t.Errorf("a symlink to a secret must not be searched: %q", r.Content)
		}
	} else {
		t.Logf("symlinks unavailable: %v", err)
	}
	// a link NAMED like a secret is refused even when it points somewhere harmless
	harmless := e.put("plain.txt", "fine")
	if err := os.Symlink(harmless, filepath.Join(e.root, ".env.link")); err == nil {
		e.fails("read_file", map[string]any{"path": ".env.link"}, "")
	}
	// a hard link to a secret file in the same directory
	if err := os.Link(secret, filepath.Join(e.root, "notes.txt")); err == nil {
		e.fails("read_file", map[string]any{"path": "notes.txt"}, "")
		e.fails("edit_file", map[string]any{"path": "notes.txt", "old_text": "TOPSECRET", "new_text": "x"}, "")
		e.fails("write_file", map[string]any{"path": "notes.txt", "content": "x"}, "")
		r := e.ok("grep_search", map[string]any{"pattern": "TOPSECRET"})
		if strings.Contains(r.Content, "notes.txt") {
			t.Errorf("a hard link to a secret must not be searched: %q", r.Content)
		}
	} else {
		t.Logf("hard links unavailable: %v", err)
	}
	if e.read(".env") != "TOPSECRET" {
		t.Fatal("the secret file changed")
	}
}

func TestListAndGlobShowNamesOfSecretFiles(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	e.put(".env", "TOPSECRET")
	if r := e.ok("list_dir", nil); !strings.Contains(r.Content, ".env") || strings.Contains(r.Content, "TOPSECRET") {
		t.Fatalf("list_dir: %q", r.Content)
	}
	if r := e.ok("glob_search", map[string]any{"pattern": ".env*"}); !strings.Contains(r.Content, ".env") || strings.Contains(r.Content, "TOPSECRET") {
		t.Fatalf("glob: %q", r.Content)
	}
}

// ---- confinement ----------------------------------------------------------------------

func TestPathsCannotEscapeTheWorkspace(t *testing.T) {
	e := newEnv(t, writeAll())
	outsideDir := t.TempDir()
	outside := filepath.Join(outsideDir, "o.txt")
	os.WriteFile(outside, []byte("OUTSIDE"), 0o644)
	rel := "../" + filepath.Base(outsideDir) + "/o.txt"
	for _, p := range []string{rel, outside, "..", "../x"} {
		e.fails("read_file", map[string]any{"path": p}, "tool_execution_error")
		e.fails("write_file", map[string]any{"path": filepath.Join(p, "..", "w.new"), "content": "x"}, "tool_execution_error")
		e.fails("edit_file", map[string]any{"path": p, "old_text": "OUTSIDE", "new_text": "x"}, "tool_execution_error")
		e.fails("list_dir", map[string]any{"path": p}, "tool_execution_error")
	}
	if b, _ := os.ReadFile(outside); string(b) != "OUTSIDE" {
		t.Fatal("outside file changed")
	}
	if _, err := os.Stat(filepath.Join(outsideDir, "w.new")); err == nil {
		t.Fatal("wrote outside the workspace")
	}
	// writing through a symlinked directory that leaves the workspace
	if err := os.Symlink(outsideDir, filepath.Join(e.root, "out")); err == nil {
		e.fails("write_file", map[string]any{"path": "out/new.txt", "content": "x"}, "tool_execution_error")
		e.fails("write_file", map[string]any{"path": "out/o.txt", "content": "x"}, "tool_execution_error")
		e.fails("read_file", map[string]any{"path": "out/o.txt"}, "tool_execution_error")
		if _, err := os.Stat(filepath.Join(outsideDir, "new.txt")); err == nil {
			t.Fatal("wrote through a symlink out of the workspace")
		}
	}
	// a dangling symlink is not followed for a write
	if err := os.Symlink(filepath.Join(outsideDir, "ghost.txt"), filepath.Join(e.root, "dangling")); err == nil {
		e.fails("write_file", map[string]any{"path": "dangling", "content": "x"}, "tool_execution_error")
		if _, err := os.Stat(filepath.Join(outsideDir, "ghost.txt")); err == nil {
			t.Fatal("wrote through a dangling symlink")
		}
	}
}

// ---- files ----------------------------------------------------------------------------

func TestWriteFile(t *testing.T) {
	e := newEnv(t, writeAll())
	r := e.ok("write_file", map[string]any{"path": "a/b/c.txt", "content": "héllo"})
	if e.read("a/b/c.txt") != "héllo" || !strings.Contains(r.Content, "a/b/c.txt") || !strings.Contains(r.Content, "5 characters") {
		t.Fatalf("write: %+v", r)
	}
	md := r.Metadata
	if md["action"] != "write_file" || md["path"] != "a/b/c.txt" || md["before_exists"] != false || md["before_sha256"] != nil ||
		md["after_sha256"] != sha256Hex("héllo") || md["content_length"] != 5 {
		t.Fatalf("metadata: %v", md)
	}
	r = e.ok("write_file", map[string]any{"path": "a/b/c.txt", "content": ""})
	if e.read("a/b/c.txt") != "" || r.Metadata["before_exists"] != true || r.Metadata["before_sha256"] != sha256Hex("héllo") {
		t.Fatalf("overwrite with empty content: %v", r.Metadata)
	}
	e.fails("write_file", map[string]any{"path": "a", "content": "x"}, "tool_execution_error") // a is a directory
}

func TestEditFile(t *testing.T) {
	e := newEnv(t, writeAll())
	e.put("f.txt", "one two two three")
	r := e.fails("edit_file", map[string]any{"path": "f.txt", "old_text": "two", "new_text": "2"}, "tool_execution_error")
	if !strings.Contains(r.Content, "2 places") {
		t.Fatalf("ambiguity must name the count: %q", r.Content)
	}
	e.fails("edit_file", map[string]any{"path": "f.txt", "old_text": "absent", "new_text": "x"}, "tool_execution_error")
	e.fails("edit_file", map[string]any{"path": "f.txt", "old_text": "one", "new_text": "one"}, "tool_execution_error")
	e.fails("edit_file", map[string]any{"path": "missing.txt", "old_text": "a", "new_text": "b"}, "tool_execution_error")
	if e.read("f.txt") != "one two two three" {
		t.Fatal("failed edits changed the file")
	}
	r = e.ok("edit_file", map[string]any{"path": "f.txt", "old_text": "one", "new_text": "1"})
	if e.read("f.txt") != "1 two two three" || r.Metadata["replaced_occurrences"] != 1 || r.Metadata["action"] != "edit_file" {
		t.Fatalf("single edit: %v %q", r.Metadata, e.read("f.txt"))
	}
	r = e.ok("edit_file", map[string]any{"path": "f.txt", "old_text": "two", "new_text": "2", "replace_all": true})
	if e.read("f.txt") != "1 2 2 three" || r.Metadata["replaced_occurrences"] != 2 {
		t.Fatalf("replace_all: %v %q", r.Metadata, e.read("f.txt"))
	}
	for _, k := range []string{"before_sha256", "after_sha256", "before_size", "after_size", "before_preview", "after_preview", "old_text_preview", "new_text_preview", "path"} {
		if _, ok := r.Metadata[k]; !ok {
			t.Errorf("edit metadata lacks %s", k)
		}
	}
}

func TestReadFileBehaviour(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	e.put("f.txt", "a\r\nb\nc\n")
	if r := e.ok("read_file", map[string]any{"path": "f.txt"}); r.Content != "a\r\nb\nc\n" {
		t.Fatalf("whole file is returned as is: %q", r.Content)
	}
	if r := e.ok("read_file", map[string]any{"path": "f.txt", "start_line": 2}); r.Content != "2: b\n3: c" {
		t.Fatalf("open-ended range: %q", r.Content)
	}
	if r := e.ok("read_file", map[string]any{"path": "f.txt", "end_line": 1}); r.Content != "1: a" {
		t.Fatalf("end only: %q", r.Content)
	}
	if r := e.ok("read_file", map[string]any{"path": "f.txt", "start_line": 3, "end_line": 2}); !strings.Contains(r.Content, "no lines") {
		t.Fatalf("end before start: %q", r.Content)
	}
	e.put("bad.txt", "ok\xff\xfeend")
	if r := e.ok("read_file", map[string]any{"path": "bad.txt"}); !strings.Contains(r.Content, "ok") || !strings.Contains(r.Content, "end") {
		t.Fatalf("invalid UTF-8 must be replaced: %q", r.Content)
	}
	e.tc.MaxOutputChars = 100
	e.put("big.txt", strings.Repeat("x", 5000))
	if r := e.ok("read_file", map[string]any{"path": "big.txt"}); len(r.Content) > 200 || !strings.Contains(r.Content, "omitted") {
		t.Fatalf("output must be truncated: %d", len(r.Content))
	}
	e.fails("read_file", map[string]any{"path": "."}, "tool_execution_error")
	e.fails("read_file", map[string]any{"path": "nope"}, "tool_execution_error")
}

func TestListDir(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	e.put("b.txt", "")
	e.put("A.txt", "")
	e.put("zdir/inner.txt", "")
	e.put("adir/inner.txt", "")
	r := e.ok("list_dir", nil)
	if r.Content != "dir\tadir\ndir\tzdir\nfile\tA.txt\nfile\tb.txt" {
		t.Fatalf("order: %q", r.Content)
	}
	if r = e.ok("list_dir", map[string]any{"path": "zdir"}); r.Content != "file\tzdir/inner.txt" {
		t.Fatalf("subdir: %q", r.Content)
	}
	if r = e.ok("list_dir", map[string]any{"max_entries": 1}); !strings.Contains(r.Content, "truncated at 1") {
		t.Fatalf("truncation note: %q", r.Content)
	}
	os.MkdirAll(filepath.Join(e.root, "empty"), 0o755)
	if r = e.ok("list_dir", map[string]any{"path": "empty"}); !strings.Contains(r.Content, "empty") {
		t.Fatalf("empty: %q", r.Content)
	}
	e.fails("list_dir", map[string]any{"path": "b.txt"}, "tool_execution_error")
}

func TestGlob(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	for _, p := range []string{"main.go", "pkg/a.go", "pkg/deep/b.go", "pkg/deep/c.txt", ".hidden/h.go", "vendor/x/y.go", "README.md"} {
		e.put(p, "")
	}
	got := func(pat string) string {
		return strings.Join(strings.Split(e.ok("glob_search", map[string]any{"pattern": pat}).Content, "\n"), " ")
	}
	for pat, want := range map[string]string{
		"*.go":          "main.go",
		"**/*.go":       ".hidden/h.go main.go pkg/a.go pkg/deep/b.go vendor/x/y.go",
		"pkg/**/*.go":   "pkg/a.go pkg/deep/b.go",
		"pkg/*":         "pkg/a.go pkg/deep/ pkg/deep/",
		"pkg/*.go":      "pkg/a.go",
		"pkg/deep/?.go": "pkg/deep/b.go",
		"pkg/[ab].go":   "pkg/a.go",
		".hidden/*":     ".hidden/h.go",
		"nothing*":      "(no matches)",
		"./main.go":     "main.go",
		"README.MD":     "(no matches)",
	} {
		if pat == "pkg/*" {
			want = "pkg/a.go pkg/deep/"
		}
		if g := got(pat); g != want {
			t.Errorf("%q: got %q want %q", pat, g, want)
		}
	}
	for _, bad := range []string{"/etc/*", "/", "../*", "a/../../b", "~/x", "", "C:/x/*", "**/../x"} {
		r := e.run("glob_search", map[string]any{"pattern": bad})
		if r.OK && bad != "" {
			t.Errorf("pattern %q must be refused: %q", bad, r.Content)
		}
		if bad != "" && r.Metadata["error_kind"] != "tool_execution_error" {
			t.Errorf("pattern %q: error_kind %v", bad, r.Metadata["error_kind"])
		}
	}
}

func TestGlobDropsSymlinksLeavingTheWorkspace(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	outside := filepath.Join(t.TempDir(), "o.txt")
	os.WriteFile(outside, []byte("x"), 0o644)
	if err := os.Symlink(outside, filepath.Join(e.root, "leak.txt")); err != nil {
		t.Skip("symlinks unavailable")
	}
	e.put("fine.txt", "")
	if r := e.ok("glob_search", map[string]any{"pattern": "*.txt"}); r.Content != "fine.txt" {
		t.Fatalf("got %q", r.Content)
	}
}

func TestGrep(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	e.put("b.txt", "alpha\nbeta needle\ngamma\n")
	e.put("a.txt", "needle first\nx.y\n")
	e.put("sub/c.txt", "deep needle\n")
	e.put(".git/config", "needle in git")
	e.put("bin.dat", "needle\x00binary")
	e.put("long.txt", "needle "+strings.Repeat("z", 2000))
	r := e.ok("grep_search", map[string]any{"pattern": "needle"})
	lines := strings.Split(r.Content, "\n")
	var paths []string
	for _, l := range lines {
		paths = append(paths, strings.SplitN(l, ":", 2)[0])
	}
	if !sort.StringsAreSorted(paths) {
		t.Errorf("results must be in path order: %v", paths)
	}
	if strings.Contains(r.Content, ".git") || strings.Contains(r.Content, "bin.dat") {
		t.Errorf("VCS directories and binary files are skipped: %q", r.Content)
	}
	for _, want := range []string{"a.txt:1: needle first", "b.txt:2: beta needle", "sub/c.txt:1: deep needle"} {
		if !strings.Contains(r.Content, want) {
			t.Errorf("missing %q in %q", want, r.Content)
		}
	}
	for _, l := range lines {
		if strings.HasPrefix(l, "long.txt") && len(l) > 600 {
			t.Errorf("long lines must be cut: %d", len(l))
		}
	}
	if r = e.ok("grep_search", map[string]any{"pattern": "x.y", "literal": true}); !strings.Contains(r.Content, "a.txt:2: x.y") {
		t.Errorf("literal: %q", r.Content)
	}
	if r = e.ok("grep_search", map[string]any{"pattern": "x.y", "path": "b.txt"}); r.Content != "(no matches)" {
		t.Errorf("single file: %q", r.Content)
	}
	if r = e.ok("grep_search", map[string]any{"pattern": "^deep", "path": "sub"}); r.Content != "sub/c.txt:1: deep needle" {
		t.Errorf("subdir: %q", r.Content)
	}
	if r = e.ok("grep_search", map[string]any{"pattern": "needle", "max_matches": 2}); !strings.Contains(r.Content, "stopped at 2 matches") || strings.Count(r.Content, "needle") < 2 {
		t.Errorf("max_matches: %q", r.Content)
	}
	e.fails("grep_search", map[string]any{"pattern": "(unclosed"}, "tool_execution_error")
	e.fails("grep_search", map[string]any{"pattern": "(?=look)ahead"}, "tool_execution_error")
	e.fails("grep_search", map[string]any{"pattern": "x", "path": "../"}, "tool_execution_error")
	e.fails("grep_search", map[string]any{"pattern": "x", "path": "nope"}, "tool_execution_error")
}

// ---- guards -----------------------------------------------------------------------------

func TestEditLoopGuard(t *testing.T) {
	e := newEnv(t, writeAll())
	e.put("f.txt", "start")
	for i := 0; i < 5; i++ {
		e.ok("write_file", map[string]any{"path": "f.txt", "content": strings.Repeat("v", i+1)})
	}
	r := e.fails("write_file", map[string]any{"path": "f.txt", "content": "again"}, "tool_execution_error")
	if !strings.Contains(r.Content, "fundamentally different") {
		t.Fatalf("message: %q", r.Content)
	}
	if e.read("f.txt") != "vvvvv" {
		t.Fatal("a refused write changed the file")
	}
	// edits on the same path share the counter, and failed edits count too
	e.fails("edit_file", map[string]any{"path": "f.txt", "old_text": "v", "new_text": "w", "replace_all": true}, "tool_execution_error")
	// other paths are unaffected; there is no reset
	e.ok("write_file", map[string]any{"path": "g.txt", "content": "x"})
	e.fails("write_file", map[string]any{"path": "f.txt", "content": "later"}, "tool_execution_error")

	e2 := newEnv(t, writeAll())
	e2.put("h.txt", "abc")
	for i := 0; i < 5; i++ {
		e2.fails("edit_file", map[string]any{"path": "h.txt", "old_text": "missing", "new_text": "x"}, "tool_execution_error")
	}
	r = e2.fails("edit_file", map[string]any{"path": "h.txt", "old_text": "abc", "new_text": "x"}, "tool_execution_error")
	if !strings.Contains(r.Content, "fundamentally different") || e2.read("h.txt") != "abc" {
		t.Fatalf("failed edits must count toward the limit: %q", r.Content)
	}
}

func TestSummaryMarkdownChurnGuard(t *testing.T) {
	for p, churn := range map[string]bool{
		"SUMMARY.md": true, "final_summary.md": true, "findings.md": true, "CRITICAL-ISSUES.md": true, "session_notes.md": true,
		"gaps_filled.md": true, "gap-filled.md": true, "quick-reference.md": true, "deliverables.md": true, "status_report.md": true,
		"progress-report.md": true, "wrap-up.md": true, "completion_report.md": true, "implementation-summary.md": true,
		"notes.md": false, "design.md": false, "README.md": false, "README_SUMMARY.md": false, "CHANGELOG.md": false, "LICENSE.md": false,
		"CONTRIBUTING.md": false, "docs/summary.md": false, "a/Docs/findings.md": false, "summary.txt": false, "summary.go": false,
		"x/summary.md": true,
	} {
		if got := isChurnMarkdown(p); got != churn {
			t.Errorf("isChurnMarkdown(%q) = %v, want %v", p, got, churn)
		}
	}
	e := newEnv(t, writeAll())
	for i := 0; i < 4; i++ {
		e.ok("write_file", map[string]any{"path": "SUMMARY_" + string(rune('a'+i)) + ".md", "content": "x"})
	}
	r := e.fails("write_file", map[string]any{"path": "summary_e.md", "content": "x"}, "tool_execution_error")
	if !strings.Contains(r.Content, "not progress") {
		t.Fatalf("message: %q", r.Content)
	}
	e.ok("write_file", map[string]any{"path": "docs/summary.md", "content": "x"})
	e.ok("write_file", map[string]any{"path": "real_code.go", "content": "x"})
}

func TestSelfAuthoredBannerOnRead(t *testing.T) {
	e := newEnv(t, writeAll())
	e.put("theirs.txt", "external")
	if r := e.ok("read_file", map[string]any{"path": "theirs.txt"}); strings.Contains(r.Content, "note:") {
		t.Fatalf("a file the agent did not write gets no banner: %q", r.Content)
	}
	e.ok("write_file", map[string]any{"path": "mine.txt", "content": "my own text"})
	r := e.ok("read_file", map[string]any{"path": "mine.txt"})
	if !strings.HasPrefix(r.Content, "[note:") || !strings.HasSuffix(r.Content, "my own text") {
		t.Fatalf("banner: %q", r.Content)
	}
	e.ok("edit_file", map[string]any{"path": "theirs.txt", "old_text": "external", "new_text": "changed"})
	if r = e.ok("read_file", map[string]any{"path": "theirs.txt"}); !strings.HasPrefix(r.Content, "[note:") {
		t.Fatalf("an edited file counts as self-authored: %q", r.Content)
	}
	// state is per context: a rebuilt context starts clean
	e.tc.Runtimes = nil
	if r = e.ok("read_file", map[string]any{"path": "mine.txt"}); strings.Contains(r.Content, "note:") {
		t.Fatalf("a new context has no memory of earlier writes: %q", r.Content)
	}
}
