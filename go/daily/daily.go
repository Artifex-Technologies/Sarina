// Package daily keeps the day's note in Sarina's vault (Daily/YYYY-MM-DD.md): a status summary of how the computer is doing,
// computed in code from the same probes the check-up uses (load, memory, disks, failed services, today's crashes, Heimdall's
// headline), and one line of numbers per refresh so a disk filling up or a program crashing day after day can be seen over
// weeks (`sarina trends`).
//
// "How's the system doing" is asked often and used to run every probe each time. Now the question reads this note when it is
// fresh (`sarina daily-report --max-age MINUTES`) and only probes again when it is stale; the daily check-up refreshes it too.
// The check-up's own lines ("- HH:MM check-up: ...") stay in the note above the status.
package daily

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/sysinfo"
)

// Numbers are the day's figures, one line per refresh.
type Numbers struct {
	At      string         // HH:MM
	Disks   map[string]int // mount point -> percent used
	Crashes int            // today, this user's programs
	Top     string         // the program that crashed most today ("" when none)
	TopN    int
	Failed  int // failed services (system and session)
	MemPct  int
	Load    float64
}

// Status is what the note says about now.
type Status struct {
	At       time.Time
	Cores    int
	Load     float64
	MemUsed  int64 // bytes
	MemTotal int64
	SwapUsed int64
	Disks    []Disk
	Failed   []string
	Crashes  map[string]int
	Security string
	Kernel   int // kernel errors today
}

// Disk is one mounted file system.
type Disk struct {
	Mount string
	Pct   int
	Free  int64
}

// Probe reads the numbers from the computer: a moment, no model.
func Probe(ctx context.Context) Status {
	s := Status{At: time.Now(), Cores: runtime.NumCPU(), Crashes: map[string]int{}}
	if b, err := os.ReadFile("/proc/loadavg"); err == nil {
		if f := strings.Fields(string(b)); len(f) > 0 {
			s.Load, _ = strconv.ParseFloat(f[0], 64)
		}
	}
	if b, err := os.ReadFile("/proc/meminfo"); err == nil {
		kb := func(key string) int64 {
			for _, l := range strings.Split(string(b), "\n") {
				if strings.HasPrefix(l, key+":") {
					f := strings.Fields(l)
					if len(f) >= 2 {
						n, _ := strconv.ParseInt(f[1], 10, 64)
						return n * 1024
					}
				}
			}
			return 0
		}
		s.MemTotal = kb("MemTotal")
		s.MemUsed = s.MemTotal - kb("MemAvailable")
		s.SwapUsed = kb("SwapTotal") - kb("SwapFree")
	}
	for _, m := range mounts() {
		var st syscall.Statfs_t
		if syscall.Statfs(m, &st) != nil || st.Blocks == 0 {
			continue
		}
		used := st.Blocks - st.Bfree
		total := used + st.Bavail
		if total == 0 {
			continue
		}
		s.Disks = append(s.Disks, Disk{Mount: m, Pct: int(used * 100 / total), Free: int64(st.Bavail) * int64(st.Bsize)})
	}
	for _, scope := range [][]string{nil, {"--user"}} {
		out := run(ctx, append(append([]string{"systemctl"}, scope...), "--failed", "--no-legend", "--plain", "--no-pager")...)
		for _, l := range strings.Split(out, "\n") {
			f := strings.Fields(l)
			if len(f) > 0 && !strings.HasPrefix(f[0], "run-") {
				unit := f[0]
				if scope != nil {
					unit += " (your session)"
				}
				s.Failed = append(s.Failed, unit)
			}
		}
	}
	uid := strconv.Itoa(os.Getuid())
	for _, l := range strings.Split(run(ctx, "coredumpctl", "list", "--no-pager", "--no-legend", "--since=today"), "\n") {
		f := strings.Fields(l)
		if len(f) >= 10 && f[5] == uid {
			s.Crashes[filepath.Base(f[9])]++
		}
	}
	for _, l := range strings.Split(run(ctx, "journalctl", "-k", "-p", "err", "--since=today", "--no-pager", "-q", "-o", "cat"), "\n") {
		if strings.TrimSpace(l) != "" {
			s.Kernel++
		}
	}
	s.Security = sysinfo.SecurityHeadline(ctx)
	return s
}

// mounts are the real file systems worth watching: /, /home, /boot and anything under /mnt or /run/media, each once.
func mounts() []string {
	seen := map[string]bool{}
	var out []string
	b, err := os.ReadFile("/proc/self/mounts")
	if err != nil {
		return []string{"/"}
	}
	for _, l := range strings.Split(string(b), "\n") {
		f := strings.Fields(l)
		if len(f) < 3 {
			continue
		}
		dev, mnt, typ := f[0], f[1], f[2]
		if !strings.HasPrefix(dev, "/dev/") || typ == "squashfs" || typ == "iso9660" {
			continue
		}
		if !(mnt == "/" || mnt == "/home" || mnt == "/boot" || mnt == "/boot/efi" || strings.HasPrefix(mnt, "/mnt/") || strings.HasPrefix(mnt, "/run/media/")) {
			continue
		}
		key := dev + " " + mnt
		if dev != "" && typ == "btrfs" {
			key = dev // subvolumes of one btrfs share the space: count it once
		}
		if seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, mnt)
	}
	if len(out) == 0 {
		out = []string{"/"}
	}
	return out
}

