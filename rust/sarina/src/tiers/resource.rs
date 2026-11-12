//! Go: `tiers/resource.go`, `cpu_linux.go`, `mem_linux.go`: the first tier's device under resource pressure. Memory from
//! /proc/meminfo, CPU load from /proc/stat over 0.1 s (only when a CPU threshold is set), the first card's memory through
//! nvidia-smi (its answer read with a bound, cached for 10 s). An unknown value never trips a threshold.

use super::config::Thresholds;
use crate::goctx::Ctx;
use crate::gotext;
use crate::gotext::parse_float;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One reading; a value counts only when its flag is set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub mem_percent: f64,
    pub mem_ok: bool,
    pub cpu_percent: f64,
    pub cpu_ok: bool,
    pub gpu_percent: f64,
    pub gpu_ok: bool,
}

/// Reads resource pressure (tests give their own).
pub trait Sampler {
    fn sample(&self, ctx: &Ctx) -> Sample;
}

impl Thresholds {
    /// Go's `Pressure`: the first breach (memory, then CPU, then GPU) as a sentence, or "".
    pub fn pressure(&self, s: &Sample) -> String {
        if self.max_memory_percent > 0.0 && s.mem_ok && s.mem_percent >= self.max_memory_percent {
            return format!("memory at {:.0}% (threshold {:.0}%)", s.mem_percent, self.max_memory_percent);
        }
        if self.max_cpu_percent > 0.0 && s.cpu_ok && s.cpu_percent >= self.max_cpu_percent {
            return format!("CPU at {:.0}% (threshold {:.0}%)", s.cpu_percent, self.max_cpu_percent);
        }
        if self.max_gpu_memory_percent > 0.0 && s.gpu_ok && s.gpu_percent >= self.max_gpu_memory_percent {
            return format!("GPU memory at {:.0}% (threshold {:.0}%)", s.gpu_percent, self.max_gpu_memory_percent);
        }
        String::new()
    }
}

fn float(s: &str) -> Option<f64> {
    parse_float(s).ok()
}

/// Go's `parseNvidia`: the first line of "used, total" (MiB).
pub fn parse_nvidia(out: &str) -> Option<f64> {
    let first = gotext::trim_space(out).split('\n').next().unwrap_or("");
    let (used, total) = first.split_once(',')?;
    let (u, t) = (float(gotext::trim_space(used))?, float(gotext::trim_space(total))?);
    if t <= 0.0 || u < 0.0 {
        return None;
    }
    Some(u / t * 100.0)
}

/// Go's `parseMeminfo`: used percent from /proc/meminfo's text.
pub fn parse_meminfo(text: &str) -> Option<f64> {
    let (mut total, mut avail) = (None, None);
    for line in text.split('\n') {
        let Some((k, v)) = line.split_once(':') else { continue };
        let Some(first) = gotext::fields(v).first().copied() else { continue };
        let Some(n) = float(first) else { continue };
        match k {
            "MemTotal" => total = Some(n),
            "MemAvailable" => avail = Some(n),
            _ => {}
        }
    }
    let (total, avail) = (total?, avail?);
    if total <= 0.0 {
        return None;
    }
    Some((total - avail) / total * 100.0)
}

/// nvidia-smi's answer is read to at most this many bytes; more is no answer.
pub const MAX_NVIDIA_OUTPUT: usize = 64 << 10;

/// The real machine (Go's `SystemSampler`).
pub struct SystemSampler {
    /// zero: 10 s
    pub gpu_cache_for: Duration,
    /// also sample CPU load (0.1 s per sample)
    pub cpu: bool,
    gpu: Mutex<Option<(Instant, Option<f64>)>>,
}

impl SystemSampler {
    pub fn new(cpu: bool) -> SystemSampler {
        SystemSampler { gpu_cache_for: Duration::ZERO, cpu, gpu: Mutex::new(None) }
    }

    fn gpu(&self, ctx: &Ctx) -> Option<f64> {
        let ttl = if self.gpu_cache_for.is_zero() { Duration::from_secs(10) } else { self.gpu_cache_for };
        let mut g = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, v)) = *g {
            if at.elapsed() < ttl {
                return v;
            }
        }
        let v = nvidia_memory_percent(ctx);
        *g = Some((Instant::now(), v));
        v
    }
}

impl Sampler for SystemSampler {
    fn sample(&self, ctx: &Ctx) -> Sample {
        let mut out = Sample::default();
        if let Some(m) = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| parse_meminfo(&t)) {
            (out.mem_percent, out.mem_ok) = (m, true);
        }
        if self.cpu {
            if let Some(c) = system_cpu_percent() {
                (out.cpu_percent, out.cpu_ok) = (c, true);
            }
        }
        if let Some(g) = self.gpu(ctx) {
            (out.gpu_percent, out.gpu_ok) = (g, true);
        }
        out
    }
}

/// Go's `nvidiaMemoryPercent`: nvidia-smi from PATH, 5 s at most, its answer bounded.
pub fn nvidia_memory_percent(ctx: &Ctx) -> Option<f64> {
    let bin = crate::tools::exec::look_path_go("nvidia-smi").ok()?;
    let args = ["--query-gpu=memory.used,memory.total".to_string(), "--format=csv,noheader,nounits".to_string()];
    let env = crate::tools::exec::parent_env();
    let mut out = Vec::new();
    let mut over = false;
    let mut sink = |stream: &'static str, b: &[u8]| {
        if stream != "stdout" {
            return;
        }
        let room = MAX_NVIDIA_OUTPUT - out.len();
        if b.len() > room {
            over = true;
        }
        out.extend_from_slice(&b[..b.len().min(room)]);
    };
    let st = crate::tools::exec::run_group(
        &bin,
        &args,
        &env,
        None,
        Instant::now() + Duration::from_secs(5),
        ctx,
        crate::tools::exec::GroupKill::Process,
        &mut sink,
    )
    .ok()?;
    if over || !st.status.is_some_and(|s| s.success()) || st.io_incomplete {
        return None;
    }
    parse_nvidia(&String::from_utf8_lossy(&out))
}

/// Busy and total jiffies from /proc/stat's first line.
fn cpu_times() -> Option<(f64, f64)> {
    let b = std::fs::read_to_string("/proc/stat").ok()?;
    let line = b.split('\n').next().unwrap_or("");
    let f = gotext::fields(line);
    if f.len() < 5 || f[0] != "cpu" {
        return None;
    }
    let vals: Vec<f64> = f[1..].iter().map(|s| float(s)).collect::<Option<_>>()?;
    let total: f64 = vals.iter().sum();
    let mut idle = vals[3];
    if vals.len() > 4 {
        idle += vals[4];
    }
    Some((total - idle, total))
}

fn system_cpu_percent() -> Option<f64> {
    let (b1, t1) = cpu_times()?;
    std::thread::sleep(Duration::from_millis(100));
    let (b2, t2) = cpu_times()?;
    if t2 <= t1 {
        return None;
    }
    Some((b2 - b1) / (t2 - t1) * 100.0)
}
