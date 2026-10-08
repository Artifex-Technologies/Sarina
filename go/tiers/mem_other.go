//go:build !linux && !windows

package tiers

// systemMemoryPercent is unknown on platforms without a native reader here;
// unknown never trips a threshold.
func systemMemoryPercent() (float64, bool) { return 0, false }
