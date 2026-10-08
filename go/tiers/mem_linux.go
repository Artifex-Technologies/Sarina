//go:build linux

package tiers

import "os"

func systemMemoryPercent() (float64, bool) {
	b, err := os.ReadFile("/proc/meminfo")
	if err != nil {
		return 0, false
	}
	return parseMeminfo(string(b))
}
