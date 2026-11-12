package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

// When her tests fail, the check's answer starts with one computed line that says which test failed, where, what it wanted, and
// where the function it calls is defined, followed by that function's lines: a failing test should point her at the code, not
// leave her to read a traceback (plan Day 4; eval code-python-fix and code-fix-failing-test, where she edited the test file or
// guessed at the function). Everything here reads text the check printed and files of the project; nothing is run.

var (
	goFailRE     = regexp.MustCompile(`(?m)^\s*--- FAIL: (\S+)`)
	goAssertRE   = regexp.MustCompile(`(?m)^\s+([\w.-]+_test\.go):(\d+): (.*)$`)
	goPanicRE    = regexp.MustCompile(`(?m)^panic: (.*?)(?: \[recovered[^\]]*\])?$`)
	goFrameRE    = regexp.MustCompile(`(?m)^(\S+)\(.*\)\n\t(\S+\.go):(\d+)`)
	pyFrameRE    = regexp.MustCompile(`(?m)^[ \t]*File "([^"]+)", line (\d+)(?:, in (\S+))?\n(?:[ \t]+(.*)\n)?`)
	pyTestRE     = regexp.MustCompile(`(?m)^(?:FAIL|ERROR): (\S+) \(([\w.]+)\)`)
	pyErrorRE    = regexp.MustCompile(`(?m)^((?:\w+\.)*\w*(?:Error|Exception|Exit|Interrupt)\b.*)$`)
	callNameRE   = regexp.MustCompile(`\b([A-Za-z_]\w*)\s*\(`)
	goBuildBadRE = regexp.MustCompile(`\[(?:build|setup) failed\]`)
	goFailCallRE = regexp.MustCompile(`\.(?:Error|Errorf|Fatal|Fatalf|Fail|FailNow)\(`)
)

// localise returns the computed line and the lines of the function under test (either may be ""). out is the test run's output,
// pkgDir the folder of the package or the tests.
func localise(out, pkgDir string) (summary, lines string) {
	if strings.Contains(out, "Traceback (most recent call last)") || pyTestRE.MatchString(out) {
		return localisePython(out, pkgDir)
	}
	return localiseGo(out, pkgDir)
}

func localiseGo(out, pkgDir string) (string, string) {
	test := ""
	if m := goFailRE.FindStringSubmatch(out); m != nil {
		test = m[1]
	}
	if m := goPanicRE.FindStringSubmatch(out); m != nil {
		// the innermost frame in her own code (the first one that is neither the runtime, the testing package nor a test file)
		for _, f := range goFrameRE.FindAllStringSubmatch(out, -1) {
			fn, file, num := f[1], f[2], f[3]
			if strings.Contains(file, "/src/runtime/") || strings.Contains(file, "/src/testing/") || strings.HasSuffix(file, "_test.go") {
				if test == "" && strings.HasSuffix(file, "_test.go") {
					test = lastDot(fn)
				}
				continue
			}
			p := projectFile(file, pkgDir)
			line, _ := strconv.Atoi(num)
			s := fmt.Sprintf("%s panicked: %s, in %s at %s:%d.", orWord(test, "A test"), m[1], lastDot(fn), filepath.Base(file), line)
			text, _ := readLines(p, max(1, line-3), line+3)
			return s, text
		}
		return fmt.Sprintf("%s panicked: %s.", orWord(test, "A test"), m[1]), ""
	}
	// the line that failed the test: a t.Error/t.Fatal call (t.Log prints the same way), else the last one printed
	all := goAssertRE.FindAllStringSubmatch(out, -1)
	var m []string
	for _, c := range all {
		n, _ := strconv.Atoi(c[2])
		if src, err := readLines(filepath.Join(pkgDir, c[1]), n, n); err == nil && goFailCallRE.MatchString(src) {
			m = c
			break
		}
	}
	if m == nil && len(all) > 0 {
		m = all[len(all)-1]
	}
	if m == nil {
		if test != "" {
			return test + " failed.", ""
		}
		return "", ""
	}
	file, num, msg := m[1], m[2], strings.TrimSpace(m[3])
	line, _ := strconv.Atoi(num)
	s := fmt.Sprintf("%s in %s:%d failed: %s", orWord(test, "A test"), file, line, clipText(msg, 200))
	names := callsBefore(filepath.Join(pkgDir, file), line, "func ")
	names = append(names, callNames(msg)...)
	if name, at, text := findDef(pkgDir, names, ".go"); name != "" {
		return s + fmt.Sprintf("; %s is at %s.", name, at), text
	}
	return s, ""
}

