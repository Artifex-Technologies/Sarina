package daily

import (
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityDaily records (testdata/parity/daily.json) the daily notes as `sarina trends` reads them: Parse and Render
// of notes with every kind of section, ParseNumbers and Line, ReadDays on a made-up Daily folder (listed in the fixture,
// the Rust test makes it the same way) and Trends over it for several day counts.
func TestParityDaily(t *testing.T) {
	rng := rand.New(rand.NewSource(7))
	numbers := func(n Numbers) any {
		var disks []any
		var keys []string
		for k := range n.Disks {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			disks = append(disks, []any{k, n.Disks[k]})
		}
		return []any{n.At, disks, n.Crashes, n.Top, n.TopN, n.Failed, n.MemPct, n.Load}
	}
	lines := []string{"- 09:00 | disk / 40% | crashes 0 | failed 0 | memory 30% | load 0.5",
		"- 09:00 | disk / 40%, /home 91% | crashes 3 (steam 2) | failed 1 | memory 30% | load 12.25",
		"  - 23:59 | disk  | crashes 0 | failed 0 | memory 0% | load 0  ", "- 9:00 | disk / 40% | crashes 0 | failed 0 | memory 30% | load 0.5",
		"- 09:00 | disk / abc%, /x -5%, /y +7%, nospace | crashes 99999999999999999999 | failed 1 | memory 30% | load 1.2.3",
		"- 09:00 | disk a b c% | crashes 1 (my prog (x) 1) | failed 0 | memory 1% | load .", "- 09:00 | disk / 40% | crashes 1 (x) | failed 0 | memory 1% | load 1",
		"", "garbage", "- 10:00 | disk /mnt/game drive 77% | crashes 0 | failed 0 | memory 100% | load 99999999999999999999.5"}
	var nums []any
	for _, l := range lines {
		n, ok := ParseNumbers(l)
		line := ""
		if ok {
			line = n.Line()
		}
		nums = append(nums, []any{l, ok, numbers(n), line})
	}
	cases := []parity.Case{{Fn: "numbers", Out: nums}}

	texts := []string{"", "# 2026-11-01\n", "---\ncreated: x\n---\n\n# Day\n- 08:00 check-up: fine\n\n## Status (09:30)\n\nAll good.\n  indented\n\n## Numbers\n\n" + lines[0] + "\nnot a number\n" + lines[1] + "\n\n## Later\nmore\n",
		"## Status (9:30)\nnot a status\n## Status (10:00)  \nline\n## Numbers\n", "## Status (10:00)\nline\n##x not a heading\n## Numbers\n- 09:00 | disk / 40% | crashes 2 (a 1) | failed 0 | memory 1% | load 1\n", "head\n\n\n", "## Numbers\n\n" + lines[2], "\r\n## Status (11:11)\r\nwindows\r\n"}
	var parses []any
	for _, s := range texts {
		n := Parse("2026-11-16", s)
		parses = append(parses, []any{s, n.Head, n.StatusAt, n.Status, n.Numbers, n.Render()})
	}
	cases = append(cases, parity.Case{Fn: "parse", Out: parses})

	// the folder
	vault := t.TempDir()
	type file struct{ Name, Text string }
	var tree []file
	progs := []string{"steam", "game.exe", "editor", ""}
	for d := 1; d <= 40; d++ {
		var b strings.Builder
		fmt.Fprintf(&b, "# day %d\n\n## Numbers\n\n", d)
		for k := rng.Intn(4); k >= 0; k-- {
			disks := fmt.Sprintf("/ %d%%", 40+d/2+rng.Intn(3))
			if rng.Intn(3) > 0 {
				disks += fmt.Sprintf(", /home %d%%", 60+rng.Intn(40))
			}
			if d > 30 {
				disks += ", /mnt/new 10%"
			}
			top := progs[rng.Intn(len(progs))]
			crash := fmt.Sprint(rng.Intn(3))
			if top != "" {
				crash = fmt.Sprintf("%d (%s %d)", 1+rng.Intn(5), top, 1+rng.Intn(3))
			}
			fmt.Fprintf(&b, "- %02d:%02d | disk %s | crashes %s | failed %d | memory %d%% | load %.1f\n", 8+k, rng.Intn(60), disks, crash, rng.Intn(2), 20+rng.Intn(70), rng.Float64()*8)
		}
		tree = append(tree, file{fmt.Sprintf("2026-10-%02d.md", d%31+1+d/31), b.String()})
	}
	tree = append(tree, file{"2026-12-01.md", "no numbers\n"}, file{"2026-12-02.md", "## Numbers\n\ngarbage\n"}, file{"notes.md", "x"},
		file{"2026-12-3.md", "## Numbers\n\n" + lines[0] + "\n"}, file{"2026-12-04.md.bak", "x"}, file{"19-01-01.md", "x"},
		file{"1999-12-31.md", "## Numbers\n\n" + lines[0] + "\n"},
		file{"2026-12-05.md", "## Numbers\n\n- 08:00 | disk / 1% | crashes 2 (first 2) | failed 0 | memory 1% | load 1\n- 09:00 | disk / 1% | crashes 2 (second 2) | failed 0 | memory 1% | load 1\n- 10:00 | disk / 1% | crashes 1 (third 1) | failed 0 | memory 1% | load 1\n"})
	for _, f := range tree {
		p := filepath.Join(vault, "Daily", f.Name)
		_ = os.MkdirAll(filepath.Dir(p), 0o755)
		if err := os.WriteFile(p, []byte(f.Text), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	var trends []any
	for _, n := range []int{0, 1, 2, 3, 7, 30, 100, -1} {
		var days []any
		ds := ReadDays(vault, n)
		for _, d := range ds {
			days = append(days, []any{d.Date, numbers(d.Numbers)})
		}
		trends = append(trends, []any{n, days, Trends(ds)})
	}
	trends = append(trends, []any{"empty", nil, Trends(ReadDays(t.TempDir(), 30))})
	cases = append(cases, parity.Case{Fn: "trends", In: tree, Out: trends})

	// Trends on days made from numbers lines: the edges of what stands out
	sets := [][]string{
		{"- 09:00 | disk / 89%, /a 10% | crashes 0 | failed 0 | memory 1% | load 1", "- 09:00 | disk / 90%, /a 14% | crashes 1 (p 1) | failed 0 | memory 1% | load 1"},
		{"- 09:00 | disk / 80%, /a 10% | crashes 1 (p 1) | failed 0 | memory 1% | load 1", "- 09:00 | disk / 85%, /a 15% | crashes 1 (p 1) | failed 2 | memory 1% | load 1",
			"- 09:00 | disk / 84%, /a 14% | crashes 1 (p 1) | failed 0 | memory 1% | load 1"},
		{"- 09:00 | disk / 89% | crashes 1 (q 1) | failed 0 | memory 1% | load 1", "- 09:00 | disk / 93% | crashes 1 (q 1) | failed 1 | memory 1% | load 1"},
		{"- 09:00 | disk / 50% | crashes 0 | failed 0 | memory 1% | load 1"},
		{"- 09:00 | disk /a 10% | crashes 0 | failed 0 | memory 1% | load 1", "- 09:00 | disk /a 15% | crashes 0 | failed 0 | memory 1% | load 1"},
	}
	var made []any
	for _, set := range sets {
		var ds []Day
		for i, l := range set {
			n, _ := ParseNumbers(l)
			ds = append(ds, Day{Date: fmt.Sprintf("2026-11-%02d", i+1), Numbers: n})
		}
		made = append(made, []any{set, Trends(ds)})
	}
	cases = append(cases, parity.Case{Fn: "trendsdays", Out: made})

	// Status.Render and SetStatus on made-up probes (the CLI differential cannot compare a real probe's numbers)
	at := time.Date(2026, 11, 16, 9, 5, 0, 0, time.Local)
	statuses := []Status{{At: at}, {At: at, Cores: 8, Load: 4.05, MemUsed: 6_500_000_000, MemTotal: 16_000_000_000, SwapUsed: 1,
		Disks: []Disk{{"/", 90, 50_500_000_000}, {"/home", 40, 499_999_999}}, Crashes: map[string]int{"b": 2, "a": 2, "c": 5}, Kernel: 3, Security: "Heimdall: x"},
		{At: at, Cores: 4, Load: 4.5, MemUsed: 1, MemTotal: 3, Failed: []string{"x.service", "y (your session)"}}, {At: at, Cores: 4, Load: 2.0}}
	var renders []any
	for i, st := range statuses {
		section, n := st.Render()
		note := Parse("2026-11-16", texts[2])
		if i == 1 {
			for k := 0; k < 50; k++ {
				note.Numbers = append(note.Numbers, lines[0])
			}
		}
		note.SetStatus(st)
		renders = append(renders, []any{i, section, numbers(n), note.Render(), note.Print(at, false)})
	}
	cases = append(cases, parity.Case{Fn: "render", Out: renders})
	parity.Fixture(t, "daily", cases)
}
