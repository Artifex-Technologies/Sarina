package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// checkStandIn stands in for every program the checks run (go, gofmt, python3, bash, cargo, shellcheck, cp, bwrap), in
// bash's builtins only (the first line names the bash to run it): it prints its name, arguments, folder and the
// variables the checks set, and plays the failure STANDIN_MODE asks for.
const checkStandIn = `#!BASH
name=${0##*/}
# the long output first, with no path before it: a cut that counts characters must not depend on the folder's name
if [ "${STANDIN_MODE-}" = big ]; then printf '%03000000d\nthe end\n' 7; exit 1; fi
if [ "${STANDIN_MODE-}" = long ]; then printf '%01500d\n' 5; exit 0; fi
printf '[%s' "$name"; for a in "$@"; do printf ' %s' "$a"; done
printf '] in %s; env %s %s %s %s %s %s\n' "$PWD" "${GOPROXY-}" "${GOFLAGS-}" "${GOTOOLCHAIN-}" "${CARGO_NET_OFFLINE-}" "${NO_COLOR-}" "${MY_TOKEN-none}"
case "${STANDIN_MODE-}:$name" in
  build:go|build:gofmt) printf 'calc.go:4:2: expected declaration, found %s\n' "'if'"; exit 1;;
  build:python3) if [ "$3" = py_compile ]; then printf '  File "slug.py", line 2\n    def x(\n         ^\nSyntaxError: invalid syntax\n'; exit 1; fi;;
  build:bash) printf '%s: line 3: syntax error near unexpected token fi\n' "$2"; exit 2;;
  build:cargo) printf 'error: it does not build\n'; exit 101;;
  gotest:go) if [ "$1" = test ]; then printf -- '--- FAIL: TestAdd (0.00s)\n    calc_test.go:6: Add(2, 3) = 6, want 5\nFAIL\nFAIL\texample/calc\t0.001s\n'; exit 1; fi;;
  gopanic:go) if [ "$1" = test ]; then printf -- '--- FAIL: TestAdd (0.00s)\npanic: runtime error: index out of range [3] with length 3 [recovered]\n\ngoroutine 7 [running]:\nexample/calc.Add(...)\n\t%s/calc.go:4 +0x1d\nexample/calc.TestAdd(0xc000)\n\t%s/calc_test.go:5 +0x18\n' "$PWD" "$PWD"; exit 2; fi;;
  gologs:go) if [ "$1" = test ]; then printf -- '--- FAIL: TestAdd (0.00s)\n    calc_test.go:1: first\n    calc_test.go:5: second\nFAIL\n'; exit 1; fi;;
  gobuildfail:go) if [ "$1" = test ]; then printf 'calc.go:9:1: undefined: Sub\nFAIL\texample/calc [build failed]\n'; exit 1; fi;;
  pytest:python3) if [ "$2" = unittest ]; then printf 'F\n======================================================================\nFAIL: test_slug (test_slug.SlugTest)\n----------------------------------------------------------------------\nTraceback (most recent call last):\n  File "%s/test_slug.py", line 5, in test_slug\n    self.assertEqual(slugify("A B"), "a-b")\n  File "/usr/lib/python3.12/unittest/case.py", line 10, in assertEqual\n    raise self.failureException(msg)\nAssertionError: %s != %s\n' "$PWD" "'a b'" "'a-b'"; exit 1; fi;;
  quiet:*) exit 3;;
  *:cp) printf 'cp: cannot copy\n'; exit 1;;
esac
exit 0
`

