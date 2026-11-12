package tools

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// gitStandIn is a stand-in for git, in bash's builtins only (the first line names the bash to run it): it answers the
// config listing from the workspace's .standin-config, rev-parse by whether .standin-head exists, and otherwise prints
// its arguments and the variables git_read sets, shaped by marker files (fail, failquiet, empty, big, bigconfig).
const gitStandIn = `#!BASH
root=
prev=
for a in "$@"; do
  if [ "$prev" = "-C" ]; then root=$a; fi
  prev=$a
done
case " $* " in
  *" config --local --name-only --list "*)
    if [ -f "$root/.standin-bigconfig" ]; then printf '%01100000d\nfilter.x.clean\n' 0; exit 0; fi
    if [ -f "$root/.standin-config" ]; then
      while IFS= read -r l || [ -n "$l" ]; do printf '%s\n' "$l"; done < "$root/.standin-config"
    fi
    exit 0;;
  *" rev-parse "*)
    : > "$root/.standin-ran-revparse"
    if [ -n "$MY_TOKEN" ]; then : > "$root/.standin-revparse-saw-secret"; fi
    [ -f "$root/.standin-head" ]; exit $?;;
esac
if [ -f "$root/.standin-fail" ]; then printf 'fatal: not a git repository\n' >&2; exit 128; fi
if [ -f "$root/.standin-failquiet" ]; then exit 3; fi
if [ -f "$root/.standin-empty" ]; then printf '  \n'; exit 0; fi
if [ -f "$root/.standin-big" ]; then printf '%02000000d' 7; exit 0; fi
printf 'args:'; printf ' [%s]' "$@"; printf '\n'
printf 'env: %s %s %s %s %s %s\n' "$GIT_TERMINAL_PROMPT" "$GIT_OPTIONAL_LOCKS" "$GIT_PAGER" "$PAGER" "${S1-unset}" "${MY_TOKEN-unset}"
printf 'to stderr\n' >&2
printf '\xff end\n'
`

// gitScenarios: [marker files ("name=content"; "" for none)], then calls [arguments as JSON].
func gitScenarios() [][2][]string {
	plain := []string{`{"command":"status"}`, `{"command":"diff"}`, `{"command":"log"}`, `{"command":"show"}`}
	return [][2][]string{
		{{""}, append(plain, `{"command":"show","ref":"abc123"}`, `{"command":"diff","ref":"HEAD~1"}`, `{"command":"show","ref":"-x"}`,
			`{"command":"show","ref":"a b"}`, `{"command":"show","ref":"`+strings.Repeat("a", 101)+`"}`, `{"command":"show","ref":"`+strings.Repeat("a", 100)+`"}`,
			`{"command":"push"}`, `{"command":"status","path":"a.txt"}`, `{"command":"diff","path":"nothere"}`, `{"command":"log","path":"priv"}`,
			`{"command":"status","path":"../out"}`, `{"command":"show","ref":"v1.0^{commit}"}`, `{"command":"show","ref":"é"}`, `{}`)},
		{{".standin-head="}, []string{`{"command":"diff"}`, `{"command":"diff","path":"a.txt"}`, `{"command":"diff","ref":"main"}`}},
		{{".standin-config=core.bare\nfilter.lfs.clean\ninclude.path\nIncludeIf.gitdir:x.path\nFILTER.A.Smudge"}, plain},
		{{".standin-config=core.editor\nfilter.lfs.required\n  include.x  "}, []string{`{"command":"status"}`}},
		{{".standin-config=filter.a.process", ".standin-head="}, []string{`{"command":"diff"}`}},
		{{".standin-bigconfig="}, []string{`{"command":"status"}`}},
		{{".standin-fail="}, plain},
		{{".standin-failquiet="}, []string{`{"command":"log"}`}},
		{{".standin-empty="}, plain},
		{{".standin-empty=", ".standin-head="}, []string{`{"command":"diff"}`, `{"command":"diff","ref":"x"}`}},
		{{".standin-big="}, []string{`{"command":"log"}`}},
	}
}

