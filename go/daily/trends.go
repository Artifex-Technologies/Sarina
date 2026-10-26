package daily

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// Day is one day's last numbers.
type Day struct {
	Date string
	Numbers
}

var dayFileRx = regexp.MustCompile(`^(\d{4}-\d{2}-\d{2})\.md$`)

// ReadDays reads the last numbers line of each day's note, oldest first, at most `days` days.
func ReadDays(vault string, days int) []Day {
	files, _ := filepath.Glob(filepath.Join(vault, "Daily", "20*.md"))
	sort.Strings(files)
	var out []Day
	for _, f := range files {
		m := dayFileRx.FindStringSubmatch(filepath.Base(f))
		if m == nil {
			continue
		}
		b, err := os.ReadFile(f)
		if err != nil {
			continue
		}
		n := Parse(m[1], string(b))
		if len(n.Numbers) == 0 {
			continue
		}
		last, ok := ParseNumbers(n.Numbers[len(n.Numbers)-1])
		if !ok {
			continue
		}
		// crashes: the day's highest count (a later refresh never loses a crash, but the top program may change)
		for _, l := range n.Numbers {
			if x, ok := ParseNumbers(l); ok && x.Crashes > last.Crashes {
				last.Crashes, last.Top, last.TopN = x.Crashes, x.Top, x.TopN
			}
		}
		out = append(out, Day{Date: m[1], Numbers: last})
	}
	if days > 0 && len(out) > days {
		out = out[len(out)-days:]
	}
	return out
}

// Trends is the table and what stands out in it, in plain words.
func Trends(days []Day) string {
	if len(days) == 0 {
		return "No daily numbers yet: they are kept from the first `sarina daily-report` or check-up on."
	}
	mounts := map[string]bool{}
	for _, d := range days {
		for m := range d.Disks {
			mounts[m] = true
		}
	}
	ms := make([]string, 0, len(mounts))
	for m := range mounts {
		ms = append(ms, m)
	}
	sort.Strings(ms)
	var b strings.Builder
	fmt.Fprintf(&b, "%-10s", "day")
	for _, m := range ms {
		fmt.Fprintf(&b, " %6s", m)
	}
	b.WriteString("  crashes  failed  memory  load\n")
	for _, d := range days {
		fmt.Fprintf(&b, "%-10s", d.Date)
		for _, m := range ms {
			if p, ok := d.Disks[m]; ok {
				fmt.Fprintf(&b, " %5d%%", p)
			} else {
				fmt.Fprintf(&b, " %6s", "-")
			}
		}
		crash := fmt.Sprint(d.Crashes)
		if d.Top != "" {
			crash += " (" + d.Top + ")"
		}
		fmt.Fprintf(&b, "  %-8s %-7d %5d%%  %.1f\n", crash, d.Failed, d.MemPct, d.Load)
	}
	var notes []string
	first, last := days[0], days[len(days)-1]
	for _, m := range ms {
		a, okA := first.Disks[m]
		z, okZ := last.Disks[m]
		switch {
		case okZ && z >= 90:
			notes = append(notes, fmt.Sprintf("%s is %d%% full: nearly full", m, z))
		case okA && okZ && len(days) > 1 && z-a >= 5:
			notes = append(notes, fmt.Sprintf("%s went from %d%% to %d%% over %d day(s): filling up", m, a, z, len(days)))
		}
	}
	byProg := map[string]int{}
	for _, d := range days {
		if d.Top != "" {
			byProg[d.Top]++
		}
	}
	progs := make([]string, 0, len(byProg))
	for p := range byProg {
		progs = append(progs, p)
	}
	sort.Slice(progs, func(i, j int) bool {
		if byProg[progs[i]] != byProg[progs[j]] {
			return byProg[progs[i]] > byProg[progs[j]]
		}
		return progs[i] < progs[j]
	})
	for _, p := range progs {
		if byProg[p] >= 3 {
			notes = append(notes, fmt.Sprintf("%s crashed on %d of %d day(s): a recurring problem, worth a look (sarina crash-detail %s)", p, byProg[p], len(days), p))
		}
	}
	if last.Failed > 0 {
		notes = append(notes, fmt.Sprintf("%d service(s) failed on the last day", last.Failed))
	}
	if len(notes) == 0 {
		notes = append(notes, "nothing stands out")
	}
	b.WriteString("\nWhat stands out: " + strings.Join(notes, "; ") + ".\n")
	return b.String()
}
