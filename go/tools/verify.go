package tools

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// After write_file or edit_file changes a code file, the change is checked the way a careful person would before saying "done":
// a build or syntax check of the language, and the project's own tests where they can run. The result goes back to the model in
// the tool's answer (errors included, so it can fix them before replying), and into the file history as "verify" (ok, failed,
// skipped), which the agent's claim check reads: an answer may not say the code builds or the tests pass unless this says so.
//
// Nothing here reaches the network (Go runs with GOPROXY=off), and nothing runs for longer than verifyTimeout. A test run executes
// the model's own code, so tests run only when shell commands are allowed in this session or SARINA_VERIFY_TESTS=on; a build or a
// syntax check executes nothing of hers and always runs.

const verifyTimeout = 90 * time.Second

var cargoManifestRE = regexp.MustCompile(`(?m)^\[package\]`)

// verifyCode checks the changed file. It returns a line for the tool's answer and the metadata keys verify (ok|failed|skipped),
// verify_tests (true when tests ran and passed) and verify_output.
func verifyCode(ctx context.Context, path string, tc *Context) (string, map[string]any) {
	md := map[string]any{"verify": "skipped"}
	ext := strings.ToLower(filepath.Ext(path))
	dir := filepath.Dir(path)
	tests := tc != nil && (tc.Permissions.AllowShellCommands || os.Getenv("SARINA_VERIFY_TESTS") == "on" || config.AccessTierFromEnv(nil) >= 2)
	// Without that trust her tests still run, but in a sandbox (bubblewrap): a copy of the project, no network, everything else read-only.
	// Otherwise she could not know whether tests she wrote pass until she had earned the tier that needs passing code (eval
	// code-add-function, 2026-10-26: a correct function, a failing test of hers, and she never learned it).
	sandbox := !tests && os.Getenv("SARINA_SANDBOX_TESTS") != "off" && haveBwrap()
	tests = tests || sandbox
	var steps [][]string
	var label string
	switch ext {
	case ".go":
		if root := findUp(dir, "go.mod"); root != "" {
			rel, _ := filepath.Rel(root, dir)
			pkg := "./" + filepath.ToSlash(rel)
			if rel == "." {
				pkg = "./..."
			}
			steps = append(steps, []string{"go", "vet", pkg})
			label = "go vet " + pkg
			if tests {
				steps = append(steps, []string{"go", "test", "-count=1", "-timeout", "60s", pkg})
				label += " and go test " + pkg
			}
			dir = root
		} else {
			steps = append(steps, []string{"gofmt", "-l", "-e", path})
			label = "gofmt -e (no go.mod found, so no build)"
		}
	case ".py":
		steps = append(steps, []string{"python3", "-m", "py_compile", path})
		label = "python3 -m py_compile"
		if tests && (strings.HasPrefix(filepath.Base(path), "test_") || strings.HasSuffix(path, "_test.py")) {
			steps = append(steps, []string{"python3", "-m", "unittest", "-q", path})
			label += " and unittest"
		}
	case ".sh", ".bash":
		steps = append(steps, []string{"bash", "-n", path})
		label = "bash -n"
		if _, err := exec.LookPath("shellcheck"); err == nil {
			steps = append(steps, []string{"shellcheck", "-S", "warning", path})
			label += " and shellcheck"
		}
	case ".js", ".mjs":
		steps = append(steps, []string{"node", "--check", path})
		label = "node --check"
	case ".rs":
		if root := findUp(dir, "Cargo.toml"); root != "" {
			if data, err := os.ReadFile(filepath.Join(root, "Cargo.toml")); err == nil && cargoManifestRE.Match(data) {
				steps = append(steps, []string{"cargo", "check", "--offline", "--quiet"})
				label = "cargo check --offline"
				if tests {
					steps = append(steps, []string{"cargo", "test", "--offline", "--quiet"})
					label += " and cargo test"
				}
				dir = root
			}
		}
	default:
		return "", md
	}
	if len(steps) == 0 {
		return "", md
	}
	ctx, cancel := context.WithTimeout(ctx, verifyTimeout)
	defer cancel()
	var out strings.Builder
	for i, step := range steps {
		if _, err := exec.LookPath(step[0]); err != nil {
			return fmt.Sprintf("\nCheck skipped: %s is not installed, so the change was not verified.", step[0]), md
		}
		runDir, argv := dir, step
		if sandbox && i > 0 && step[0] != "shellcheck" {
			sd, sargv, cleanup, why := sandboxStep(dir, step)
			if why != "" {
				md["verify"] = "ok"
				return fmt.Sprintf("\nCheck passed: the build. Tests were not run (%s): do not say the tests pass.", why), md
			}
			defer cleanup()
			runDir, argv = sd, sargv
		}
		cmd := exec.CommandContext(ctx, argv[0], argv[1:]...)
		cmd.Dir = runDir
		cmd.Env = append(childEnv(tc), "GOPROXY=off", "GOFLAGS=-mod=mod", "GOTOOLCHAIN=local", "CARGO_NET_OFFLINE=true", "NO_COLOR=1")
		var buf bytes.Buffer
		cmd.Stdout, cmd.Stderr = &buf, &buf
		err := cmd.Run()
		text := strings.TrimSpace(strings.ToValidUTF8(buf.String(), "�"))
		if step[0] == "gofmt" && err == nil && text != "" {
			err = fmt.Errorf("syntax errors") // gofmt -l -e prints the problems and exits 0 for some of them
		}
		if err != nil {
			if ctx.Err() != nil {
				text = "the check ran out of time (" + verifyTimeout.String() + ")"
			}
			if text == "" {
				text = err.Error()
			}
			md["verify"], md["verify_output"] = "failed", clipText(text, 3000)
			what := "build"
			if i > 0 {
				what = "tests"
			}
			return fmt.Sprintf("\nCHECK FAILED (%s, %s):\n%s%s\nFix this before you answer; do not say the code works.", what, strings.Join(step, " "), clipText(text, 3000), around(text, dir, path)), md
		}
		if i > 0 {
			md["verify_tests"] = true
		}
		if text != "" {
			out.WriteString(clipText(text, 1200))
		}
	}
	md["verify"] = "ok"
	note := "\nCheck passed: " + label + "."
	if sandbox && md["verify_tests"] == true {
		note += " (The tests ran in a sandbox: a copy of the project, no network.)"
	}
	if !tests && (ext == ".go" || ext == ".rs" || ext == ".py") {
		note += " Tests were not run (shell commands are off in this session): do not say the tests pass."
	}
	if o := strings.TrimSpace(out.String()); o != "" {
		note += "\n" + o
	}
	return note, md
}

