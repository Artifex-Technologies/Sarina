//go:build windows

package tiers

import (
	"syscall"
	"unsafe"
)

type memoryStatusEx struct {
	Length               uint32
	MemoryLoad           uint32
	TotalPhys            uint64
	AvailPhys            uint64
	TotalPageFile        uint64
	AvailPageFile        uint64
	TotalVirtual         uint64
	AvailVirtual         uint64
	AvailExtendedVirtual uint64
}

func systemMemoryPercent() (float64, bool) {
	proc := syscall.NewLazyDLL("kernel32.dll").NewProc("GlobalMemoryStatusEx")
	ms := memoryStatusEx{Length: uint32(unsafe.Sizeof(memoryStatusEx{}))}
	r, _, _ := proc.Call(uintptr(unsafe.Pointer(&ms)))
	if r == 0 {
		return 0, false
	}
	return float64(ms.MemoryLoad), true
}
