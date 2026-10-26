package tools

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

func TestVerifyCodeGo(t *testing.T) {
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("no go toolchain")
	}
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example.com/v\n\ngo 1.21\n"), 0o644)
	good := filepath.Join(dir, "a.go")
	os.WriteFile(good, []byte("package v\n\nfunc Add(a, b int) int { return a + b }\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "a_test.go"), []byte("package v\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) { if Add(1, 2) != 3 { t.Fatal() } }\n"), 0o644)
	tc := &Context{Root: dir}
	t.Setenv("SARINA_SANDBOX_TESTS", "off")
	note, md := verifyCode(context.Background(), good, tc)
	if md["verify"] != "ok" || !strings.Contains(note, "Check passed") || !strings.Contains(note, "Tests were not run") {
		t.Fatalf("build-only: %v %q", md, note)
	}
	tc.Permissions = config.Permissions{AllowShellCommands: true}
	note, md = verifyCode(context.Background(), good, tc)
	if md["verify"] != "ok" || md["verify_tests"] != true || !strings.Contains(note, "go test") {
		t.Fatalf("with tests: %v %q", md, note)
	}
	os.WriteFile(good, []byte("package v\n\nfunc Add(a, b int) int { return a - b }\n"), 0o644)
	note, md = verifyCode(context.Background(), good, tc)
	if md["verify"] != "failed" || !strings.Contains(note, "CHECK FAILED (tests") {
		t.Fatalf("a failing test must fail the check: %v %q", md, note)
	}
	os.WriteFile(good, []byte("package v\n\nfunc Add(a, b int) int { return a + }\n"), 0o644)
	note, md = verifyCode(context.Background(), good, tc)
	if md["verify"] != "failed" || !strings.Contains(note, "CHECK FAILED (build") {
		t.Fatalf("a syntax error must fail the build: %v %q", md, note)
	}
}

func TestVerifyCodePythonShellAndText(t *testing.T) {
	dir := t.TempDir()
	py := filepath.Join(dir, "s.py")
	os.WriteFile(py, []byte("print('hi'\n"), 0o644)
	if _, md := verifyCode(context.Background(), py, &Context{Root: dir}); md["verify"] != "failed" {
		t.Fatalf("broken python passed: %v", md)
	}
	os.WriteFile(py, []byte("print('hi')\n"), 0o644)
	if _, md := verifyCode(context.Background(), py, &Context{Root: dir}); md["verify"] != "ok" {
		t.Fatalf("good python failed: %v", md)
	}
	sh := filepath.Join(dir, "s.sh")
	os.WriteFile(sh, []byte("if [ -z \"$1\" ]; then\n"), 0o644)
	if _, md := verifyCode(context.Background(), sh, &Context{Root: dir}); md["verify"] != "failed" {
		t.Fatalf("broken shell passed: %v", md)
	}
	txt := filepath.Join(dir, "notes.txt")
	os.WriteFile(txt, []byte("hello\n"), 0o644)
	if note, md := verifyCode(context.Background(), txt, &Context{Root: dir}); note != "" || md["verify"] != "skipped" {
		t.Fatalf("text is not checked: %q %v", note, md)
	}
}

func TestTruncateFileNamesTheMissingLines(t *testing.T) {
	var b strings.Builder
	for i := 1; i <= 100; i++ {
		b.WriteString(strings.Repeat("x", 20) + "\n")
	}
	out := truncateFile(b.String(), 600)
	if !strings.Contains(out, "omitted") || !strings.Contains(out, "of 101") || !strings.Contains(out, "start_line") {
		t.Fatalf("%q", out)
	}
	if truncateFile("short\n", 600) != "short\n" {
		t.Fatal("a short file is unchanged")
	}
}

func TestVerifyRunsHerTestsInASandbox(t *testing.T) {
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("no go toolchain")
	}
	if !haveBwrap() {
		t.Skip("no bubblewrap")
	}
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example.com/v\n\ngo 1.21\n"), 0o644)
	good := filepath.Join(dir, "a.go")
	os.WriteFile(good, []byte("package v\n\nfunc Add(a, b int) int { return a + b }\n"), 0o644)
	// The test writes into its folder and tries the network: the copy takes the write, the network is not there.
	os.WriteFile(filepath.Join(dir, "a_test.go"), []byte(`package v

import (
	"net"
	"os"
	"testing"
)

func TestAdd(t *testing.T) {
	os.WriteFile("left-behind", []byte("x"), 0o644)
	if c, err := net.Dial("tcp", "1.1.1.1:80"); err == nil {
		c.Close()
		t.Fatal("network reachable")
	}
	if Add(1, 2) != 3 {
		t.Fatal()
	}
}
`), 0o644)
	tc := &Context{Root: dir}
	note, md := verifyCode(context.Background(), good, tc)
	if md["verify"] != "ok" || md["verify_tests"] != true || !strings.Contains(note, "sandbox") {
		t.Fatalf("sandboxed tests: %v %q", md, note)
	}
	if _, err := os.Stat(filepath.Join(dir, "left-behind")); err == nil {
		t.Fatal("a sandboxed test wrote into the real project")
	}
	os.WriteFile(good, []byte("package v\n\nfunc Add(a, b int) int { return a - b }\n"), 0o644)
	if note, md = verifyCode(context.Background(), good, tc); md["verify"] != "failed" || !strings.Contains(note, "CHECK FAILED (tests") {
		t.Fatalf("a failing test in the sandbox must fail the check: %v %q", md, note)
	}
}
