package tools

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

func simple(name string, h Handler) Tool {
	return Tool{Spec: llm.ToolSpec{Name: name, Description: name}, Handler: h}
}

func TestExecuteContract(t *testing.T) {
	reg := NewRegistry(
		simple("good", func(context.Context, map[string]any, *Context) (string, map[string]any, error) {
			return "fine", map[string]any{"path": "p"}, nil
		}),
		simple("denied", func(context.Context, map[string]any, *Context) (string, map[string]any, error) {
			return "", nil, Denied("rerun with --allow-write")
		}),
		simple("broken", func(context.Context, map[string]any, *Context) (string, map[string]any, error) {
			return "", map[string]any{"keep": 1}, errors.New("disk on fire")
		}),
		simple("panics", func(context.Context, map[string]any, *Context) (string, map[string]any, error) {
			var m map[string]int
			m["x"] = 1
			return "", nil, nil
		}),
	)
	r := reg.Execute(t.Context(), "good", nil, &Context{})
	if !r.OK || r.Content != "fine" || r.Metadata["path"] != "p" || r.Name != "good" {
		t.Fatalf("good: %+v", r)
	}
	r = reg.Execute(t.Context(), "denied", nil, &Context{})
	if r.OK || r.Metadata["error_kind"] != "permission_denied" || !strings.Contains(r.Content, "--allow-write") {
		t.Fatalf("denied: %+v", r)
	}
	r = reg.Execute(t.Context(), "broken", nil, &Context{})
	if r.OK || r.Metadata["error_kind"] != "tool_execution_error" || r.Metadata["keep"] != 1 || r.Content != "disk on fire" {
		t.Fatalf("broken: %+v", r)
	}
	r = reg.Execute(t.Context(), "panics", nil, &Context{})
	if r.OK || r.Metadata["error_kind"] != "tool_execution_error" {
		t.Fatalf("a handler panic must become an error result: %+v", r)
	}
	r = reg.Execute(t.Context(), "nope", nil, &Context{})
	if r.OK || !strings.Contains(r.Content, "unknown tool") {
		t.Fatalf("unknown: %+v", r)
	}
	if _, has := r.Metadata["error_kind"]; has {
		t.Fatal("unknown tools carry no error_kind")
	}
}

func TestFilterSemantics(t *testing.T) {
	base := NewRegistry(simple("a", nil), simple("b", nil), simple("c", nil))
	if got := base.Filter(nil).Names(); strings.Join(got, ",") != "a,b,c" {
		t.Fatalf("nil allowlist = %v", got)
	}
	if got := base.Filter([]string{}); got.Len() != 0 || len(got.Specs()) != 0 {
		t.Fatalf("empty allowlist must yield zero tools: %v", got.Names())
	}
	if got := base.Filter([]string{"c", "a", "zzz"}).Names(); strings.Join(got, ",") != "a,c" {
		t.Fatalf("filtered = %v (unknown names grant nothing; order is registration order)", got)
	}
	var nilReg *Registry
	if nilReg.Len() != 0 || nilReg.Specs() != nil {
		t.Fatal("nil registry must be safe")
	}
}

func TestRegisterReplacesByName(t *testing.T) {
	reg := NewRegistry(simple("a", nil), simple("b", nil))
	reg.Register(Tool{Spec: llm.ToolSpec{Name: "a", Description: "new"}})
	if reg.Len() != 2 || reg.Specs()[0].Description != "new" {
		t.Fatalf("specs = %+v", reg.Specs())
	}
}

func TestBuilderReappliesAllowlistAndProviders(t *testing.T) {
	saved := providers
	t.Cleanup(func() { providers = saved })
	providers = nil
	RegisterProvider(func() []Tool { return []Tool{simple("limbo_validate", nil), simple("limbo_profiles", nil)} })

	all := Builder{}.Build()
	names := strings.Join(all.Names(), ",")
	if !strings.HasPrefix(names, "read_file,") || !strings.HasSuffix(names, ",limbo_validate,limbo_profiles") || !strings.Contains(names, ",bash,") {
		t.Fatalf("default registry = %s (providers must be appended after the built-ins)", names)
	}
	b := Builder{Allowlist: []string{"limbo_profiles"}, Extra: []Tool{simple("extra", nil)}}
	for i := 0; i < 3; i++ { // every rebuild keeps the restriction
		if got := b.Build().Names(); len(got) != 1 || got[0] != "limbo_profiles" {
			t.Fatalf("rebuild %d = %v", i, got)
		}
	}
	if got := (Builder{Allowlist: []string{}}).Build(); got.Len() != 0 {
		t.Fatalf("empty allowlist built %v", got.Names())
	}
	if got := (Builder{NoDefaults: true, Extra: []Tool{simple("only", nil)}}).Build().Names(); len(got) != 1 || got[0] != "only" {
		t.Fatalf("NoDefaults = %v", got)
	}
}

