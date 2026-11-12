package tools

import (
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// parityRoots are the workspaces the guard fixture is recorded with: none, a folder that exists on no machine (so
// realRoot changes nothing and the fixture is the same everywhere), the same with a trailing slash, a relative one, and "/".
var parityRoots = []string{"", "/nonexistent-sarina-parity/ws", "/nonexistent-sarina-parity/ws/", "ws-relative", "/"}

// parityPaths are written to by redirections and tee in the extra commands: inside, beside and above the workspace.
var parityPaths = []string{
	"/nonexistent-sarina-parity/ws/out.txt", "/nonexistent-sarina-parity/ws", "/nonexistent-sarina-parity/ws/../x",
	"/nonexistent-sarina-parity/wsx/a", "/nonexistent-sarina-parity/ws/./a/../../ws/b", "out.txt", "./a/../b", "a/../../b",
	"../ws/x", "ws-relative/x", "../ws-relative/x", "/", "//nonexistent-sarina-parity//ws//y", "~", "~/x", "/dev/fd/3", "/dev/tty1",
	"/dev/null/x", "/nonexistent-sarina-parity/ws\xff/a", "",
}

// verdict runs a check, many times when the line has two pipelines or more: Go walks the pipelines in map order (a small
// map starts at a random one of 8 slots, so the rarer order comes about 1 time in 8), and a command refused for two
// different reasons can name either one. The fixture holds the reason (Blocked is always reason != ""), or
// {"reasons": [...]}, sorted, when Go gave more than one.
func verdict(t *testing.T, cmd string, check func() Verdict) any {
	runs := 1
	if strings.Count(cmd, "|") >= 2 {
		runs = 400
	}
	seen := map[string]bool{}
	for i := 0; i < runs; i++ {
		v := check()
		if v.Blocked != (v.Reason != "") {
			t.Fatalf("blocked %v with reason %q", v.Blocked, v.Reason)
		}
		seen[v.Reason] = true
	}
	var reasons []string
	for r := range seen {
		reasons = append(reasons, r)
	}
	sort.Strings(reasons)
	if len(reasons) == 1 {
		return parity.Text(reasons[0])
	}
	return map[string]any{"reasons": parity.Texts(reasons)}
}

// TestParityDestructive records CheckDestructive (each of parityRoots, in order) and CheckPrivileged on the shell corpus and on writes to
// paths around the workspace (testdata/parity/destructive.json; docs/RUST_PORT.md).
func TestParityDestructive(t *testing.T) {
	cmds := append(parity.ShellCorpus(t), parity.GuardCorpus()...)
	for _, p := range parityPaths {
		cmds = append(cmds, "echo x > "+p, "echo x >> '"+p+"'", "a | tee -a "+p+" b", "cat >| "+p, "x 2>"+p, "x &>"+p, "x >&"+p,
			"x <>"+p, "sudo tee "+p, "find . -exec tee "+p+" ;", "bash -c 'echo > "+p+"'")
	}
	var cases []parity.Case
	for _, c := range cmds {
		var d []any
		for _, root := range parityRoots {
			d = append(d, verdict(t, c, func() Verdict { return CheckDestructive(c, root) }))
		}
		cases = append(cases, parity.Case{Fn: "Check", In: parity.Text(c),
			Out: map[string]any{"destructive": d, "privileged": verdict(t, c, func() Verdict { return CheckPrivileged(c) })}})
	}
	// within and realRoot on their own: Rel's edges
	for _, root := range parityRoots {
		for _, p := range parityPaths {
			cases = append(cases, parity.Case{Fn: "within", In: []any{root, parity.Text(p)}, Out: within(root, p)})
			rel, err := filepath.Rel(root, p)
			cases = append(cases, parity.Case{Fn: "filepath.Rel", In: []any{root, parity.Text(p)}, Out: []any{parity.Text(rel), parity.Err(err)}})
		}
	}
	parity.Fixture(t, "destructive", cases)
}
