package testguard

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// parityTree is the made-up folder tree the fixture is recorded on: [path, kind, link target]. "$BASE" in a target is the
// tree's own folder (the Rust test makes the same tree in its own temporary folder).
var parityTree = [][3]string{
	{"ws", "dir", ""}, {"ws/sub", "dir", ""}, {"ws/file", "file", ""}, {"out", "dir", ""}, {"out/x", "file", ""},
	{"tmp", "dir", ""}, {"r1", "dir", ""}, {"r2", "dir", ""}, {"r2/deep", "dir", ""},
	{"link-ws", "link", "ws"}, {"abs-link", "link", "$BASE/ws"}, {"ws/escape", "link", "../out"}, {"ws/loop1", "link", "loop2"},
	{"ws/loop2", "link", "loop1"}, {"ws/dangling", "link", "nothere"}, {"ws/filelink", "link", "file"}, {"r1/inner", "link", "../out"},
	{"r1/back", "link", "../r1"}, {"r1/up", "link", ".."}, {"root-link", "link", "/"}, {"ws/sub/dots", "link", "../../ws/./sub/.."},
	{"chain1", "link", "chain2"}, {"chain2", "link", "chain3"}, {"chain3", "link", "ws/sub"}, {"tmp/in", "link", "../r2/deep"},
}

// parityPathsIn are evaluated relative to the tree ("$BASE/" is put in front unless they start with "/" or "rel:").
var parityPathsIn = []string{
	"", ".", "ws", "ws/", "ws/sub", "ws/file", "ws/file/x", "ws/missing", "ws/missing/deeper", "link-ws", "link-ws/sub",
	"link-ws/../out", "abs-link/sub", "ws/escape", "ws/escape/x", "ws/escape/new", "ws/loop1", "ws/loop1/x", "ws/dangling",
	"ws/dangling/x", "ws/filelink", "ws/filelink/x", "r1/inner/x", "r1/back/back/back/inner", "r1/up/ws", "root-link",
	"root-link/tmp", "ws/sub/dots", "ws/sub/dots/file", "chain1", "chain1/..", "chain1/../..", "ws/./sub/../file", "ws//sub///",
	"ws/../ws/sub", "../x", "tmp/in", "tmp/in/new.md", "tmp/note.md", "r2/deep/x", "r2x/a", "r1", "/", "/nonexistent-sarina-parity/a",
	"//", "/..", "rel:ws/sub", "rel:link-ws/file", "rel:../x", "rel:.", "rel:", "rel:ws/escape/n",
}

func pathIn(base, p string) string {
	if rest, ok := strings.CutPrefix(p, "rel:"); ok {
		return rest
	}
	if strings.HasPrefix(p, "/") {
		return p
	}
	if p == "" {
		return base
	}
	return base + "/" + p
}

// TestParityTestguard records, on the made-up tree, filepath.EvalSymlinks and Abs (with the tree as the working folder),
// Clean, Join, Dir and Base on a path corpus, and Allow under each combination of SARINA_TEST, SARINA_TEST_ROOT and TMPDIR
// (testdata/parity/testguard.json; docs/RUST_PORT.md). The tree's folder is written "$BASE" in every answer, the folder
// above it "$TOP".
func TestParityTestguard(t *testing.T) {
	top := t.TempDir()
	if real, err := filepath.EvalSymlinks(top); err == nil {
		top = real
	}
	base := filepath.Join(top, "tree")
	if err := os.Mkdir(base, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, e := range parityTree {
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
	t.Chdir(base)
	hide := func(s string) string { return strings.ReplaceAll(strings.ReplaceAll(s, base, "$BASE"), top, "$TOP") }
	var cases []parity.Case
	cases = append(cases, parity.Case{Fn: "tree", In: parityTree, Out: nil})
	for _, p := range parityPathsIn {
		in := pathIn(base, p)
		r, err := filepath.EvalSymlinks(in)
		cases = append(cases, parity.Case{Fn: "EvalSymlinks", In: p, Out: []any{hide(r), err != nil}})
		a, err := filepath.Abs(in)
		cases = append(cases, parity.Case{Fn: "Abs", In: p, Out: []any{hide(a), err != nil}})
		cases = append(cases, parity.Case{Fn: "resolve", In: p, Out: hide(resolve(in))})
	}
	// a working folder reached through a link: Go's Getwd answers $PWD (the link's path), the kernel the real one
	t.Chdir(filepath.Join(base, "link-ws", "sub"))
	for _, p := range []string{"rel:x", "rel:.", "rel:..", "rel:../../../tree/ws", "rel:../../link-ws/file",
		"rel:../dots", "rel:../../chain1/..", "/bin", "/bin/."} {
		in := pathIn(base, p)
		r, err := filepath.EvalSymlinks(in)
		cases = append(cases, parity.Case{Fn: "EvalSymlinks via link", In: p, Out: []any{hide(r), err != nil}})
		a, err := filepath.Abs(in)
		cases = append(cases, parity.Case{Fn: "Abs via link", In: p, Out: []any{hide(a), err != nil}})
		cases = append(cases, parity.Case{Fn: "resolve via link", In: p, Out: hide(resolve(in))})
	}
	t.Chdir(base)
	for _, p := range []string{"", ".", "..", "/", "//", "///a//b/", "a/b/../..", "a/../../b", "../../a/./b/../c", "/../a", "/a/../../b",
		"./", "a//", "a/.", "a/..", ".a/..b/...", "a/b/c/../../d", "/a/b/", "x\xff/../y", "...", "a/./../b/./c/.", "/./"} {
		cases = append(cases, parity.Case{Fn: "Clean", In: parity.Text(p), Out: parity.Text(filepath.Clean(p))})
		cases = append(cases, parity.Case{Fn: "Dir", In: parity.Text(p), Out: parity.Text(filepath.Dir(p))})
		cases = append(cases, parity.Case{Fn: "Base", In: parity.Text(p), Out: parity.Text(filepath.Base(p))})
		cases = append(cases, parity.Case{Fn: "Join", In: []any{"/r", parity.Text(p)}, Out: parity.Text(filepath.Join("/r", p))})
		cases = append(cases, parity.Case{Fn: "Join", In: []any{parity.Text(p), "x"}, Out: parity.Text(filepath.Join(p, "x"))})
		cases = append(cases, parity.Case{Fn: "Join", In: []any{"", parity.Text(p)}, Out: parity.Text(filepath.Join("", p))})
	}
	for _, test := range []string{"", "1", "0", "true", " 1"} {
		for _, roots := range []string{"", "$BASE/r1", "$BASE/r1:$BASE/r2", "$BASE/link-ws", ":", "rel-root", "r2", "$BASE/r1/", "$BASE/ws/escape"} {
			for _, tmp := range []string{"$BASE/tmp", "$BASE/tmp/in"} { // not "": /tmp would hold the tree on one machine and not another
				t.Setenv(Env, test)
				t.Setenv(RootEnv, strings.ReplaceAll(roots, "$BASE", base))
				t.Setenv("TMPDIR", strings.ReplaceAll(tmp, "$BASE", base))
				var out []any
				for _, p := range parityPathsIn {
					out = append(out, hide(parity.Err(Allow(pathIn(base, p)))))
				}
				cases = append(cases, parity.Case{Fn: "Allow", In: []string{test, roots, tmp}, Out: out})
			}
		}
	}
	parity.Fixture(t, "testguard", cases)
}
