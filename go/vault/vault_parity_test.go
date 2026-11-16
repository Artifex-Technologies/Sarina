package vault

import (
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityVault records (testdata/parity/vault.json) how a vault is read: ParseNote on texts with every kind of front
// matter, heading, link and tag; Notes on a made-up tree (hidden folders, big files, links in and out, a named pipe, equal
// times); Resolve, SafePath, Search with its ranking, Backlinks, Suggest, Lessons with Digest, and Lint on it; and Go's
// sort.Slice order for keys with many ties (the port's gosort::slice). The tree is listed in the fixture; the Rust test
// makes it the same way.
func TestParityVault(t *testing.T) {
	rng := rand.New(rand.NewSource(41))
	note := func(n Note) any {
		var front []any
		var keys []string
		for k := range n.Front {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			front = append(front, []any{k, n.Front[k]})
		}
		return []any{n.Path, n.Title, n.Tags, n.Links, n.Body, front}
	}
	texts := []string{"", "plain", "# Title\nbody", "---\ntags: [a, b]\ncreated: 2026-11-01\n---\n# T\n#x and #y/z #9no\n",
		"---\nno end\n# H", "---\n---\nempty front", "---\nKey : Value: more\n  tags :  #one, two three \n---\n\n\nbody [[A]] [[B#h]] [[C|alias]] [[D#h|al]] [[ E ]]",
		"text #Tag #tag #TAG, #a-b_c/d #-no", "[[x]] [[]] [[a]b]] [[ [[y]]", "#Not a heading\n#  Heading two  \n# \n#\tTab", "---\r\ntags: a\r\n---\r\nCRLF",
		"---\ntags: \"quoted\", 'q2', #h\n---\n", "body\n# Late heading\n# Second", "Été #café [[Nöte]] #été", "  # indented\n#nospace",
		"---\ntags: [ ]\n---\n", "a\n---\nb\n---\n", "---\nonly: front\n---"}
	var parses []any
	for i, s := range texts {
		rel := []string{"x.md", "Lessons/Note one.md", "a/b/c.md", "noext"}[i%4]
		parses = append(parses, []any{rel, s, note(ParseNote(rel, s, time.Time{}))})
	}
	cases := []parity.Case{{Fn: "parse", Out: parses}}

	// the tree
	root, outside := t.TempDir(), t.TempDir()
	if r, err := filepath.EvalSymlinks(root); err == nil {
		root = r
	}
	type entry struct {
		Path, Text string
		Mtime      int64
	}
	words := []string{"backup", "disk", "steam", "proton", "audio", "pipewire", "kernel", "crash", "game", "driver", "nvidia",
		"vault", "lesson", "verify", "write", "network", "wifi", "boot", "slow", "memory", "the", "and", "a", "of", "fix"}
	var tree []entry
	add := func(p, s string, m int64) { tree = append(tree, entry{p, s, m}) }
	add("Home.md", "---\ncreated: 2026-11-01\n---\n# Home\n[[Lessons/Verify after writing]] [[Projects/Game fixes]] [[Missing note]]\n", 1000)
	add("Lessons/Verify after writing.md", "---\nrule: \"Read a file back after writing it.\"\nstatus: active\n---\n# Verify after writing\nAfter write_file, read it back. #lesson\n", 1100)
	add("Lessons/Short answers.md", "---\nrule:   Keep   spoken\n answers short\nstatus: 'Proposed'\n---\n# Short answers\n", 1100)
	add("Lessons/No rule.md", "---\nstatus: active\n---\nnothing\n", 1200)
	add("Lessons/Retired.md", "---\nrule: Old rule\nstatus: retired\n---\n", 900)
	add("Lessons/No status.md", "---\nrule: A rule without a status\n---\n", 950)
	add("Index9.md", "# Nine\n"+strings.Repeat("[[Gen/Gen 1]] ", 9), 1260)
	add("Tagged.md", "# Tagged\n#game first line\nthe proton line\n", 1270)
	add("Projects/Game fixes.md", "# Game fixes\nDead Space 2 at 3440x1440 [[Entities/Proton]] #game\n", 1300)
	add("Entities/Proton.md", "---\ntags: [game, wine]\n---\n# Proton\nValve's Wine. [[Projects/Game fixes]]\n", 1300)
	add("Index.md", "# Index\n"+strings.Repeat("[[Home]] [[Entities/Proton]] [[Projects/Game fixes]] ", 4), 1250)
	add(".obsidian/app.md", "# hidden\nbackup", 2000)
	add("Inbox/.hidden.md", "# hidden too\nbackup", 2000)
	add("Inbox/readme.txt", "not a note backup", 2000)
	add("Big.md", "# Big\n"+strings.Repeat("backup ", 80000), 2000)
	for i := 0; i < 30; i++ {
		var b strings.Builder
		fmt.Fprintf(&b, "# %s %s %d\n", words[rng.Intn(len(words))], words[rng.Intn(len(words))], i)
		for j := rng.Intn(60); j >= 0; j-- {
			b.WriteString(words[rng.Intn(len(words))] + " ")
			if rng.Intn(9) == 0 {
				b.WriteString("\n")
			}
		}
		if rng.Intn(3) == 0 {
			fmt.Fprintf(&b, "\n[[Gen %d]]", rng.Intn(30))
		}
		if rng.Intn(4) == 0 {
			b.WriteString(" #" + words[rng.Intn(10)])
		}
		add(fmt.Sprintf("Gen/Gen %d.md", i), b.String(), int64(1400+rng.Intn(5)))
	}
	for _, e := range tree {
		p := filepath.Join(root, filepath.FromSlash(e.Path))
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(e.Text), 0o644); err != nil {
			t.Fatal(err)
		}
		mt := time.Unix(e.Mtime, 0)
		if err := os.Chtimes(p, mt, mt); err != nil {
			t.Fatal(err)
		}
	}
	// links (a link's time is when it was made: written as -1) and a named pipe
	if err := os.WriteFile(filepath.Join(outside, "key"), []byte("PRIVATE"), 0o600); err != nil {
		t.Fatal(err)
	}
	_ = os.Symlink(filepath.Join(outside, "key"), filepath.Join(root, "Leak.md"))
	_ = os.Symlink(filepath.Join(root, "Entities", "Proton.md"), filepath.Join(root, "Alias.md"))
	_ = os.Symlink(outside, filepath.Join(root, "Out"))
	_ = syscall.Mkfifo(filepath.Join(root, "Pipe.md"), 0o644)
	v, err := Open(root)
	if err != nil {
		t.Fatal(err)
	}
	notes, err := v.Notes()
	if err != nil {
		t.Fatal(err)
	}
	var listed []any
	for _, n := range notes {
		mt := n.Modified.Unix()
		if st, err := os.Lstat(filepath.Join(root, n.Path)); err == nil && st.Mode()&os.ModeSymlink != 0 {
			mt = -1
		}
		listed = append(listed, []any{note(n), mt})
	}
	cases = append(cases, parity.Case{Fn: "notes", In: tree, Out: listed})

	var resolves []any
	for _, target := range []string{"Home", "home.md", "/Home", "Proton", "Entities/Proton", "entities/proton.md", "Game fixes", "game FIXES",
		"Verify after writing", " Proton ", "Missing", "", "/", "Gen 3", "gen/gen 3", "Valve", "Index", "Lessons", "Alias", "a/../Home"} {
		resolves = append(resolves, []any{target, Resolve(notes, target)})
	}
	cases = append(cases, parity.Case{Fn: "resolve", Out: resolves})

	var safe []any
	for _, name := range []string{"Lessons/New", "Inbox/Voice test.md", "", " ", "/etc/passwd", "../x", "a/../b", "a/./b", ".hidden", "a/.git/x",
		"x\x00y", "a\\b", strings.Repeat("n", 201), strings.Repeat("n", 197), "Out/x", "Out", "a//b", "trailing/", "./x", "Home"} {
		rel, abs, err := v.SafePath(name)
		safe = append(safe, []any{parity.Text(name), rel, strings.TrimPrefix(abs, root), parity.Err(err)})
	}
	cases = append(cases, parity.Case{Fn: "safepath", Out: safe})

	var searches []any
	for _, q := range []string{"backup", "game proton", "the and of", "", "a", "Proton", "steam crash fix", "verify writing", "nvidia driver kernel",
		"disk slow boot", "wine", "lesson", "audio pipewire wifi network", "café", "x", "first"} {
		for _, limit := range []int{0, 3} {
			var hits []any
			for _, h := range Search(notes, q, limit) {
				hits = append(hits, []any{h.Path, h.Title, h.Score, h.Snippet, h.Why})
			}
			searches = append(searches, []any{q, limit, hits})
		}
	}
	cases = append(cases, parity.Case{Fn: "search", Out: searches})

	var backs, suggests []any
	for _, n := range notes {
		backs = append(backs, []any{n.Path, Backlinks(notes, n.Path)})
	}
	for _, l := range []string{"Missing note", "Game fix", "Verify writing", "Protons", "zzz", "", "the", "Gen", "Short answer", "Game fixes aaaa bbbb cccc"} {
		suggests = append(suggests, []any{l, Suggest(notes, l)})
	}
	cases = append(cases, parity.Case{Fn: "backlinks", Out: backs}, parity.Case{Fn: "suggest", Out: suggests})

	ls, err := v.Lessons()
	if err != nil {
		t.Fatal(err)
	}
	var lessons []any
	for _, l := range ls {
		lessons = append(lessons, []any{l.Note, l.Rule, l.Status})
	}
	var lint []any
	for _, p := range Lint(notes) {
		lint = append(lint, []any{p.Path, p.What})
	}
	cases = append(cases, parity.Case{Fn: "lessons", Out: []any{lessons, Digest(ls)}}, parity.Case{Fn: "lint", Out: lint})
	// Lint on a vault whose Home links nowhere
	var lint2 []any
	for _, p := range Lint([]Note{ParseNote("Home.md", "# Home\n", time.Time{}), ParseNote("A.md", "---\nx: y\n---\n[[B]]", time.Time{})}) {
		lint2 = append(lint2, []any{p.Path, p.What})
	}
	cases = append(cases, parity.Case{Fn: "lint2", Out: lint2})

	// sort.Slice with ties: the order Go leaves equal keys in
	var sorts []any
	for _, n := range []int{0, 1, 5, 12, 13, 20, 49, 50, 51, 100, 300, 1000} {
		for _, kinds := range []int{1, 3, 1000} {
			keys := make([]int, n)
			for i := range keys {
				keys[i] = rng.Intn(kinds)
			}
			if kinds == 1000 && n > 50 { // runs: ascending, descending, sawtooth
				for i := range keys {
					keys[i] = []int{i, n - i, i % 7}[n%3] / 3
				}
			}
			idx := make([]int, n)
			for i := range idx {
				idx[i] = i
			}
			sort.Slice(idx, func(a, b int) bool { return keys[idx[a]] < keys[idx[b]] })
			sorts = append(sorts, []any{keys, idx})
		}
	}
	cases = append(cases, parity.Case{Fn: "sortslice", Out: sorts})
	parity.Fixture(t, "vault", cases)
}