func TestReadFile(t *testing.T) {
	root := t.TempDir()
	os.WriteFile(filepath.Join(root, "f.txt"), []byte("one\ntwo\nthree\nfour"), 0o644)
	outside := filepath.Join(t.TempDir(), "secret.txt")
	os.WriteFile(outside, []byte("nope"), 0o644)
	tc := &Context{Root: root, MaxOutputChars: 12000}
	reg := NewRegistry(ReadFile())
	run := func(args map[string]any) Result { return reg.Execute(t.Context(), "read_file", args, tc) }

	r := run(map[string]any{"path": "f.txt"})
	if !r.OK || r.Content != "one\ntwo\nthree\nfour" {
		t.Fatalf("full read: %+v", r)
	}
	r = run(map[string]any{"path": "f.txt", "start_line": 2, "end_line": 3})
	if r.Content != "2: two\n3: three" {
		t.Fatalf("slice: %q", r.Content)
	}
	r = run(map[string]any{"path": "f.txt", "start_line": "3"})
	if r.Content != "3: three\n4: four" {
		t.Fatalf("string start_line: %q", r.Content)
	}
	if r = run(map[string]any{"path": "f.txt", "start_line": 99}); !r.OK || !strings.Contains(r.Content, "no lines") {
		t.Fatalf("a range past the end is an empty result, not an error: %+v", r)
	}
	if r = run(map[string]any{"path": "f.txt", "start_line": 0}); r.OK {
		t.Fatal("start_line below the schema minimum must be refused")
	}
	for _, p := range []string{"../" + filepath.Base(filepath.Dir(outside)) + "/secret.txt", outside, "", "missing.txt"} {
		if r = run(map[string]any{"path": p}); r.OK {
			t.Fatalf("path %q must be refused: %+v", p, r)
		}
	}
	if r = run(map[string]any{"path": filepath.Join(root, "f.txt")}); !r.OK {
		t.Fatalf("absolute path inside the root must work: %+v", r)
	}
	if r = run(nil); r.OK || r.Metadata["error_kind"] != "tool_execution_error" {
		t.Fatalf("missing path: %+v", r)
	}
}

func TestReadFileSymlinkEscape(t *testing.T) {
	root := t.TempDir()
	target := filepath.Join(t.TempDir(), "outside.txt")
	os.WriteFile(target, []byte("secret"), 0o644)
	if err := os.Symlink(target, filepath.Join(root, "link.txt")); err != nil {
		t.Skip("symlinks unavailable here: ", err)
	}
	reg := NewRegistry(ReadFile())
	r := reg.Execute(t.Context(), "read_file", map[string]any{"path": "link.txt"}, &Context{Root: root})
	if r.OK {
		t.Fatalf("a symlink leaving the workspace must be refused: %+v", r)
	}
}

func TestTruncateOutput(t *testing.T) {
	if TruncateOutput("short", 100) != "short" || TruncateOutput("anything", 0) != "anything" {
		t.Fatal("under the limit must be untouched")
	}
	long := strings.Repeat("a", 500) + strings.Repeat("z", 500)
	got := TruncateOutput(long, 100)
	if !strings.HasPrefix(got, "aaa") || !strings.HasSuffix(got, "zzz") || !strings.Contains(got, "characters omitted") {
		t.Fatalf("got %q", got)
	}
	uni := strings.Repeat("é", 300)
	if g := TruncateOutput(uni, 50); strings.ContainsRune(g, '�') {
		t.Fatal("truncation split a rune")
	}
}

func TestBuilderPermsStopOfferingDeniedToolsButStillRefuseCalls(t *testing.T) {
	none := config.Permissions{}
	reg := Builder{Perms: &none}.Build()
	for _, n := range reg.Names() {
		if tool, _ := reg.Get(n); tool.Requires != PermNone {
			t.Errorf("%s needs %v but is offered with no permissions", n, tool.Requires)
		}
	}
	for _, s := range reg.Specs() {
		if s.Name == "bash" || s.Name == "write_file" {
			t.Errorf("%s must not be in the request", s.Name)
		}
	}
	res := reg.Execute(context.Background(), "bash", map[string]any{"command": "echo hi"}, &Context{Root: t.TempDir()})
	if res.OK || res.Metadata["error_kind"] != "permission_denied" {
		t.Fatalf("a call that arrives anyway must be denied: %+v", res)
	}
	write := config.Permissions{AllowFileWrite: true}
	got := Builder{Perms: &write}.Build()
	names := got.Names()
	if !slices.Contains(names, "write_file") || slices.Contains(names, "bash") {
		t.Fatalf("write only: %v", names)
	}
	if !slices.Contains(Builder{}.Build().Names(), "bash") {
		t.Fatal("no Perms means no filtering")
	}
}
