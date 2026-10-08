package tiers

import (
	"context"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Sample is one reading of the device's resource pressure. A field is only
// meaningful when its OK flag is set; unknown never trips a threshold.
type Sample struct {
	MemPercent float64
	MemOK      bool
	CPUPercent float64
	CPUOK      bool
	GPUPercent float64
	GPUOK      bool
}

// Sampler reads resource pressure. Tests inject fakes.
type Sampler interface {
	Sample(ctx context.Context) Sample
}

// Pressure evaluates a sample against thresholds and returns a reason sentence
// for the first breach (memory, then CPU, then GPU: the original order), or "" when there is none.
func (t Thresholds) Pressure(s Sample) string {
	if t.MaxMemoryPercent > 0 && s.MemOK && s.MemPercent >= t.MaxMemoryPercent {
		return fmt.Sprintf("memory at %.0f%% (threshold %.0f%%)", s.MemPercent, t.MaxMemoryPercent)
	}
	if t.MaxCPUPercent > 0 && s.CPUOK && s.CPUPercent >= t.MaxCPUPercent {
		return fmt.Sprintf("CPU at %.0f%% (threshold %.0f%%)", s.CPUPercent, t.MaxCPUPercent)
	}
	if t.MaxGPUMemoryPercent > 0 && s.GPUOK && s.GPUPercent >= t.MaxGPUMemoryPercent {
		return fmt.Sprintf("GPU memory at %.0f%% (threshold %.0f%%)", s.GPUPercent, t.MaxGPUMemoryPercent)
	}
	return ""
}

// SystemSampler measures the real machine without cgo: system memory through
// /proc/meminfo (Linux) or GlobalMemoryStatusEx (Windows), and the first GPU's
// memory through nvidia-smi when that binary exists. The GPU reading is cached
// briefly so a busy loop does not spawn a process per model call.
type SystemSampler struct {
	GPUCacheFor time.Duration // 0 = 10s
	CPU         bool          // also sample CPU load (costs 0.1 s per sample); off unless a CPU threshold is set

	mu      sync.Mutex
	gpuAt   time.Time
	gpuPct  float64
	gpuOK   bool
	gpuDone bool
}

// Sample implements Sampler.
func (s *SystemSampler) Sample(ctx context.Context) Sample {
	var out Sample
	out.MemPercent, out.MemOK = systemMemoryPercent()
	if s.CPU {
		out.CPUPercent, out.CPUOK = systemCPUPercent()
	}
	out.GPUPercent, out.GPUOK = s.gpu(ctx)
	return out
}

func (s *SystemSampler) gpu(ctx context.Context) (float64, bool) {
	ttl := s.GPUCacheFor
	if ttl == 0 {
		ttl = 10 * time.Second
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.gpuDone && time.Since(s.gpuAt) < ttl {
		return s.gpuPct, s.gpuOK
	}
	s.gpuPct, s.gpuOK = nvidiaMemoryPercent(ctx)
	s.gpuAt, s.gpuDone = time.Now(), true
	return s.gpuPct, s.gpuOK
}

func nvidiaMemoryPercent(ctx context.Context) (float64, bool) {
	bin, err := exec.LookPath("nvidia-smi")
	if err != nil {
		return 0, false
	}
	cctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(cctx, bin, "--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits").Output()
	if err != nil {
		return 0, false
	}
	return parseNvidia(string(out))
}

// parseNvidia reads the first line of "used, total" (MiB) output.
func parseNvidia(out string) (float64, bool) {
	line, _, _ := strings.Cut(strings.TrimSpace(out), "\n")
	used, total, ok := strings.Cut(line, ",")
	if !ok {
		return 0, false
	}
	u, err1 := strconv.ParseFloat(strings.TrimSpace(used), 64)
	t, err2 := strconv.ParseFloat(strings.TrimSpace(total), 64)
	if err1 != nil || err2 != nil || t <= 0 || u < 0 {
		return 0, false
	}
	return u / t * 100, true
}

// parseMeminfo computes used% from /proc/meminfo text.
func parseMeminfo(text string) (float64, bool) {
	var total, avail float64
	var haveT, haveA bool
	for _, line := range strings.Split(text, "\n") {
		k, v, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		f := strings.Fields(v)
		if len(f) == 0 {
			continue
		}
		n, err := strconv.ParseFloat(f[0], 64)
		if err != nil {
			continue
		}
		switch k {
		case "MemTotal":
			total, haveT = n, true
		case "MemAvailable":
			avail, haveA = n, true
		}
	}
	if !haveT || !haveA || total <= 0 {
		return 0, false
	}
	return (total - avail) / total * 100, true
}
