package tools

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

func writeCheckFiles(t *testing.T, dir string, files map[string]string) {
	t.Helper()
	for name, body := range files {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
}

// The eval code-fix-failing-test's package: Add returns a - b. check_code on the folder (and on the file) names the failing test and
// the function it calls, and shows that function's lines, before anything is edited; the real folder is left as it was.
func TestCheckCodeNamesTheFailingTestAndTheFunction(t *testing.T) {
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("no go toolchain")
	}
	if !haveBwrap() {
		t.Skip("no bubblewrap")
	}
	t.Setenv("SARINA_SANDBOX_TESTS", "")
	dir := t.TempDir()
	writeCheckFiles(t, dir, map[string]string{
		"go.mod":       "module example.com/calc\n\ngo 1.21\n",
		"calc.go":      "package calc\n\n// Add returns the sum of a and b.\nfunc Add(a, b int) int {\n\treturn a - b\n}\n",
		"calc_test.go": "package calc\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nfunc TestAdd(t *testing.T) {\n\tos.WriteFile(\"left-behind\", []byte(\"x\"), 0o644)\n\tif got := Add(2, 3); got != 5 {\n\t\tt.Errorf(\"Add(2, 3) = %d, want 5\", got)\n\t}\n}\n",
	})
	reg := NewRegistry(CheckCode())
	tc := &Context{Root: dir, Permissions: config.Permissions{AllowFileWrite: true}}
	for _, p := range []string{".", "calc.go"} {
		r := reg.Execute(t.Context(), "check_code", map[string]any{"path": p}, tc)
		if !r.OK || r.Metadata["verify"] != "failed" || r.Metadata["verify_kind"] != "tests" {
			t.Fatalf("%s: a failing test must fail the check: %+v", p, r)
		}
		for _, want := range []string{"TestAdd in calc_test.go", "Add is at calc.go", "return a - b"} {
			if !strings.Contains(r.Content, want) {
				t.Fatalf("%s: %q missing from %q", p, want, r.Content)
			}
		}
		if !strings.HasPrefix(r.Content, "check_code ") || r.Metadata["action"] != "check_code" {
			t.Fatalf("%s: %+v", p, r)
		}
	}
	if _, err := os.Stat(filepath.Join(dir, "left-behind")); err == nil {
		t.Fatal("her test ran on the real project, not in the sandbox")
	}
	writeCheckFiles(t, dir, map[string]string{"calc.go": "package calc\n\n// Add returns the sum of a and b.\nfunc Add(a, b int) int {\n\treturn a + b\n}\n"})
	r := reg.Execute(t.Context(), "check_code", map[string]any{"path": "."}, tc)
	if !r.OK || r.Metadata["verify"] != "ok" || r.Metadata["verify_tests"] != true || !strings.Contains(r.Content, "sandbox") {
		t.Fatalf("the fixed package: %+v", r)
	}
}

func TestCheckCodePythonSyntaxError(t *testing.T) {
	if _, err := exec.LookPath("python3"); err != nil {
		t.Skip("no python3")
	}
	dir := t.TempDir()
	writeCheckFiles(t, dir, map[string]string{"slug.py": "def slugify(s):\n    return s.replace(\"(\", \"\").replace(\")\"\", \"\")\n"})
	reg := NewRegistry(CheckCode())
	tc := &Context{Root: dir, Permissions: config.Permissions{AllowFileWrite: true}}
	r := reg.Execute(t.Context(), "check_code", map[string]any{"path": "slug.py"}, tc)
	if !r.OK || r.Metadata["verify"] != "failed" || !strings.Contains(r.Content, "CHECK FAILED (build") || !strings.Contains(r.Content, "slug.py as it is now, around line 2") {
		t.Fatalf("a syntax error must fail the build and show the line: %+v", r)
	}
}