// findUp returns the nearest directory at or above dir holding name, or "".
func findUp(dir, name string) string {
	for d := dir; ; d = filepath.Dir(d) {
		if _, err := os.Stat(filepath.Join(d, name)); err == nil {
			return d
		}
		if filepath.Dir(d) == d {
			return ""
		}
	}
}

func clipText(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + " [...]"
}

func haveBwrap() bool {
	_, err := exec.LookPath("bwrap")
	return err == nil
}

const sandboxMaxBytes = 200 << 20

// sandboxStep copies the project at dir to a temporary folder and returns the folder and step to run there inside bubblewrap: the
// whole system read-only, /tmp empty, the copy writable, no network, its own process space, gone with its parent. Go gets a build cache
// of its own (~/.cache/sarina/sandbox-go) so it does not compile the standard library every time and never writes to the real one.
func sandboxStep(dir string, step []string) (string, []string, func(), string) {
	size := int64(0)
	_ = filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() && (d.Name() == ".git" || d.Name() == "node_modules" || d.Name() == "target") {
			return filepath.SkipDir
		}
		if info, e := d.Info(); e == nil && !d.IsDir() {
			size += info.Size()
		}
		if size > sandboxMaxBytes {
			return filepath.SkipAll
		}
		return nil
	})
	if size > sandboxMaxBytes {
		return "", nil, nil, "the project is too large to copy into a sandbox"
	}
	tmp, err := os.MkdirTemp("", "sarina-sandbox-")
	if err != nil {
		return "", nil, nil, "no sandbox folder: " + err.Error()
	}
	cleanup := func() { _ = os.RemoveAll(tmp) }
	copyDir := filepath.Join(tmp, "project")
	if out, err := exec.Command("cp", "-a", dir, copyDir).CombinedOutput(); err != nil {
		cleanup()
		return "", nil, nil, "copying the project failed: " + strings.TrimSpace(string(out))
	}
	home, _ := os.UserHomeDir()
	gocache := filepath.Join(home, ".cache", "sarina", "sandbox-go")
	_ = os.MkdirAll(filepath.Join(gocache, "build"), 0o700)
	_ = os.MkdirAll(filepath.Join(gocache, "mod"), 0o700)
	args := make([]string, len(step))
	for i, a := range step {
		args[i] = strings.Replace(a, dir, copyDir, 1)
	}
	bw := []string{"bwrap", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp",
		"--bind", tmp, tmp, "--bind", gocache, gocache, "--unshare-net", "--unshare-pid", "--die-with-parent", "--chdir", copyDir,
		"env", "GOCACHE=" + filepath.Join(gocache, "build"), "GOMODCACHE=" + filepath.Join(gocache, "mod"), "HOME=" + tmp}
	return copyDir, append(bw, args...), cleanup, ""
}

var errAtRE = regexp.MustCompile(`(?m)(?:^|[\s"(])((?:\.{0,2}/)?[\w./-]+\.(?:go|py|rs|sh|js|mjs)):(\d+)(?::\d+)?|File "([^"]+)", line (\d+)`)

// around shows the lines of the file the first error points at, as they are now: a failed edit is fixed by seeing what the file looks
// like after it (eval code-add-function, 2026-10-26: an edit left a function's body orphaned, the check said "line 37: expected
// declaration, found 'if'", and the same edit was sent three more times without the file being looked at).
func around(out, dir, edited string) string {
	m := errAtRE.FindStringSubmatch(out)
	if m == nil {
		return ""
	}
	name, num := m[1], m[2]
	if name == "" {
		name, num = m[3], m[4]
	}
	line, _ := strconv.Atoi(num)
	p := name
	if !filepath.IsAbs(p) {
		p = filepath.Join(dir, p)
		if _, err := os.Stat(p); err != nil {
			p = filepath.Join(filepath.Dir(edited), name)
		}
	}
	text, err := readLines(p, max(1, line-5), line+3)
	if err != nil || text == "" || line <= 0 {
		return ""
	}
	return fmt.Sprintf("\n%s as it is now, around line %d (read it before you edit again; an edit's old_text must match these lines):\n%s", filepath.Base(p), line, text)
}