func localisePython(out, pkgDir string) (string, string) {
	block := out
	// several failures: the first one
	if i := strings.Index(out, "Traceback (most recent call last)"); i >= 0 {
		block = out[i:]
		if j := strings.Index(block[1:], "Traceback (most recent call last)"); j >= 0 {
			block = block[:j+1]
		}
	}
	test := ""
	if m := pyTestRE.FindStringSubmatch(out); m != nil {
		test = m[1] + " (" + m[2] + ")"
	}
	errLine := ""
	if all := pyErrorRE.FindAllString(block, -1); len(all) > 0 {
		errLine = strings.TrimSpace(all[len(all)-1])
	}
	// the innermost frame in the project: the last one that is not Python's own library
	var file, fn, src string
	line := 0
	for _, f := range pyFrameRE.FindAllStringSubmatch(block, -1) {
		if strings.Contains(f[1], "/lib/python") || strings.HasPrefix(f[1], "<") || strings.Contains(f[1], "site-packages") {
			continue
		}
		file, fn, src = f[1], f[3], strings.TrimSpace(f[4])
		line, _ = strconv.Atoi(f[2])
	}
	if file == "" {
		if test != "" || errLine != "" {
			return strings.TrimSpace(orWord(test, "A test") + " failed: " + errLine), ""
		}
		return "", ""
	}
	base := filepath.Base(file)
	s := fmt.Sprintf("%s failed at %s:%d", orWord(test, "A test"), base, line)
	if fn != "" && fn != "<module>" {
		s += " (in " + fn + ")"
	}
	if errLine != "" {
		s += ": " + clipText(errLine, 200)
	}
	if !isPyTestFile(base) {
		// the error is raised in her code itself: that is the place to look
		text, _ := readLines(projectFile(file, pkgDir), max(1, line-3), line+3)
		return s + ".", text
	}
	names := callNames(src)
	names = append(names, callsBefore(projectFile(file, pkgDir), line, "def ")...)
	if name, at, text := findDef(pkgDir, names, ".py"); name != "" {
		return s + fmt.Sprintf("; %s is at %s.", name, at), text
	}
	return s + ".", ""
}

// callNames lists the names called in a line of code, nearest first; assertion and formatting helpers are left to findDef, which
// only finds what the project itself defines.
func callNames(src string) []string {
	var out []string
	for _, m := range callNameRE.FindAllStringSubmatch(src, -1) {
		out = append(out, m[1])
	}
	return out
}

// callsBefore lists the names called on the failing line of a test and the lines above it, up to the start of the test (a line
// starting with stop): "got := Add(2, 3)" is often a line above the "if got != want".
func callsBefore(path string, line int, stop string) []string {
	text, err := readLines(path, max(1, line-15), line)
	if err != nil {
		return nil
	}
	ls := strings.Split(text, "\n")
	var out []string
	for i := len(ls) - 1; i >= 0; i-- {
		code := ls[i]
		if k := strings.Index(code, ": "); k >= 0 {
			code = code[k+2:]
		}
		out = append(out, callNames(code)...)
		if strings.HasPrefix(strings.TrimSpace(code), stop) {
			break
		}
	}
	return out
}

// findDef finds the first of names that the project's non-test files with the extension define (func Name / func (r T) Name, or
// def name), and returns it, "file:line", and the definition's first lines.
func findDef(dir string, names []string, ext string) (string, string, string) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return "", "", ""
	}
	seen := map[string]bool{}
	for _, name := range names {
		if seen[name] || name == "func" || name == "def" {
			continue
		}
		seen[name] = true
		var re *regexp.Regexp
		if ext == ".go" {
			re = regexp.MustCompile(`^func (?:\([^)]*\) )?` + regexp.QuoteMeta(name) + `\s*[\[(]`)
		} else {
			re = regexp.MustCompile(`^\s*def ` + regexp.QuoteMeta(name) + `\s*\(`)
		}
		for _, e := range entries {
			fn := e.Name()
			if e.IsDir() || filepath.Ext(fn) != ext || strings.HasSuffix(fn, "_test.go") || isPyTestFile(fn) {
				continue
			}
			data, err := readBounded(filepath.Join(dir, fn)) // a planted file of gigabytes is skipped, not read
			if err != nil {
				continue
			}
			for i, l := range strings.Split(string(data), "\n") {
				if re.MatchString(l) {
					text, _ := readLines(filepath.Join(dir, fn), i+1, i+12)
					return name, fmt.Sprintf("%s:%d", fn, i+1), defBody(text)
				}
			}
		}
	}
	return "", "", ""
}

// defBody cuts numbered lines after the definition's end (a "}" or a new top-level def at the start of a line).
func defBody(text string) string {
	ls := strings.Split(text, "\n")
	for i := 1; i < len(ls); i++ {
		code := ls[i]
		if k := strings.Index(code, ": "); k >= 0 {
			code = code[k+2:]
		}
		if code == "}" {
			return strings.Join(ls[:i+1], "\n")
		}
		if strings.HasPrefix(code, "def ") || strings.HasPrefix(code, "func ") || strings.HasPrefix(code, "class ") {
			return strings.Join(ls[:i], "\n")
		}
	}
	return text
}

// projectFile maps a path the check printed to the project: the sandbox ran a copy that is gone by now, so a path that does not
// exist is looked up by its base name in the package folder.
func projectFile(p, pkgDir string) string {
	if !filepath.IsAbs(p) {
		return filepath.Join(pkgDir, p)
	}
	if _, err := os.Stat(p); err == nil {
		return p
	}
	return filepath.Join(pkgDir, filepath.Base(p))
}

func isPyTestFile(name string) bool {
	return strings.HasSuffix(name, ".py") && (strings.HasPrefix(name, "test_") || strings.HasSuffix(name, "_test.py"))
}

func lastDot(s string) string {
	if i := strings.LastIndex(s, "."); i >= 0 {
		return s[i+1:]
	}
	return s
}

func orWord(s, alt string) string {
	if s == "" {
		return alt
	}
	return s
}

// clipLines keeps at most n lines of a check's output: the first and the last half, with the number left out between them.
func clipLines(s string, n int) string {
	ls := strings.Split(s, "\n")
	if len(ls) <= n {
		return s
	}
	h := n / 2
	return strings.Join(ls[:h], "\n") + fmt.Sprintf("\n[... %d lines left out ...]\n", len(ls)-n) + strings.Join(ls[len(ls)-h:], "\n")
}
