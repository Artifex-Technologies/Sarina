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

// realOutput runs a test command in dir and returns what it printed (the check sees exactly this).
func realOutput(t *testing.T, dir string, argv ...string) string {
	t.Helper()
	if _, err := exec.LookPath(argv[0]); err != nil {
		t.Skip("no " + argv[0])
	}
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GOPROXY=off", "GOFLAGS=-mod=mod", "GOTOOLCHAIN=local")
	b, _ := cmd.CombinedOutput()
	return string(b)
}

func goModule(t *testing.T, files map[string]string) string {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example.com/calc\n\ngo 1.21\n"), 0o644)
	for n, c := range files {
		os.WriteFile(filepath.Join(dir, n), []byte(c), 0o644)
	}
	return dir
}

func TestLocaliseAGoTestFailure(t *testing.T) {
	dir := goModule(t, map[string]string{
		"calc.go":      "package calc\n\n// Add adds.\nfunc Add(a, b int) int {\n\treturn a - b\n}\n",
		"calc_test.go": "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tgot := Add(2, 3)\n\tif got != 5 {\n\t\tt.Errorf(\"Add(2, 3) = %d, want 5\", got)\n\t}\n}\n",
	})
	out := realOutput(t, dir, "go", "test", "-count=1", ".")
	s, lines := localise(out, dir)
	if !strings.HasPrefix(s, "TestAdd in calc_test.go:8 failed: Add(2, 3) = -1, want 5; Add is at calc.go:4.") {
		t.Fatalf("summary: %q\n%s", s, out)
	}
	if !strings.Contains(lines, "5: \treturn a - b") || !strings.Contains(lines, "6: }") {
		t.Fatalf("the function's lines: %q", lines)
	}
}

func TestLocaliseAGoPanic(t *testing.T) {
	dir := goModule(t, map[string]string{
		"util.go":      "package calc\n\nfunc Half(n int, by int) int {\n\treturn n / by\n}\n",
		"util_test.go": "package calc\n\nimport \"testing\"\n\nfunc TestHalf(t *testing.T) {\n\tif Half(4, 0) != 2 {\n\t\tt.Fatal()\n\t}\n}\n",
	})
	out := realOutput(t, dir, "go", "test", "-count=1", ".")
	s, lines := localise(out, dir)
	if !strings.Contains(s, "TestHalf panicked: runtime error: integer divide by zero, in Half at util.go:4.") {
		t.Fatalf("summary: %q\n%s", s, out)
	}
	if !strings.Contains(lines, "4: \treturn n / by") {
		t.Fatalf("lines: %q", lines)
	}
}

func TestLocaliseAPythonUnittestFailure(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "slug.py"), []byte("def slugify(title):\n    return title.replace(' ', '-').lower()\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "test_slug.py"), []byte("import unittest\nfrom slug import slugify\n\n\nclass SlugTest(unittest.TestCase):\n    def test_spaces(self):\n        self.assertEqual(slugify('Hello  World'), 'hello-world')\n\n\nif __name__ == '__main__':\n    unittest.main()\n"), 0o644)
	out := realOutput(t, dir, "python3", "-m", "unittest", "-q")
	s, lines := localise(out, dir)
	if !strings.HasPrefix(s, "test_spaces (test_slug.SlugTest") || !strings.Contains(s, "failed at test_slug.py:7 (in test_spaces): AssertionError:") ||
		!strings.HasSuffix(s, "; slugify is at slug.py:1.") {
		t.Fatalf("summary: %q\n%s", s, out)
	}
	if !strings.Contains(lines, "2:     return title.replace(' ', '-').lower()") {
		t.Fatalf("lines: %q", lines)
	}
}

func TestLocaliseAPythonTraceback(t *testing.T) {
	// code-python-fix, 2026-11-06: an edit left a "return" outside its function; the tests cannot even import the module
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "slug.py"), []byte("import re\n\nreturn re.sub(r'\\s+', '-', title).lower()\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "test_slug.py"), []byte("import unittest\nfrom slug import slugify\n\n\nclass SlugTest(unittest.TestCase):\n    def test_spaces(self):\n        self.assertEqual(slugify('a b'), 'a-b')\n"), 0o644)
	out := realOutput(t, dir, "python3", "-m", "unittest", "-q")
	s, lines := localise(out, dir)
	if !strings.Contains(s, "failed at slug.py:3") || !strings.Contains(s, "SyntaxError: 'return' outside function") {
		t.Fatalf("summary: %q\n%s", s, out)
	}
	if !strings.Contains(lines, "3: return re.sub") {
		t.Fatalf("lines: %q", lines)
	}
	// an error raised inside her code is pointed at where it is raised, not at the test
	os.WriteFile(filepath.Join(dir, "slug.py"), []byte("def slugify(title):\n    words = title.split()\n    return words[5]\n"), 0o644)
	out = realOutput(t, dir, "python3", "-m", "unittest", "-q")
	if s, _ = localise(out, dir); !strings.Contains(s, "failed at slug.py:3 (in slugify): IndexError: list index out of range.") {
		t.Fatalf("raised in her code: %q\n%s", s, out)
	}
}

func TestVerifyPutsTheLocalisedLineFirstAndRetriesTheFailedTest(t *testing.T) {
	if _, err := exec.LookPath("go"); err != nil {
		t.Skip("no go toolchain")
	}
	t.Setenv("SARINA_SANDBOX_TESTS", "off")
	var long strings.Builder
	for i := 0; i < 60; i++ {
		long.WriteString("\tt.Log(\"noise\")\n")
	}
	dir := goModule(t, map[string]string{
		"calc.go":      "package calc\n\nfunc Add(a, b int) int {\n\treturn a - b\n}\n",
		"calc_test.go": "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n" + long.String() + "\tif Add(2, 3) != 5 {\n\t\tt.Fatalf(\"Add(2, 3) = %d, want 5\", Add(2, 3))\n\t}\n}\n\nfunc TestOther(t *testing.T) {}\n",
	})
	tc := &Context{Root: dir, Permissions: config.Permissions{AllowShellCommands: true}}
	code := filepath.Join(dir, "calc.go")
	note, md := verifyCode(context.Background(), code, tc)
	if md["verify"] != "failed" || md["verify_kind"] != "tests" {
		t.Fatalf("%v %q", md, note)
	}
	first := strings.SplitN(note, "\n", 4)
	if len(first) < 3 || !strings.HasPrefix(first[2], "TestAdd in calc_test.go:67 failed: Add(2, 3) = -1, want 5; Add is at calc.go:3.") {
		t.Fatalf("the computed line must come first:\n%s", note)
	}
	if strings.Contains(note, "go vet") {
		t.Fatalf("go vet runs as part of go test, not on its own:\n%s", note)
	}
	if !strings.Contains(note, "lines left out") || !strings.Contains(md["verify_output"].(string), "noise") {
		t.Fatalf("a long output is cut in the answer and kept whole in the metadata:\n%s", note)
	}
	// the retry runs the test that failed first
	note, _ = verifyCode(context.Background(), code, tc)
	if !strings.Contains(note, "-run ^TestAdd$") {
		t.Fatalf("the retry should run TestAdd first:\n%s", note)
	}
	os.WriteFile(code, []byte("package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n"), 0o644)
	note, md = verifyCode(context.Background(), code, tc)
	if md["verify"] != "ok" || md["verify_tests"] != true {
		t.Fatalf("fixed: %v %q", md, note)
	}
	if stateOf(tc).failedTest[dir+"|./..."] != "" {
		t.Fatal("a passing package forgets the failed test")
	}
}
