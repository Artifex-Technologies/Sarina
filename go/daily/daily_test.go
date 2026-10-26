package daily

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func sample() Status {
	return Status{
		At: time.Date(2026, 10, 25, 16, 40, 0, 0, time.Local), Cores: 24, Load: 0.9,
		MemUsed: 21e9, MemTotal: 64e9, SwapUsed: 0,
		Disks:    []Disk{{"/", 47, 458e9}, {"/boot", 11, 900e6}},
		Failed:   nil,
		Crashes:  map[string]int{"crashy": 2, "spotify": 1},
		Security: "Heimdall: running kernel has no known holes; 1 package(s) with known holes (worst low); 2 setting(s) could be safer",
		Kernel:   0,
	}
}

func TestStatusRendersAndItsNumbersReadBack(t *testing.T) {
	section, n := sample().Render()
	for _, want := range []string{"## Status (16:40)", "Load: 0.9 on 24 cores (idle)", "Memory: 21 GB of 64 GB used (32%)", "/ 47% used (458 GB free)", "Services: none failed", "Crashes today: 3 (crashy 2, spotify 1)", "Security: Heimdall"} {
		if !strings.Contains(section, want) {
			t.Errorf("missing %q in\n%s", want, section)
		}
	}
	line := n.Line()
	if line != "- 16:40 | disk / 47%, /boot 11% | crashes 3 (crashy 2) | failed 0 | memory 32% | load 0.9" {
		t.Fatal(line)
	}
	back, ok := ParseNumbers(line)
	if !ok || back.Disks["/"] != 47 || back.Disks["/boot"] != 11 || back.Crashes != 3 || back.Top != "crashy" || back.TopN != 2 || back.MemPct != 32 || back.Load != 0.9 {
		t.Fatalf("%+v", back)
	}
	full := Status{At: sample().At, Cores: 4, Load: 6.5, Disks: []Disk{{"/", 93, 2e9}}, Failed: []string{"foo.service"}}
	section, _ = full.Render()
	if !strings.Contains(section, "NEARLY FULL") || !strings.Contains(section, "(busy: more work than cores)") || !strings.Contains(section, "Services failed: foo.service") {
		t.Error(section)
	}
}

func TestTheCheckupsLinesStayAboveTheStatusAndFreshnessIsByTime(t *testing.T) {
	text := `---
created: 2026-10-25
source: labyrinth-sarina-checkup
tags: [daily]
---

# 2026-10-25

- 10:09 check-up: crashy crashed 2 times. Check the config file.

## Status (12:00)

- Load: 0.5 on 24 cores (idle)

## Numbers

- 12:00 | disk / 46% | crashes 1 (crashy 1) | failed 0 | memory 30% | load 0.5
- 22:53 check-up: all fine
`
	n := Parse("2026-10-25", text)
	if n.StatusAt != "12:00" || len(n.Numbers) != 1 || len(n.Status) != 1 {
		t.Fatalf("%+v", n)
	}
	// the stray check-up line that an older check-up appended at the end went back up to the head
	head := strings.Join(n.Head, "\n")
	if !strings.Contains(head, "22:53 check-up: all fine") || !strings.Contains(head, "10:09 check-up") {
		t.Fatal(head)
	}
	noon := time.Date(2026, 10, 25, 12, 10, 0, 0, time.Local)
	if !n.Fresh(noon, 15*time.Minute) || n.Fresh(noon, 5*time.Minute) || n.Fresh(noon.Add(24*time.Hour), 15*time.Minute) || n.Fresh(noon, 0) {
		t.Error("freshness")
	}
	n.SetStatus(sample())
	out := n.Render()
	if strings.Index(out, "22:53 check-up") > strings.Index(out, "## Status (16:40)") || strings.Count(out, "## Status") != 1 || len(n.Numbers) != 2 {
		t.Fatal(out)
	}
	again := Parse("2026-10-25", out)
	if again.StatusAt != "16:40" || len(again.Numbers) != 2 || !strings.Contains(strings.Join(again.Head, "\n"), "10:09 check-up") {
		t.Fatalf("%+v", again)
	}
	// an empty note gets a header
	e := Parse("2026-10-26", "")
	if !strings.Contains(strings.Join(e.Head, "\n"), "# 2026-10-26") {
		t.Fatal(e.Head)
	}
	if !strings.Contains(n.Print(noon, true), "from the day's note") || !strings.Contains(n.Print(noon, true), "Today's check-up notes:") {
		t.Error(n.Print(noon, true))
	}
}

func TestTrendsSeeADiskFillingAndARecurringCrash(t *testing.T) {
	v := t.TempDir()
	os.MkdirAll(filepath.Join(v, "Daily"), 0o755)
	days := []string{"2026-10-20", "2026-10-21", "2026-10-22", "2026-10-23"}
	pct := []int{40, 43, 45, 47}
	for i, d := range days {
		n := Note{Day: d, Head: header(d), StatusAt: "10:00", Status: []string{"- x"}}
		n.Numbers = []string{Numbers{At: "10:00", Disks: map[string]int{"/": pct[i]}, Crashes: 1, Top: "crashy", TopN: 1, MemPct: 30, Load: 0.5}.Line()}
		if i == 3 {
			n.Numbers = append(n.Numbers, Numbers{At: "18:00", Disks: map[string]int{"/": pct[i]}, Crashes: 0, Failed: 1, MemPct: 30, Load: 0.5}.Line())
		}
		if err := Save(v, n); err != nil {
			t.Fatal(err)
		}
	}
	got := ReadDays(v, 30)
	if len(got) != 4 || got[3].Crashes != 1 || got[3].Top != "crashy" || got[3].Failed != 1 {
		t.Fatalf("%+v", got)
	}
	out := Trends(got)
	for _, want := range []string{"/ went from 40% to 47% over 4 day(s): filling up", "crashy crashed on 4 of 4 day(s)", "1 service(s) failed on the last day", "2026-10-23"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
	if !strings.Contains(Trends(nil), "No daily numbers yet") || !strings.Contains(Trends(got[:1]), "nothing stands out") {
		t.Error("edge cases")
	}
	if len(ReadDays(v, 2)) != 2 {
		t.Error("days limit")
	}
}