// checkTree is the project the checks run on ([path, content]; the module example/calc is the workspace).
func checkTree() [][2]string {
	return [][2]string{
		{"ws/go.mod", "module example/calc\n\ngo 1.22\n"},
		{"ws/calc.go", "package calc\n\n// Add adds.\nfunc Add(a, b int) int {\n\treturn a + b + 1\n}\n"},
		{"ws/calc_test.go", "package calc\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tif got := Add(2, 3); got != 5 {\n\t\tt.Errorf(\"Add(2, 3) = %d, want 5\", got)\n\t}\n}\n"},
		{"ws/sub/util.go", "package sub\n"},
		{"ws/py/slug.py", "def slugify(s):\n    return s.lower()\n\n\ndef other():\n    pass\n"},
		{"ws/py/test_slug.py", "import unittest\nfrom slug import slugify\n\nclass SlugTest(unittest.TestCase):\n    def test_slug(self):\n        self.assertEqual(slugify(\"A B\"), \"a-b\")\n"},
		{"ws/sh/ok.sh", "#!/bin/bash\necho ok\n"},
		{"ws/sh/glob.sh", "#!/bin/bash\n# clean up\nrm -f \"$1/*.tmp\"\n"},
		{"ws/js/app.js", "console.log(1)\n"},
		{"ws/rs/Cargo.toml", "[package]\nname = \"x\"\n"},
		{"ws/rs/src/lib.rs", "pub fn x() {}\n"},
		{"ws/rs2/Cargo.toml", "[workspace]\n"},
		{"ws/rs2/src/lib.rs", "pub fn y() {}\n"},
		{"ws/notes.txt", "notes\n"},
		{"ws/emptydir/.keep", ""},
		{"ws/onlygo/go.mod", "module example/onlygo\n"},
		{"ws/onlygo/inner/a.go", "package inner\n"},
		{"ws/nomod/README.md", "x\n"}, {"ws/emptymod/go.mod", "module example/empty\n"},
		{"nogo/x.go", "package x\n"},
	}
}

var checkHideRE = regexp.MustCompile(`sarina-sandbox-[0-9]+`)

