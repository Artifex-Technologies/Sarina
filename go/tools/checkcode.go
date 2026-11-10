package tools

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// check_code runs, on demand, the checks the verify hook runs after a change (the build and the package's tests), so she can see a
// failure before she edits instead of guessing at it from the code (plan Day 6, "reproduce before edit"; eval code-fix-failing-test,
// where she read calc.go, described the fix and changed nothing). Her tests run only in the sandbox (bubblewrap: a copy of the project,
// no network, the system read-only), never on the project itself, whatever the session allows; without a sandbox only the build check
// runs. It needs the write permission, like the file tools: a read-only session gains nothing from it.

// codeExts is the order a folder's files are tried in: the first language found is the one checked.
var codeExts = []string{".go", ".py", ".rs", ".sh", ".bash", ".js", ".mjs"}

// CheckCode builds a code file or folder and runs its tests in the sandbox.
func CheckCode() Tool {
	return Tool{
		Requires: PermWrite, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "check_code",
			Description: "Build a code file or folder and run its tests in a sandbox; shows the first failure and the code it points at. Run it on failing code before you edit.",
			Parameters:  obj(map[string]any{"path": prop("string", "", "minLength", 1)}, "path"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			given := argStr(args, "path")
			path, err := ResolveInRoot(tc.Root, given)
			if err != nil {
				return "", nil, err
			}
			if err := secretGuard(given, path, "check"); err != nil {
				return "", nil, err
			}
			if err := privateGuard(given, path, "check"); err != nil {
				return "", nil, err
			}
			info, err := os.Stat(path)
			if err != nil {
				return "", nil, fmt.Errorf("cannot access %s: %v", given, err)
			}
			file := path
			if info.IsDir() {
				if file = codeFileIn(path); file == "" {
					return "", nil, fmt.Errorf("%s has no Go, Python, Rust, shell or JavaScript file directly in it; give the file or the package folder", given)
				}
			}
			sandbox := haveBwrap() && os.Getenv("SARINA_SANDBOX_TESTS") != "off"
			note, md := verifyRun(ctx, file, tc, sandbox, false, sandbox, "there is no sandbox (bubblewrap) here, and check_code runs tests only in one")
			if note == "" {
				return "", nil, fmt.Errorf("check_code checks Go, Python, Rust, shell and JavaScript files; %s is none of them", given)
			}
			md["action"], md["path"] = "check_code", relPath(tc.Root, path)
			// the failure may be the finding itself (the owner's code her test caught: eval tt-code-test), so no order to fix it
			note = strings.Replace(note, "\nFix this before you answer; do not say the code works.", "\nThis is how it fails now; do not say the code works.", 1)
			return "check_code " + relPath(tc.Root, path) + ": " + strings.TrimPrefix(note, "\n"), md, nil
		},
	}
}

// codeFileIn picks the file a folder is checked through: a Go module root without Go files of its own stands for all its packages,
// and a code file before a test file, so a Python folder's own tests run (verifyRun runs a test file alone).
func codeFileIn(dir string) string {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return ""
	}
	for _, ext := range codeExts {
		test := ""
		for _, e := range entries {
			n := e.Name()
			if !e.Type().IsRegular() || strings.ToLower(filepath.Ext(n)) != ext {
				continue
			}
			if strings.HasPrefix(n, "test_") || strings.HasSuffix(n, "_test.go") || strings.HasSuffix(n, "_test.py") {
				if test == "" {
					test = n
				}
				continue
			}
			return filepath.Join(dir, n)
		}
		if test != "" {
			return filepath.Join(dir, test)
		}
		if ext == ".go" {
			if _, err := os.Stat(filepath.Join(dir, "go.mod")); err == nil {
				return filepath.Join(dir, "doc.go") // need not exist: the Go check reads only its folder
			}
		}
	}
	return ""
}
