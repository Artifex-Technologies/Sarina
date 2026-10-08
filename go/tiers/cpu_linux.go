//go:build linux

package tiers

import (
	"os"
	"strconv"
	"strings"
	"time"
)

// cpuTimes reads the aggregate busy and total jiffies from /proc/stat.
func cpuTimes() (busy, total float64, ok bool) {
	b, err := os.ReadFile("/proc/stat")
	if err != nil {
		return 0, 0, false
	}
	line, _, _ := strings.Cut(string(b), "\n")
	f := strings.Fields(line)
	if len(f) < 5 || f[0] != "cpu" {
		return 0, 0, false
	}
	var vals []float64
	for _, s := range f[1:] {
		n, err := strconv.ParseFloat(s, 64)
		if err != nil {
			return 0, 0, false
		}
		vals = append(vals, n)
	}
	for _, v := range vals {
		total += v
	}
	idle := vals[3]
	if len(vals) > 4 {
		idle += vals[4] // iowait
	}
	return total - idle, total, true
}

// systemCPUPercent samples CPU use over a short interval (0.1 s, as the original).
func systemCPUPercent() (float64, bool) {
	b1, t1, ok := cpuTimes()
	if !ok {
		return 0, false
	}
	time.Sleep(100 * time.Millisecond)
	b2, t2, ok := cpuTimes()
	if !ok || t2 <= t1 {
		return 0, false
	}
	return (b2 - b1) / (t2 - t1) * 100, true
}
