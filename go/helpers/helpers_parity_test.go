package helpers

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// fakeScript is a stand-in for the Python interpreter that both languages' tests run, so the helper client is compared
// on the same process: it answers `serve` with the lines in <model dir>/replies, in order ("@big": a 5,000-byte line,
// "@exit": a last error line and exit 3; "@late": its output closed first, the error line 0.3 s later, exit 4), and logs each request to <model dir>/requests.
const fakeScript = `#!/bin/sh
if [ "$1" != "-P" ]; then echo "not the helper: no -P"; exit 2; fi
shift
if [ "$1" != "-m" ] || [ "$2" != "sarina_helpers" ]; then echo "not the helper"; exit 2; fi
shift 2
if [ $# -eq 0 ]; then echo "usage: python -m sarina_helpers {serve|memory-search|voice-chat} ..."; exit 2; fi
if [ "$1" != "serve" ]; then
  printf 'fake-helper'; for a in "$@"; do printf '|%s' "$a"; done; echo
  echo "to stderr" >&2
  exit "${FAKE_EXIT:-0}"
fi
dir="$3"
case "$(basename "$dir")" in
  # bad and silent read the ping first: answering (or ending) before it is written raced with the write (a broken pipe
  # instead of the answer on a busy CI runner, 2026-11-12); the real helper reads its request before it answers
  bad) read -r _; echo '{"ok":false,"error":"no model.onnx in bad"}'; exit 1;;
  noisy) i=0; while [ $i -lt 3000 ]; do echo "warning: unimportant line $i" >&2; i=$((i+1)); done; echo "fatal: noisy" >&2; exit 1;;
  silent) read -r _; exit 0;;
esac
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '%s\n' "$line" >> "$dir/requests"
  r=$(sed -n "${n}p" "$dir/replies")
  if [ "$r" = "@big" ]; then head -c 5000 /dev/zero | tr '\0' x; echo; continue; fi
  if [ "$r" = "@exit" ]; then echo "fatal: gone" >&2; exit 3; fi
  if [ "$r" = "@late" ]; then exec 1>&-; sleep 0.3; echo "fatal: said late" >&2; exit 4; fi
  printf '%s\n' "$r"
done
`

// noModule is an interpreter without the helper package.
const noModule = "#!/bin/sh\necho \"Traceback (most recent call last):\" >&2\necho \"ModuleNotFoundError: No module named 'sarina_helpers'\" >&2\nexit 1\n"

type helperOp struct {
	op        string // classify, group
	text      string
	captures  []map[string]any
	threshold float64
}

// TestParityHelpers records the helper client (testdata/parity/helpers.json) against the shell stand-in: start (ping and
// protocol), classify, group, the helper's errors, damaged and over-long answers, a helper that dies, Close's exit
// status, the requests sent; and Available and Passthrough. {DIR} is the test folder.
func TestParityHelpers(t *testing.T) {
	root := t.TempDir()
	py := filepath.Join(root, "python")
	os.WriteFile(py, []byte(fakeScript), 0o755)
	nomod := filepath.Join(root, "nomodule")
	os.WriteFile(nomod, []byte(noModule), 0o755)
	t.Setenv(PythonEnv, py)
	t.Setenv(PathEnv, "")
	ping := `{"ok":true,"protocol":1,"prototypes":"abc"}`
	ops := []helperOp{{op: "classify", text: "draw <pictures> é"}, {op: "group", captures: []map[string]any{{"name": "a.nfc", "size": 12.5}}, threshold: 0.7},
		{op: "classify", text: "second"}, {op: "classify", text: "third"}, {op: "classify", text: "fourth"}}
	scenarios := []struct {
		dir     string
		replies []string
		maxLine int
	}{
		{"main", []string{ping, `{"ok":true,"label":"Graphics","score":0.9}`, `{"ok":true,"groups":[["a.nfc","b.nfc"]]}`, `{"ok":true,"label":null,"score":0.25}`,
			`{"ok":false,"error":"boom"}`, `not json`}, 0},
		{"types", []string{ping, `{"ok":true,"protocol":"1"}`, `{"ok":true,"label":5}`, `{"ok":true,"groups":{"a":1}}`, ``, `{"ok":true} trailing`}, 0},
		{"big", []string{ping, "@big", `{"ok":true,"label":"never"}`}, 4096},
		{"bigok", []string{ping, "@big"}, 8192},
		{"bigexact", []string{ping, "@big", `{"ok":true,"label":"after"}`}, 5001}, {"bigover", []string{ping, "@big"}, 5000},
		{"late", []string{ping, "@late"}, 0},
		{"dies", []string{ping, `{"ok":true,"label":"x","score":1}`, "@exit"}, 0},
		{"proto2", []string{`{"ok":true,"protocol":2,"prototypes":"abc"}`}, 0},
		{"pingfail", []string{`{"ok":false,"error":"model folder unreadable"}`}, 0},
		{"pingjunk", []string{`garbage`}, 0},
		{"bad", nil, 0}, {"noisy", nil, 0}, {"silent", nil, 0},
	}
	var cases []parity.Case
	clean := func(s string) string { return strings.ReplaceAll(s, root, "{DIR}") }
	for _, sc := range scenarios {
		dir := filepath.Join(root, sc.dir)
		os.MkdirAll(dir, 0o755)
		os.WriteFile(filepath.Join(dir, "replies"), []byte(strings.Join(sc.replies, "\n")+"\n"), 0o644)
		s, err := startServer(dir, sc.maxLine)
		out := map[string]any{"start": clean(parity.Err(err))}
		if err == nil {
			out["prototypes"] = s.Prototypes
			var results []any
			for _, o := range ops {
				switch o.op {
				case "classify":
					label, score, err := s.Classify(o.text)
					results = append(results, []any{label, score, clean(parity.Err(err))})
				case "group":
					g, err := s.Group(o.captures, o.threshold)
					results = append(results, []any{g, clean(parity.Err(err))})
				}
			}
			out["results"] = results
			out["close"] = parity.Err(s.Close())
		}
		reqs, _ := os.ReadFile(filepath.Join(dir, "requests"))
		out["requests"] = parity.Text(string(reqs))
		cases = append(cases, parity.Case{Fn: "server", In: map[string]any{"dir": sc.dir, "replies": parity.Texts(sc.replies), "maxLine": sc.maxLine}, Out: out})
	}
	// Available and Passthrough
	for _, c := range []struct {
		python string
		args   []string
		exit   string
	}{{py, []string{"memory-search", "--model-dir", "m", "find  it"}, ""}, {py, []string{"voice-chat"}, "3"}, {nomod, []string{"memory-search", "q"}, ""},
		{filepath.Join(root, "no-such-python"), []string{"memory-search", "q"}, ""}} {
		t.Setenv(PythonEnv, c.python)
		t.Setenv("FAKE_EXIT", c.exit)
		var stdout, stderr bytes.Buffer
		code := Passthrough(strings.NewReader(""), &stdout, &stderr, c.args...)
		cases = append(cases, parity.Case{Fn: "passthrough", In: []any{clean(c.python), c.args, c.exit},
			Out: []any{clean(parity.Err(Available())), code, stdout.String(), clean(stderr.String())}})
	}
	cases = append(cases, parity.Case{Fn: "scripts", In: map[string]string{"python": fakeScript, "nomodule": noModule}})
	parity.Fixture(t, "helpers", cases)
}