func run(ctx context.Context, args ...string) string {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, args[0], args[1:]...).Output()
	if err != nil && len(out) == 0 {
		return ""
	}
	return string(out)
}

func gb(b int64) string { return fmt.Sprintf("%.0f GB", float64(b)/1e9) }

// Render writes the status section and the numbers line of a probe.
func (s Status) Render() (section string, numbers Numbers) {
	numbers = Numbers{At: s.At.Format("15:04"), Disks: map[string]int{}, Failed: len(s.Failed), Load: s.Load}
	if s.MemTotal > 0 {
		numbers.MemPct = int(s.MemUsed * 100 / s.MemTotal)
	}
	var b strings.Builder
	fmt.Fprintf(&b, "## Status (%s)\n\n", s.At.Format("15:04"))
	fmt.Fprintf(&b, "- Load: %.1f on %d cores", s.Load, s.Cores)
	switch {
	case s.Cores > 0 && s.Load > float64(s.Cores):
		b.WriteString(" (busy: more work than cores)")
	case s.Cores > 0 && s.Load > float64(s.Cores)/2:
		b.WriteString(" (working)")
	default:
		b.WriteString(" (idle)")
	}
	b.WriteString("\n")
	if s.MemTotal > 0 {
		fmt.Fprintf(&b, "- Memory: %s of %s used (%d%%)", gb(s.MemUsed), gb(s.MemTotal), numbers.MemPct)
		if s.SwapUsed > 0 {
			fmt.Fprintf(&b, ", swap %s in use", gb(s.SwapUsed))
		}
		b.WriteString("\n")
	}
	if len(s.Disks) > 0 {
		var parts []string
		for _, d := range s.Disks {
			numbers.Disks[d.Mount] = d.Pct
			note := ""
			if d.Pct >= 90 {
				note = " NEARLY FULL"
			}
			parts = append(parts, fmt.Sprintf("%s %d%% used (%s free)%s", d.Mount, d.Pct, gb(d.Free), note))
		}
		fmt.Fprintf(&b, "- Disks: %s\n", strings.Join(parts, "; "))
	}
	if len(s.Failed) == 0 {
		b.WriteString("- Services: none failed\n")
	} else {
		fmt.Fprintf(&b, "- Services failed: %s\n", strings.Join(s.Failed, ", "))
	}
	if len(s.Crashes) == 0 {
		b.WriteString("- Crashes today: none\n")
	} else {
		names := make([]string, 0, len(s.Crashes))
		for n := range s.Crashes {
			names = append(names, n)
		}
		sort.Slice(names, func(i, j int) bool {
			if s.Crashes[names[i]] != s.Crashes[names[j]] {
				return s.Crashes[names[i]] > s.Crashes[names[j]]
			}
			return names[i] < names[j]
		})
		var parts []string
		for _, n := range names {
			numbers.Crashes += s.Crashes[n]
			parts = append(parts, fmt.Sprintf("%s %d", n, s.Crashes[n]))
		}
		numbers.Top, numbers.TopN = names[0], s.Crashes[names[0]]
		fmt.Fprintf(&b, "- Crashes today: %d (%s)\n", numbers.Crashes, strings.Join(parts, ", "))
	}
	if s.Kernel > 0 {
		fmt.Fprintf(&b, "- Kernel errors today: %d\n", s.Kernel)
	}
	if s.Security != "" {
		fmt.Fprintf(&b, "- Security: %s\n", s.Security)
	}
	return b.String(), numbers
}

// Line is the numbers as one line of the note.
func (n Numbers) Line() string {
	mounts := make([]string, 0, len(n.Disks))
	for m := range n.Disks {
		mounts = append(mounts, m)
	}
	sort.Strings(mounts)
	var d []string
	for _, m := range mounts {
		d = append(d, fmt.Sprintf("%s %d%%", m, n.Disks[m]))
	}
	crash := strconv.Itoa(n.Crashes)
	if n.Top != "" {
		crash += fmt.Sprintf(" (%s %d)", n.Top, n.TopN)
	}
	return fmt.Sprintf("- %s | disk %s | crashes %s | failed %d | memory %d%% | load %.1f", n.At, strings.Join(d, ", "), crash, n.Failed, n.MemPct, n.Load)
}

var numbersRx = regexp.MustCompile(`^- (\d{2}:\d{2}) \| disk (.*?) \| crashes (\d+)(?: \((.+?) (\d+)\))? \| failed (\d+) \| memory (\d+)% \| load ([\d.]+)$`)

// ParseNumbers reads one numbers line back.
func ParseNumbers(line string) (Numbers, bool) {
	m := numbersRx.FindStringSubmatch(strings.TrimSpace(line))
	if m == nil {
		return Numbers{}, false
	}
	n := Numbers{At: m[1], Disks: map[string]int{}, Top: m[4]}
	for _, d := range strings.Split(m[2], ", ") {
		if mnt, pct, ok := strings.Cut(d, " "); ok {
			n.Disks[mnt], _ = strconv.Atoi(strings.TrimSuffix(pct, "%"))
		}
	}
	n.Crashes, _ = strconv.Atoi(m[3])
	n.TopN, _ = strconv.Atoi(m[5])
	n.Failed, _ = strconv.Atoi(m[6])
	n.MemPct, _ = strconv.Atoi(m[7])
	n.Load, _ = strconv.ParseFloat(m[8], 64)
	return n, true
}