func TestCheckCodeRefusals(t *testing.T) {
	dir := t.TempDir()
	outside := t.TempDir()
	writeCheckFiles(t, outside, map[string]string{"x.go": "package x\n"})
	writeCheckFiles(t, dir, map[string]string{"notes.txt": "hello\n"})
	reg := NewRegistry(CheckCode())
	tc := &Context{Root: dir, Permissions: config.Permissions{AllowFileWrite: true}}
	for _, c := range []struct{ path, want string }{
		{filepath.Join(outside, "x.go"), "outside the workspace"},
		{"../" + filepath.Base(outside) + "/x.go", "outside the workspace"},
		{"notes.txt", "none of them"},
		{".", "writes nothing: write the code with write_file first"},
		{"missing.go", "missing.go does not exist. check_code only checks files that exist and writes nothing"},
	} {
		r := reg.Execute(t.Context(), "check_code", map[string]any{"path": c.path}, tc)
		if r.OK || !strings.Contains(r.Content, c.want) {
			t.Fatalf("%s: want a refusal with %q, got %+v", c.path, c.want, r)
		}
	}
	// a read-only session gains nothing: the tool needs the write permission
	r := reg.Execute(t.Context(), "check_code", map[string]any{"path": "notes.txt"}, &Context{Root: dir})
	if r.OK || r.Metadata["error_kind"] != "permission_denied" {
		t.Fatalf("read-only: %+v", r)
	}
}

func TestCheckCodeWithoutASandboxRunsOnlyTheBuild(t *testing.T) {
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("no go toolchain")
	}
	t.Setenv("SARINA_SANDBOX_TESTS", "off")
	dir := t.TempDir()
	writeCheckFiles(t, dir, map[string]string{
		"go.mod":       "module example.com/calc\n\ngo 1.21\n",
		"calc.go":      "package calc\n\nfunc Add(a, b int) int { return a - b }\n",
		"calc_test.go": "package calc\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nfunc TestAdd(t *testing.T) {\n\tos.WriteFile(\"left-behind\", []byte(\"x\"), 0o644)\n\tif Add(2, 3) != 5 {\n\t\tt.Fatal()\n\t}\n}\n",
	})
	// even with shell commands allowed her tests never run on the project itself through check_code
	tc := &Context{Root: dir, Permissions: config.Permissions{AllowFileWrite: true, AllowShellCommands: true}}
	r := NewRegistry(CheckCode()).Execute(t.Context(), "check_code", map[string]any{"path": "."}, tc)
	if !r.OK || r.Metadata["verify"] != "ok" || r.Metadata["verify_tests"] == true || !strings.Contains(r.Content, "no sandbox") {
		t.Fatalf("no sandbox: %+v", r)
	}
	if _, err := os.Stat(filepath.Join(dir, "left-behind")); err == nil {
		t.Fatal("her test ran on the real project")
	}
}

func TestCodeFileIn(t *testing.T) {
	dir := t.TempDir()
	writeCheckFiles(t, dir, map[string]string{"test_slug.py": "", "slug.py": "", "README.md": ""})
	if got := filepath.Base(codeFileIn(dir)); got != "slug.py" {
		t.Fatalf("code before its test: %s", got)
	}
	mod := t.TempDir()
	writeCheckFiles(t, mod, map[string]string{"go.mod": "module m\n", "three.txt": "1\n2\n3\n"})
	// night 6 code-go-package: an empty module checked as ./... failed with "matched no packages", and she called check_code again
	// instead of writing the files
	if got := codeFileIn(mod); got != "" {
		t.Fatalf("a module with no Go file yet has nothing to check: %s", got)
	}
	os.MkdirAll(filepath.Join(mod, "stack"), 0o755)
	writeCheckFiles(t, filepath.Join(mod, "stack"), map[string]string{"stack.go": "package stack\n"})
	if got := codeFileIn(mod); got != filepath.Join(mod, "doc.go") {
		t.Fatalf("a module root stands for its packages: %s", got)
	}
}
