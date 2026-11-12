package tools

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityFsutil records ResolveInRoot, ResolveForWrite and relPath on a made-up tree for several workspace roots, and
// TruncateOutput and preview (testdata/parity/fsutil.json; docs/RUST_PORT.md). The tree's folder is written "$BASE".
func TestParityFsutil(t *testing.T) {
	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	base := filepath.Join(top, "t")
	tree := [][3]string{
		{"", "dir", ""}, {"ws", "dir", ""}, {"ws/sub", "dir", ""}, {"ws/a.txt", "file", ""}, {"ws/sub/b.go", "file", ""},
		{"ws/in-link", "link", "sub"}, {"ws/out-link", "link", "../out"}, {"ws/abs-out", "link", "$BASE/out/x"}, {"ws/dangling", "link", "nothere"},
		{"ws/dangling-out", "link", "../nothere"}, {"ws/loop", "link", "loop"}, {"ws/file-as-dir", "link", "a.txt"}, {"out", "dir", ""},
		{"out/x", "file", ""}, {"link-ws", "link", "ws"}, {"home", "dir", ""}, {"home/notes", "file", ""}, {"ws/home-link", "link", "../home"},
	}
	for _, e := range tree {
		p := filepath.Join(base, e[0])
		var err error
		switch e[1] {
		case "dir":
			err = os.Mkdir(p, 0o755)
		case "file":
			err = os.WriteFile(p, []byte("x"), 0o644)
		case "link":
			err = os.Symlink(strings.ReplaceAll(e[2], "$BASE", base), p)
		}
		if err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("HOME", filepath.Join(base, "home"))
	t.Chdir(base)
	hide := func(s string) string { return strings.ReplaceAll(strings.ReplaceAll(s, base, "$BASE"), top, "$TOP") }
	var cases []parity.Case
	cases = append(cases, parity.Case{Fn: "tree", In: tree})
	paths := []string{"", ".", "a.txt", "./a.txt", "sub/b.go", "sub/../a.txt", "../ws/a.txt", "../out/x", "../link-ws/a.txt", "in-link/b.go",
		"in-link/new.go", "out-link", "out-link/x", "out-link/new", "abs-out", "dangling", "dangling/x", "dangling-out", "loop", "loop/x",
		"file-as-dir/x", "new/deeper/file", "a.txt/x", "$BASE/ws/a.txt", "$BASE/out/x", "$BASE/link-ws/sub", "/", "/nonexistent-sarina-parity/x",
		"~", "~/notes", "~/new", `~\notes`, "~x", "home-link/notes", "home-link/new", "//$BASE//ws//a.txt", "sub/", "sub/.", ".."}
	for _, root := range []string{"$BASE/ws", "$BASE/link-ws", "$BASE/ws/", "$BASE/nonexistent", "$BASE", "ws"} {
		r := strings.ReplaceAll(root, "$BASE", base)
		var out []any
		for _, p := range paths {
			p := strings.ReplaceAll(p, "$BASE", base)
			in, err1 := ResolveInRoot(r, p)
			w, err2 := ResolveForWrite(r, p)
			out = append(out, []any{hide(in), hide(parity.Err(err1)), hide(w), hide(parity.Err(err2)), hide(relPath(r, w)), hide(relPath(r, p))})
		}
		cases = append(cases, parity.Case{Fn: "resolve", In: []any{root, paths}, Out: out})
	}
	for _, s := range []string{"", "abc", "héllo wörld", "a\nb\tc  d", strings.Repeat("x", 50), strings.Repeat("é", 50) + "😀", "   lead and trail 　 "} {
		for _, n := range []int{-1, 0, 1, 2, 3, 5, 10, 49, 50, 51} {
			cases = append(cases, parity.Case{Fn: "TruncateOutput", In: []any{s, n}, Out: TruncateOutput(s, n)})
			if n >= 0 {
				cases = append(cases, parity.Case{Fn: "preview", In: []any{s, n}, Out: preview(s, n)})
			}
		}
	}
	parity.Fixture(t, "fsutil", cases)
}
