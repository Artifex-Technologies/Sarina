// Package parity writes the fixtures the Rust port runs (docs/RUST_PORT.md): what the Go code answers on the same
// inputs, recorded by each package's parity_test.go, in testdata/parity/<name>.json at the repository's top. Run
//
//	SARINA_PARITY_UPDATE=1 go test ./... -run Parity
//
// after a deliberate change, and review the diff; without the variable a test fails when its fixture is out of date, so
// the fixtures cannot drift from the Go code unnoticed. Only tests import this package; it is never part of a build.
package parity

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"
	"unicode/utf8"
)

// UpdateEnv set to 1 rewrites the fixtures instead of comparing them.
const UpdateEnv = "SARINA_PARITY_UPDATE"

// Case is one recorded answer: a function name, its input and what Go returned.
type Case struct {
	Fn  string `json:"fn"`
	In  any    `json:"in"`
	Out any    `json:"out"`
}

// Root is the repository's root folder (above go/).
func Root() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Dir(filepath.Dir(filepath.Dir(filepath.Dir(file))))
}

// Fixture compares cases with testdata/parity/<name>.json, or writes them when SARINA_PARITY_UPDATE=1.
func Fixture(t *testing.T, name string, cases []Case) {
	t.Helper()
	// one case per line: small files and readable diffs
	var buf bytes.Buffer
	buf.WriteString(`{"fixture":` + strconv.Quote(name) + `,"cases":[` + "\n")
	for i, c := range cases {
		var line bytes.Buffer
		enc := json.NewEncoder(&line)
		enc.SetEscapeHTML(false)
		if err := enc.Encode(c); err != nil {
			t.Fatal(err)
		}
		buf.Write(bytes.TrimSuffix(line.Bytes(), []byte("\n")))
		if i < len(cases)-1 {
			buf.WriteByte(',')
		}
		buf.WriteByte('\n')
	}
	buf.WriteString("]}\n")
	path := filepath.Join(Root(), "testdata", "parity", name+".json")
	if os.Getenv(UpdateEnv) == "1" {
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, buf.Bytes(), 0o644); err != nil {
			t.Fatal(err)
		}
		t.Logf("wrote %s (%d cases)", path, len(cases))
		return
	}
	old, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("%s: %v (write it with %s=1)", path, err, UpdateEnv)
	}
	if !bytes.Equal(old, buf.Bytes()) {
		// the first line that differs, cut short, so a failure on another machine says which case moved
		ol, nl := bytes.Split(old, []byte("\n")), bytes.Split(buf.Bytes(), []byte("\n"))
		for i := 0; i < max(len(ol), len(nl)); i++ {
			var a, b []byte
			if i < len(ol) {
				a = ol[i]
			}
			if i < len(nl) {
				b = nl[i]
			}
			if !bytes.Equal(a, b) {
				t.Errorf("line %d\n  fixture: %.800s\n  now:     %.800s", i+1, a, b)
				break
			}
		}
		t.Fatalf("%s is out of date with the Go code: rerun with %s=1 and review the diff", path, UpdateEnv)
	}
}

// Text is a Go string as a fixture holds it: the string itself when it is valid UTF-8, else {"hex": its bytes}, so a
// string that is not UTF-8 reaches the Rust side byte for byte (JSON would turn each bad byte into U+FFFD).
func Text(s string) any {
	if utf8.ValidString(s) {
		return s
	}
	return map[string]string{"hex": hex.EncodeToString([]byte(s))}
}

// Texts is Text of each.
func Texts(ss []string) []any {
	out := make([]any, len(ss))
	for i, s := range ss {
		out[i] = Text(s)
	}
	return out
}

// Err is an error's text, or "" for none.
func Err(err error) string {
	if err == nil {
		return ""
	}
	return err.Error()
}

// StringLiterals returns every string literal in the named Go files (relative to the calling test's folder), in file order,
// unquoted. A parity test uses it to carry over every input of the package's own tests without copying them by hand.
func StringLiterals(t *testing.T, files ...string) []string {
	t.Helper()
	var out []string
	fset := token.NewFileSet()
	for _, f := range files {
		file, err := parser.ParseFile(fset, f, nil, 0)
		if err != nil {
			t.Fatal(err)
		}
		ast.Inspect(file, func(n ast.Node) bool {
			if lit, ok := n.(*ast.BasicLit); ok && lit.Kind == token.STRING {
				if s, err := strconv.Unquote(lit.Value); err == nil {
					out = append(out, s)
				}
			}
			return true
		})
	}
	return out
}

// Generated compares data with the file at path (relative to the repository's root), or writes it when
// SARINA_PARITY_UPDATE=1: data the Rust port carries as it stands (the tool definitions the model sees), recorded from
// the Go code so the two cannot drift apart.
func Generated(t *testing.T, path string, data []byte) {
	t.Helper()
	full := filepath.Join(Root(), path)
	if os.Getenv(UpdateEnv) == "1" {
		if err := os.WriteFile(full, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	old, err := os.ReadFile(full)
	if err != nil || !bytes.Equal(old, data) {
		t.Fatalf("%s is out of date with the Go code: rerun with %s=1 and review the diff", path, UpdateEnv)
	}
}