// TestParityChecks records the checks after a code change and check_code (testdata/parity/checks.json): verifyMode over
// the settings, the bubblewrap command line (sandboxArgs, as data: nothing is copied or run), verifyRun for each language,
// failure and mode against the stand-in, check_code, and write_file/edit_file on code files. The tree's folder is
// written "$BASE".
func TestParityChecks(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash")
	}
	if r, err := filepath.EvalSymlinks(bash); err == nil {
		bash = r
	}
	var cases []parity.Case
	// verifyMode
	for _, shell := range []bool{false, true} {
		for _, vt := range []string{"", "on"} {
			for _, sb := range []string{"", "off"} {
				for _, tier := range []string{"", "1", "2", "3"} {
					for _, bw := range []bool{false, true} {
						env := map[string]string{"SARINA_VERIFY_TESTS": vt, "SARINA_SANDBOX_TESTS": sb, "SARINA_ACCESS_TIER": tier}
						tests, trusted, sandbox := verifyMode(&Context{Permissions: config.Permissions{AllowShellCommands: shell}}, func(k string) string { return env[k] }, bw)
						cases = append(cases, parity.Case{Fn: "verifyMode", In: []any{shell, vt, sb, tier, bw}, Out: []bool{tests, trusted, sandbox}})
					}
				}
			}
		}
	}
	// sandboxArgs
	for _, c := range []struct {
		path   string
		exists []string
		step   []string
	}{
		{"", nil, []string{"go", "test", "-count=1", "/w/p/./..."}},
		{"/usr/bin:/bin", []string{"/h/.cargo", "/h/go", "/h/.local/lib"}, []string{"cargo", "test", "--offline", "--quiet"}},
		{"/x", []string{"/h/go"}, []string{"python3", "-m", "unittest", "-q", "/w/p/test_a.py", "/w/p/again/w/p"}},
	} {
		exists := func(p string) bool {
			for _, e := range c.exists {
				if e == p {
					return true
				}
			}
			return false
		}
		got := sandboxArgs("/w/p", "/t/x/project", "/t/x", "/h", "/h/.cache/sarina/sandbox-go", c.path, c.step, exists)
		cases = append(cases, parity.Case{Fn: "sandboxArgs", In: []any{c.path, c.exists, c.step}, Out: got})
	}

	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	tree := checkTree()
	treeOut := make([]any, len(tree))
	for i, e := range tree {
		treeOut[i] = []any{e[0], e[1]}
		p := filepath.Join(top, e[0])
		os.MkdirAll(filepath.Dir(p), 0o755)
		if err := os.WriteFile(p, []byte(e[1]), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	cases = append(cases, parity.Case{Fn: "tree", In: treeOut}, parity.Case{Fn: "standin", In: checkStandIn})
	bin, binbw := filepath.Join(top, "bin"), filepath.Join(top, "binbw")
	script := []byte(strings.Replace(checkStandIn, "BASH", bash, 1))
	for _, d := range []string{bin, binbw, filepath.Join(top, "home"), filepath.Join(top, "tmp")} {
		os.MkdirAll(d, 0o755)
	}
	for _, name := range []string{"go", "gofmt", "python3", "bash", "cargo", "shellcheck", "cp", "bwrap"} {
		for _, d := range []string{bin, binbw} {
			if name == "bwrap" && d == bin {
				continue
			}
			if err := os.WriteFile(filepath.Join(d, name), script, 0o755); err != nil {
				t.Fatal(err)
			}
		}
	}
	t.Chdir(top)
	// what a child inherits unfiltered (the sandbox copy runs with the whole environment): the same in every run
	for _, k := range []string{"GOPROXY", "GOFLAGS", "GOTOOLCHAIN", "CARGO_NET_OFFLINE", "NO_COLOR", "MY_TOKEN", "STANDIN_MODE"} {
		if v, ok := os.LookupEnv(k); ok {
			os.Unsetenv(k)
			t.Cleanup(func() { os.Setenv(k, v) })
		}
	}
	t.Setenv("HOME", filepath.Join(top, "home"))
	t.Setenv("TMPDIR", filepath.Join(top, "tmp"))
	t.Setenv("SARINA_PRIVATE_DIRS", "")
	t.Setenv("SARINA_VERIFY_TESTS", "")
	t.Setenv("SARINA_ACCESS_TIER", "")
	t.Setenv("SARINA_SANDBOX_TESTS", "")
	hide := func(s string) string {
		return checkHideRE.ReplaceAllString(strings.ReplaceAll(s, top, "$BASE"), "sarina-sandbox-N")
	}
	ws := filepath.Join(top, "ws")
	mode := ""
	newTC := func(shell bool) *Context {
		return &Context{Root: ws, MaxOutputChars: 4000, Permissions: config.Permissions{AllowFileWrite: true, AllowShellCommands: shell},
			Environ: func() []string { return []string{"PATH=" + os.Getenv("PATH"), "MY_TOKEN=x", "STANDIN_MODE=" + mode} }}
	}
	// verifyRun: [file, mode, tests, trusted, sandbox]; one context for the whole list, so a failed Go test is run first next time
	runs := [][5]string{
		{"ws/calc.go", "", "1", "1", ""}, {"ws/calc.go", "", "", "", ""}, {"ws/calc.go", "build", "", "", ""}, {"ws/calc.go", "build", "1", "1", ""},
		{"ws/calc.go", "gotest", "1", "1", ""}, {"ws/calc.go", "", "1", "1", ""}, {"ws/calc.go", "gopanic", "1", "1", ""},
		{"ws/calc.go", "gobuildfail", "1", "1", ""}, {"ws/calc.go", "quiet", "1", "1", ""}, {"ws/calc.go", "gologs", "1", "1", ""}, {"ws/calc.go", "long", "", "", ""}, {"ws/calc.go", "big", "1", "1", ""},
		{"ws/calc_test.go", "gotest", "1", "1", ""}, {"ws/sub/util.go", "", "1", "1", ""}, {"nogo/x.go", "", "", "", ""}, {"nogo/x.go", "build", "", "", ""},
		{"ws/py/slug.py", "", "", "", ""}, {"ws/py/slug.py", "", "1", "1", ""}, {"ws/py/slug.py", "build", "1", "1", ""},
		{"ws/py/slug.py", "pytest", "1", "1", ""}, {"ws/py/test_slug.py", "pytest", "1", "1", ""},
		{"ws/sh/ok.sh", "", "", "", ""}, {"ws/sh/ok.sh", "build", "", "", ""}, {"ws/sh/glob.sh", "", "", "", ""}, {"ws/js/app.js", "", "", "", ""},
		{"ws/rs/src/lib.rs", "", "", "", ""}, {"ws/rs/src/lib.rs", "", "1", "1", ""}, {"ws/rs/src/lib.rs", "build", "1", "1", ""},
		{"ws/rs2/src/lib.rs", "", "1", "1", ""}, {"ws/notes.txt", "", "1", "1", ""}, {"ws/CALC.GO", "", "", "", ""},
		{"ws/calc.go", "", "1", "", "1"}, {"ws/calc.go", "gotest", "1", "", "1"}, {"ws/py/slug.py", "", "1", "", "1"},
		{"ws/rs/src/lib.rs", "", "", "", "1"}, {"ws/sh/ok.sh", "", "1", "", "1"},
	}
	tc := newTC(true)
	var runOut []any
	for _, r := range runs {
		mode = r[1]
		if r[4] != "" {
			t.Setenv("PATH", binbw)
		} else {
			t.Setenv("PATH", bin)
		}
		note, md := verifyRun(context.Background(), filepath.Join(top, r[0]), tc, r[2] != "", r[3] != "", r[4] != "", "the reason given")
		mdj, _ := json.Marshal(md)
		runOut = append(runOut, []any{parity.Text(hide(note)), hide(string(mdj))})
	}
	cases = append(cases, parity.Case{Fn: "verifyRun", In: runs, Out: runOut})
	// check_code and the file tools on code files: [tool, arguments, mode, shell, bwrap]
	calls := [][5]string{
		{"check_code", `{"path":"calc.go"}`, "", "", ""}, {"check_code", `{"path":"calc.go"}`, "gotest", "", ""},
		{"check_code", `{"path":"."}`, "", "", ""}, {"check_code", `{"path":"py"}`, "", "", ""}, {"check_code", `{"path":"sh"}`, "build", "", ""},
		{"check_code", `{"path":"onlygo"}`, "", "", ""}, {"check_code", `{"path":"nomod"}`, "", "", ""}, {"check_code", `{"path":"emptymod"}`, "", "", ""}, {"check_code", `{"path":"emptydir"}`, "", "", ""},
		{"check_code", `{"path":"nothere.go"}`, "", "", ""}, {"check_code", `{"path":"notes.txt"}`, "", "", ""}, {"check_code", `{"path":"rs2"}`, "", "", ""},
		{"check_code", `{"path":"../nogo/x.go"}`, "", "", ""}, {"check_code", `{"path":"calc.go"}`, "", "", "1"},
		{"check_code", `{"path":"calc.go"}`, "gotest", "", "1"},
		{"write_file", `{"path":"py/test_new.py","content":"import unittest\n"}`, "pytest", "1", ""},
		{"write_file", `{"path":"py/new.py","content":"x = 1\n"}`, "", "1", ""},
		{"write_file", `{"path":"py/test_new.py","content":"import unittest\nimport new\n"}`, "pytest", "1", ""},
		{"read_file", `{"path":"calc.go"}`, "", "1", ""}, {"edit_file", `{"path":"calc.go","old_text":"+ 1","new_text":"+ 0"}`, "gotest", "1", ""},
		{"edit_file", `{"path":"calc.go","old_text":"+ 0","new_text":"+ 2"}`, "", "", ""},
	}
	reg := NewRegistry(CheckCode(), WriteFile(), EditFile(), ReadFile())
	var callOut []any
	tcs := map[bool]*Context{true: newTC(true), false: newTC(false)}
	for i, c := range calls {
		mode = c[2]
		if c[4] != "" {
			t.Setenv("PATH", binbw)
		} else {
			t.Setenv("PATH", bin)
		}
		var args map[string]any
		if err := json.Unmarshal([]byte(c[1]), &args); err != nil {
			t.Fatal(err)
		}
		tcx := tcs[c[3] != ""]
		tcx.Step = i + 1
		r := reg.Execute(context.Background(), c[0], args, tcx)
		mdj, _ := json.Marshal(r.Metadata)
		callOut = append(callOut, []any{r.OK, parity.Text(hide(r.Content)), hide(string(mdj))})
	}
	cases = append(cases, parity.Case{Fn: "calls", In: calls, Out: callOut})
	var files []any
	for _, f := range []string{"ws/calc.go", "ws/py/test_new.py", "ws/py/new.py"} {
		data, _ := os.ReadFile(filepath.Join(top, f))
		files = append(files, []any{f, fmt.Sprintf("%q", data)})
	}
	cases = append(cases, parity.Case{Fn: "files", Out: files})
	parity.Fixture(t, "checks", cases)
}
