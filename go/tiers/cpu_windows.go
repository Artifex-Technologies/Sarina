//go:build windows

package tiers

import (
	"syscall"
	"time"
	"unsafe"
)

func ftUint(ft syscall.Filetime) uint64 { return uint64(ft.HighDateTime)<<32 | uint64(ft.LowDateTime) }

func cpuTimes() (busy, total float64, ok bool) {
	var idle, kernel, user syscall.Filetime
	proc := syscall.NewLazyDLL("kernel32.dll").NewProc("GetSystemTimes")
	r, _, _ := proc.Call(uintptr(unsafe.Pointer(&idle)), uintptr(unsafe.Pointer(&kernel)), uintptr(unsafe.Pointer(&user)))
	if r == 0 {
		return 0, 0, false
	}
	// kernel time includes idle time
	tot := float64(ftUint(kernel) + ftUint(user))
	return tot - float64(ftUint(idle)), tot, true
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
