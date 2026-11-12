package tools

import (
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
// the model's own code, so tests run in a sandbox whenever bubblewrap exists, and on the project itself only when shell commands are
// allowed and there is no sandbox (verifyMode); a build or a syntax check executes nothing of hers and always runs.

const verifyTimeout = 90 * time.Second

var cargoManifestRE = regexp.MustCompile(`(?m)^\[package\]`)

// verifyCode checks the changed file. It returns a line for the tool's answer and the metadata keys verify (ok|failed|skipped),
// verify_tests (true when tests ran and passed) and verify_output.
func verifyCode(ctx context.Context, path string, tc *Context) (string, map[string]any) {
	tests, trusted, sandbox := verifyMode(tc, os.Getenv, haveBwrap())
	return verifyRun(ctx, path, tc, tests, trusted, sandbox, "shell commands are off in this session")
}

// verifyRun runs the checks for path in the given mode; noTests is why tests did not run when tests is false.
func verifyRun(ctx context.Context, path string, tc *Context, tests, trusted, sandbox bool, noTests string) (string, map[string]any) {
	md := map[string]any{"verify": "skipped"}
	ext := strings.ToLower(filepath.Ext(path))
	dir := filepath.Dir(path)
	var steps [][]string
	var label, failKey string
	testFrom := 1 // the steps from this one on run her code (the tests); the ones before only build or check syntax
	vetFallback := []string(nil)
	switch ext {
	case ".go":
		if root := findUp(dir, "go.mod"); root != "" {
			rel, _ := filepath.Rel(root, dir)
			pkg := "./" + filepath.ToSlash(rel)
			if rel == "." {
				pkg = "./..."
			}
			if tests {
				// go test vets the package itself (a subset of go vet) before it runs anything, so the test run is the build check
				// too; a retry runs the test that failed last time first, and the whole package only once that one passes
				failKey = root + "|" + pkg
				st := stateOf(tc)
				st.mu.Lock()
				prev := st.failedTest[failKey]
				st.mu.Unlock()
				if prev != "" {
					steps = append(steps, []string{"go", "test", "-count=1", "-timeout", "60s", "-run", "^" + regexp.QuoteMeta(prev) + "$", pkg})
				}
				steps = append(steps, []string{"go", "test", "-count=1", "-timeout", "60s", pkg})
				label = "go test " + pkg + " (it vets the package too)"
				testFrom, vetFallback = 0, []string{"go", "vet", pkg}
			} else {
				steps = append(steps, []string{"go", "vet", pkg})
				label = "go vet " + pkg
			}
			dir = root
		} else {
			steps = append(steps, []string{"gofmt", "-l", "-e", path})
			label = "gofmt -e (no go.mod found, so no build)"
		}
	case ".py":
		// -I: isolated, so the project's own folder is not on the import path (a planted py_compile.py would run) and PYTHON*
		// variables are ignored
		steps = append(steps, []string{"python3", "-I", "-m", "py_compile", path})
		label = "python3 -m py_compile"
		if tests && (strings.HasPrefix(filepath.Base(path), "test_") || strings.HasSuffix(path, "_test.py")) {
			steps = append(steps, []string{"python3", "-m", "unittest", "-q", path})
			label += " and unittest"
		} else if tests && hasPyTests(dir) {
			// a change to the code runs the folder's tests too, as go test does for a package: slug.py half fixed and its test_slug.py
			// never run, so she never learned it still failed (eval code-python-fix, 2026-10-26)
			steps = append(steps, []string{"python3", "-m", "unittest", "-q"})
			label += " and the folder's unittest tests"
		}
	case ".sh", ".bash":
		if l := shellLint(path); l != "" {
			md["verify"], md["verify_output"] = "failed", l
			return "\nCHECK FAILED (lint): " + l + "\nFix this before you answer; do not say the script works.", md
		}
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
			if data, err := readBounded(filepath.Join(root, "Cargo.toml")); err == nil && cargoManifestRE.Match(data) {
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
		// cargo's build runs the project's own code (build scripts, procedural macros): without trust it goes in the sandbox with
		// the tests, or does not run at all (Go is built so that compiling never runs the module's code)
		builds := step[0] == "cargo"
		if !trusted && !sandbox && builds {
			md["verify"] = "skipped"
			return fmt.Sprintf("\nCheck skipped: %s would run the project's own build code, and there is no sandbox here (bubblewrap): the change was not verified.", strings.Join(step[:2], " ")), md
		}
		if sandbox && (i >= testFrom || builds) && step[0] != "shellcheck" {
			sd, sargv, cleanup, why := sandboxStep(dir, step)
			if why != "" && vetFallback != nil && i == 0 {
				// nothing has checked the build yet: check it here (compiling runs nothing of hers) and say the tests did not run
				return vetOnly(ctx, tc, dir, path, vetFallback, why, md)
			}
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
		buf := &headTail{max: maxCheckOutput}
		cmd.Stdout, cmd.Stderr = buf, buf
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
			// the whole output stays in the metadata; the answer gets a computed first line and at most 40 lines of it
			md["verify"], md["verify_output"] = "failed", clipText(text, 12000)
			what := "build"
			if i >= testFrom && !goBuildBadRE.MatchString(text) {
				what = "tests"
			}
			md["verify_kind"] = what
			head, tail := "", around(text, dir, path)
			if what == "tests" {
				summary, lines := localise(text, filepath.Dir(path))
				if summary != "" {
					head = summary + "\n"
				}
				if lines != "" {
					tail = "\nThe code it points at, as it is now (read it before you edit; an edit's old_text must match these lines):\n" + lines
				}
				if m := goFailRE.FindStringSubmatch(text); m != nil && failKey != "" {
					st := stateOf(tc)
					st.mu.Lock()
					st.failedTest[failKey] = strings.SplitN(m[1], "/", 2)[0]
					st.mu.Unlock()
				}
			}
			advice := "Fix this before you answer; do not say the code works."
			if what == "tests" && ownTestOnOthersCode(stateOf(tc), path) {
				// owner's yes 2026-11-10 (tt-own-test-fails): her new test failing on code she did not write is a finding, not a
				// licence to change that code (tt-code-test: she edited the owner's rule11.py until her own test passed)
				advice = "Your test fails on the code it tests: report it (what fails, and whether the test or the code looks wrong); change that code only if you were asked to fix it."
				md["verify_own_test"] = true
			}
			return fmt.Sprintf("\nCHECK FAILED (%s, %s):\n%s%s%s\n%s", what, strings.Join(step, " "), head, clipText(clipLines(text, 40), 3000), tail, advice), md
		}
		if i >= testFrom {
			md["verify_tests"] = true
		}
		if text != "" {
			out.WriteString(clipText(text, 1200))
		}
	}
	if failKey != "" {
		st := stateOf(tc)
		st.mu.Lock()
		delete(st.failedTest, failKey)
		st.mu.Unlock()
	}
	md["verify"] = "ok"
	note := "\nCheck passed: " + label + "."
	if sandbox && md["verify_tests"] == true {
		note += " (The tests ran in a sandbox: a copy of the project, no network.)"
	}
	if !tests && (ext == ".go" || ext == ".rs" || ext == ".py") {
		note += " Tests were not run (" + noTests + "): do not say the tests pass."
	}
	if o := strings.TrimSpace(out.String()); o != "" {
		note += "\n" + o
	}
	return note, md
}

// verifyMode decides three separate facts about a change's tests. Whether they run at all: always in a sandbox (bubblewrap: a
// copy of the project, no network, everything else read-only) when there is one, so she learns whether tests she wrote pass before
// she has earned the tier that needs passing code (eval code-add-function, 2026-10-26); without a sandbox only when shell commands
// are allowed. Whether they run in the sandbox: whenever bwrap exists and SARINA_SANDBOX_TESTS is not "off". Whether they run on
// the project itself (trusted): only with shell allowed AND no sandbox. SARINA_VERIFY_TESTS=on and tier 2 ask for tests; they never
// take them out of the sandbox (before 2026-11-03 they did, and the code evals ran her fresh tests unsandboxed on the host).
func verifyMode(tc *Context, getenv func(string) string, bwrap bool) (tests, trusted, sandbox bool) {
	if tc == nil {
		return false, false, false
	}
	wanted := tc.Permissions.AllowShellCommands || getenv("SARINA_VERIFY_TESTS") == "on" || config.AccessTierFromEnv(getenv) >= 2
	sandbox = bwrap && getenv("SARINA_SANDBOX_TESTS") != "off"
	trusted = wanted && !sandbox && tc.Permissions.AllowShellCommands
	return trusted || sandbox, trusted, sandbox
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
	cp := exec.Command("cp", "-a", dir, copyDir)
	out := &headTail{max: maxCheckOutput}
	cp.Stdout, cp.Stderr = out, out
	if err := cp.Run(); err != nil {
		cleanup()
		return "", nil, nil, "copying the project failed: " + strings.TrimSpace(out.String())
	}
	home, _ := os.UserHomeDir()
	gocache := filepath.Join(home, ".cache", "sarina", "sandbox-go")
	_ = os.MkdirAll(filepath.Join(gocache, "build"), 0o700)
	_ = os.MkdirAll(filepath.Join(gocache, "mod"), 0o700)
	return copyDir, sandboxArgs(dir, copyDir, tmp, home, gocache, os.Getenv("PATH"), step, dirExists), cleanup, ""
}

// sandboxArgs is the bubblewrap command line for step, run in copyDir (the copy of dir in tmp): the whole system
// read-only, /tmp, /run and the home folder hidden, the toolchains under home put back on a throw-away layer, no network,
// a clean environment.
func sandboxArgs(dir, copyDir, tmp, home, gocache, path string, step []string, exists func(string) bool) []string {
	args := make([]string, len(step))
	for i, a := range step {
		args[i] = strings.Replace(a, dir, copyDir, 1)
	}
	// Everything read-only, and then hidden: /run (the session's D-Bus, the compositor, Sarina's and Heimdall's sockets: a socket
	// can be connected to through a read-only mount) and the home folder (Sarina's key, the vault, ssh keys), with only the
	// toolchains put back, writable on a throw-away layer. All namespaces unshared, no network, a clean environment.
	bw := []string{"bwrap", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp", "--tmpfs", "/run", "--tmpfs", home}
	for _, d := range []string{".cargo", "go", ".local/lib"} {
		if p := filepath.Join(home, d); exists(p) {
			bw = append(bw, "--overlay-src", p, "--tmp-overlay", p)
		}
	}
	if path == "" {
		path = "/usr/local/bin:/usr/bin"
	}
	bw = append(bw, "--bind", tmp, tmp, "--bind", gocache, gocache, "--unshare-all", "--new-session", "--die-with-parent", "--clearenv",
		"--setenv", "PATH", path, "--setenv", "HOME", tmp, "--setenv", "TMPDIR", "/tmp", "--setenv", "LANG", "C.UTF-8",
		"--setenv", "GOCACHE", filepath.Join(gocache, "build"), "--setenv", "GOMODCACHE", filepath.Join(gocache, "mod"),
		"--setenv", "GOPROXY", "off", "--setenv", "GOFLAGS", "-mod=mod", "--setenv", "GOTOOLCHAIN", "local",
		"--setenv", "CARGO_NET_OFFLINE", "true", "--setenv", "NO_COLOR", "1", "--chdir", copyDir, "--")
	return append(bw, args...)
}

// maxCheckOutput bounds what is kept of a check's output at each end: a build or test run printing without end was held
// in memory whole (found by the Rust port, 2026-11-12). The first and the last part are kept (the first failure and the
// summary at the end), with how much was left out between them.
const maxCheckOutput = 1 << 20

// headTail keeps the first and the last max bytes written, and how many were left out between them.
type headTail struct {
	head, tail []byte
	max        int
	dropped    int64
}

func (b *headTail) Write(p []byte) (int, error) {
	n := len(p)
	if room := b.max - len(b.head); room > 0 {
		k := min(room, len(p))
		b.head = append(b.head, p[:k]...)
		p = p[k:]
	}
	b.tail = append(b.tail, p...)
	if len(b.tail) > 2*b.max {
		cut := len(b.tail) - b.max
		b.dropped += int64(cut)
		b.tail = append([]byte(nil), b.tail[cut:]...)
	}
	return n, nil
}

func (b *headTail) String() string {
	tail := b.tail
	dropped := b.dropped
	if len(tail) > b.max {
		dropped += int64(len(tail) - b.max)
		tail = tail[len(tail)-b.max:]
	}
	if dropped == 0 {
		return string(b.head) + string(tail)
	}
	return string(b.head) + fmt.Sprintf("\n[... %d bytes of output left out ...]\n", dropped) + string(tail)
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
	hint := ""
	for _, l := range strings.Split(text, "\n") {
		if strings.HasPrefix(l, fmt.Sprintf("%d: ", line)) && innerQuoteRE.MatchString(l) {
			// "Expected "[1 2]", got %s": the inner quote ends the string, and Go reports "expected ']', found 2" (eval
			// code-go-package, 2026-10-26: the same line rewritten for 400 s)
			hint = "\nLine " + fmt.Sprint(line) + " has a double quote inside a \"...\" string, which ends the string there. Write the inner quote as \\\" (in your edit's JSON that is \\\\\\\"), or use a `backquoted` string, or print the value with %q."
		}
	}
	return fmt.Sprintf("\n%s as it is now, around line %d (read it before you edit again; an edit's old_text must match these lines):\n%s%s", filepath.Base(p), line, text, hint)
}

// innerQuoteRE is a "..." string with a bare " inside, followed by more of the same string: "Expected "[1 2]", got %s".
var innerQuoteRE = regexp.MustCompile(`"[^"\\\n]*[A-Za-z:=] ?"[^",)\n]+"[^"\n]*%[a-z]`)

func hasPyTests(dir string) bool {
	m, _ := filepath.Glob(filepath.Join(dir, "test_*.py"))
	return len(m) > 0
}

var quotedGlobRE = regexp.MustCompile(`"[^"\n]*/\*[^"\n]*"`)

// shellLint finds the shell mistake bash -n cannot: a glob inside double quotes, which is taken literally, so `rm -f "$1/*.tmp"`
// removes nothing (eval code-shell-safe, 2026-10-26; shellcheck catches it too, when it is installed).
func shellLint(path string) string {
	b, err := readBounded(path)
	if err != nil {
		return ""
	}
	for i, l := range strings.Split(string(b), "\n") {
		t := strings.TrimSpace(l)
		if t == "" || strings.HasPrefix(t, "#") || strings.HasPrefix(t, "echo ") || strings.HasPrefix(t, "printf ") {
			continue
		}
		if m := quotedGlobRE.FindString(l); m != "" {
			return fmt.Sprintf("line %d: %s has the * inside double quotes, where the shell does not expand it, so it matches only a file literally named that. Quote the variable and leave the pattern outside, e.g. \"$1\"/*.tmp", i+1, m)
		}
	}
	return ""
}

func dirExists(p string) bool {
	fi, err := os.Stat(p)
	return err == nil && fi.IsDir()
}

// vetOnly is the build check of a Go change whose tests could not be put in the sandbox: go vet, which compiles and runs nothing.
func vetOnly(ctx context.Context, tc *Context, dir, path string, step []string, why string, md map[string]any) (string, map[string]any) {
	cmd := exec.CommandContext(ctx, step[0], step[1:]...)
	cmd.Dir = dir
	cmd.Env = append(childEnv(tc), "GOPROXY=off", "GOFLAGS=-mod=mod", "GOTOOLCHAIN=local", "NO_COLOR=1")
	b := &headTail{max: maxCheckOutput}
	cmd.Stdout, cmd.Stderr = b, b
	err := cmd.Run()
	text := strings.TrimSpace(strings.ToValidUTF8(b.String(), "\uFFFD"))
	if err != nil {
		if text == "" {
			text = err.Error()
		}
		md["verify"], md["verify_output"], md["verify_kind"] = "failed", clipText(text, 12000), "build"
		return fmt.Sprintf("\nCHECK FAILED (build, %s):\n%s%s\nFix this before you answer; do not say the code works.", strings.Join(step, " "), clipText(clipLines(text, 40), 3000), around(text, dir, path)), md
	}
	md["verify"] = "ok"
	return fmt.Sprintf("\nCheck passed: the build (%s). Tests were not run (%s): do not say the tests pass.", strings.Join(step, " "), why), md
}

// testFileRE is a file that holds tests rather than the code they test.
var testFileRE = regexp.MustCompile(`(^test_.*\.py$|_test\.(py|go)$|\.(test|spec)\.(js|ts|mjs)$|^tests?\.rs$)`)

// ownTestOnOthersCode: path is a test file she created in this session, and she has created or edited no other (non-test)
// file in its folder, so a failure is about code she did not write.
func ownTestOnOthersCode(st *fileState, path string) bool {
	if !testFileRE.MatchString(filepath.Base(path)) {
		return false
	}
	st.mu.Lock()
	defer st.mu.Unlock()
	if !st.authored[path] {
		return false
	}
	dir := filepath.Dir(path)
	for p := range st.authored {
		if filepath.Dir(p) == dir && !testFileRE.MatchString(filepath.Base(p)) {
			return false
		}
	}
	for p, n := range st.edits {
		if n > 0 && filepath.Dir(p) == dir && !testFileRE.MatchString(filepath.Base(p)) {
			return false
		}
	}
	return true
}
