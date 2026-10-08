//go:build !linux && !windows

package tiers

// systemCPUPercent is unknown on platforms without a native reader here;
// unknown never trips a threshold.
func systemCPUPercent() (float64, bool) { return 0, false }