// TestParityGit records git_read against the stand-in above (testdata/parity/git.json): the arguments and variables it
// gives git, the ref and path checks, the refusal of a repository whose own settings run programs, git's failures and
// empty answers, and the bounds on what is read. The tree's folder is written "$BASE".
func TestParityGit(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash")
	}
	if r, err := filepath.EvalSymlinks(bash); err == nil {
		bash = r
	}
	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	bin := filepath.Join(top, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(strings.Replace(gitStandIn, "BASH", bash, 1)), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)
	hide := func(s string) string { return strings.ReplaceAll(s, top, "$BASE") }
	environ := []string{"PATH=" + bin, "MY_TOKEN=x", "LANG=C"}
	t.Setenv("MY_TOKEN", "x")
	cases := []parity.Case{{Fn: "standin", In: gitStandIn}}
	for n, sc := range gitScenarios() {
		base := filepath.Join(top, "s", string(rune('a'+n)))
		ws := filepath.Join(base, "ws")
		for _, d := range []string{ws, filepath.Join(ws, "priv"), filepath.Join(base, "out"), filepath.Join(base, "home")} {
			if err := os.MkdirAll(d, 0o755); err != nil {
				t.Fatal(err)
			}
		}
		os.WriteFile(filepath.Join(ws, "a.txt"), []byte("a\n"), 0o644)
		for _, m := range sc[0] {
			if m == "" {
				continue
			}
			name, content, _ := strings.Cut(m, "=")
			if err := os.WriteFile(filepath.Join(ws, name), []byte(content), 0o644); err != nil {
				t.Fatal(err)
			}
		}
		t.Setenv("HOME", filepath.Join(base, "home"))
		t.Setenv("SARINA_PRIVATE_DIRS", filepath.Join(ws, "priv"))
		tc := &Context{Root: ws, MaxOutputChars: 1500, Environ: func() []string { return environ }, SessionEnv: map[string]string{"S1": "v1"}}
		reg := NewRegistry(GitRead())
		var out []any
		for _, call := range sc[1] {
			var args map[string]any
			if err := json.Unmarshal([]byte(call), &args); err != nil {
				t.Fatalf("%s: %v", call, err)
			}
			r := reg.Execute(context.Background(), "git_read", args, tc)
			md, _ := json.Marshal(r.Metadata)
			out = append(out, []any{r.OK, parity.Text(hide(r.Content)), string(md)})
		}
		var marks []string
		for _, m := range []string{".standin-ran-revparse", ".standin-revparse-saw-secret"} {
			if _, err := os.Stat(filepath.Join(ws, m)); err == nil {
				marks = append(marks, m)
			}
		}
		cases = append(cases, parity.Case{Fn: "scenario", In: []any{n, sc[0], sc[1]}, Out: []any{out, marks}})
	}
	parity.Fixture(t, "git", cases)
}

// A repository's config listing past the bound counts as risky (a filter after 1 MiB of harmless settings went unseen
// before the bound, and with no bound the whole listing was read), and git's own output is kept to the bound (found by
// the Rust port, 2026-11-12).
func TestGitOutputIsBounded(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash")
	}
	top := t.TempDir()
	if err := os.WriteFile(filepath.Join(top, "git"), []byte(strings.Replace(gitStandIn, "BASH", bash, 1)), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", top)
	os.WriteFile(filepath.Join(top, ".standin-bigconfig"), nil, 0o644)
	if got := GitConfigRisk(t.Context(), top); !strings.Contains(got, "not read whole") {
		t.Fatalf("a config listing past the bound: %q", got)
	}
	b := &boundedBuffer{max: 4}
	for _, p := range []string{"ab", "cde", "f"} {
		if n, err := b.Write([]byte(p)); n != len(p) || err != nil {
			t.Fatalf("Write(%q) = %d, %v", p, n, err)
		}
	}
	if b.buf.String() != "abcd" || !b.over {
		t.Fatalf("%q %v", b.buf.String(), b.over)
	}
}

// A repository whose own settings run programs gets no git at all: diff asked git for HEAD before the settings were
// checked, with the whole environment and no time limit (found by the Rust port, 2026-11-12); and rev-parse, when it
// runs, sees the filtered environment.
func TestGitReadChecksSettingsBeforeRunningGit(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash")
	}
	bin, ws := t.TempDir(), t.TempDir()
	if err := os.WriteFile(filepath.Join(bin, "git"), []byte(strings.Replace(gitStandIn, "BASH", bash, 1)), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)
	t.Setenv("MY_TOKEN", "x")
	os.WriteFile(filepath.Join(ws, ".standin-config"), []byte("filter.a.clean\n"), 0o644)
	reg := NewRegistry(GitRead())
	if r := reg.Execute(t.Context(), "git_read", map[string]any{"command": "diff"}, &Context{Root: ws}); r.OK {
		t.Fatalf("risky settings: %+v", r)
	}
	if _, err := os.Stat(filepath.Join(ws, ".standin-ran-revparse")); err == nil {
		t.Fatal("git ran before the settings were checked")
	}
	os.Remove(filepath.Join(ws, ".standin-config"))
	if r := reg.Execute(t.Context(), "git_read", map[string]any{"command": "diff"}, &Context{Root: ws}); !r.OK {
		t.Fatalf("plain diff: %+v", r)
	}
	if _, err := os.Stat(filepath.Join(ws, ".standin-revparse-saw-secret")); err == nil {
		t.Fatal("rev-parse saw a secret-looking variable")
	}
}
