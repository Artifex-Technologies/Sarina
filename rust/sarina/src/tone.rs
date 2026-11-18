//! Go: `go/tone` (with the pieces of `go/cmd/sarina-tone` that touch files). How something was said, not what: loudness,
//! pitch movement, speaking pace and pauses of one spoken question, compared with how this person usually sounds, to
//! give Sarina a quiet hint ("a bit rushed", "sounds tired") that only changes her style. Plain signal processing, no
//! model. The running averages (a handful of numbers) are one private file; the moods heard (a time and one word each)
//! another. Proved against the Go package on testdata/parity/tone.json and, as a whole program, by
//! rust/parity/tone-differential.py.
//!
//! Errors are bytes: a path that is not UTF-8 is named in them byte for byte, as Go names it.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

use crate::gojson;
use crate::gomath;
use crate::gopath::{self, os_path};
use crate::gostruct::{self, Field, Ty, Val};
use crate::tz::Location;

/// Features of one recording.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Features {
    /// mean level of the speaking frames, dB below full scale
    pub loud_db: f64,
    /// Hz
    pub f0_median: f64,
    /// spread of pitch (10th to 90th percentile) in semitones
    pub f0_range_st: f64,
    /// syllable-like beats per second of speech
    pub rate: f64,
    /// time in pauses (250 ms or more) over speaking time
    pub pause_ratio: f64,
    pub voiced_sec: f64,
    /// zero crossings per second over the speaking frames (median): how bright the voice sounds
    pub bright: f64,
}

impl Features {
    /// Go's `vec()`: the measures the baseline keeps, by name.
    fn vec(&self) -> [(&'static str, f64); 6] {
        [
            ("loud", self.loud_db),
            ("f0", self.f0_median),
            ("range", self.f0_range_st),
            ("rate", self.rate),
            ("pause", self.pause_ratio),
            ("bright", self.bright),
        ]
    }

    /// Go's `json.Marshal(f)`: the struct's fields in their order.
    pub fn json(&self) -> Result<String, gojson::Error> {
        let mut out = String::from("{");
        let fields = [
            ("loud_db", self.loud_db),
            ("f0_median", self.f0_median),
            ("f0_range_st", self.f0_range_st),
            ("rate", self.rate),
            ("pause_ratio", self.pause_ratio),
            ("voiced_sec", self.voiced_sec),
            ("bright", self.bright),
        ];
        for (i, (k, v)) in fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("\"{k}\":"));
            gojson::write_f64(&mut out, *v)?;
        }
        out.push('}');
        Ok(out)
    }
}

/// Go's `math.NaN()`.
const GO_NAN: f64 = f64::from_bits(0x7FF8_0000_0000_0001);

/// Go's `math.Max`: +Inf wins, then NaN (Go's own), then +0 over -0.
fn go_max(x: f64, y: f64) -> f64 {
    if x == f64::INFINITY || y == f64::INFINITY {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return GO_NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { y } else { x };
    }
    if x > y {
        x
    } else {
        y
    }
}

/// Go's `math.Min`: -Inf wins, then NaN (Go's own), then -0 over +0.
fn go_min(x: f64, y: f64) -> f64 {
    if x == f64::NEG_INFINITY || y == f64::NEG_INFINITY {
        return f64::NEG_INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return GO_NAN;
    }
    if x == 0.0 && x == y {
        return if x.is_sign_negative() { x } else { y };
    }
    if x < y {
        x
    } else {
        y
    }
}

/// Go's `math.Log10`: Log times 1/Ln10 (Go's `Log` as amd64 runs it).
fn log10(x: f64) -> f64 {
    gomath::log(x) * std::f64::consts::LOG10_E
}

/// Go's `sort.Float64s` on values that are never NaN.
fn sort_floats(v: &mut [f64]) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
}

/// Go's `*os.PathError` text, the path byte for byte.
fn path_err(op: &str, path: &[u8], e: &std::io::Error) -> Vec<u8> {
    let mut m = format!("{op} ").into_bytes();
    m.extend_from_slice(path);
    m.extend_from_slice(b": ");
    m.extend_from_slice(crate::goerr::text(e).as_bytes());
    m
}

/// Go's `os.ReadFile`: "open" when the file cannot be opened, "read" when it cannot be read.
fn read_file(path: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    let mut f = std::fs::File::open(os_path(path)).map_err(|e| path_err("open", path, &e))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|e| path_err("read", path, &e))?;
    Ok(b)
}

fn u16le(b: &[u8], at: usize) -> i64 {
    i64::from(u16::from_le_bytes([b[at], b[at + 1]]))
}

fn u32le(b: &[u8], at: usize) -> i64 {
    i64::from(u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))
}

/// Go's `ReadWAV`: 16-bit PCM (mono, or the channels mixed down) as samples in -1..1, and the sample rate.
pub fn read_wav(path: &[u8]) -> Result<(Vec<f64>, i64), Vec<u8>> {
    parse_wav(&read_file(path)?)
}

/// ReadWAV's reading of a file's bytes.
pub fn parse_wav(b: &[u8]) -> Result<(Vec<f64>, i64), Vec<u8>> {
    if b.len() < 44 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(b"not a WAV file".to_vec());
    }
    let (mut rate, mut channels, mut bits) = (0i64, 0i64, 0i64);
    let mut pos = 12usize;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let mut size = u32le(b, pos + 4) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            channels = u16le(b, body + 2);
            rate = u32le(b, body + 4);
            bits = u16le(b, body + 14);
        }
        if id == b"data" {
            if size == 0 || body + size > b.len() {
                // a recording still being written, or a streamed file: take what is there
                size = b.len() - body;
            }
            if bits != 16 || channels < 1 || rate <= 0 {
                return Err(format!("only 16-bit PCM is read (got {bits} bits, {channels} channels)").into_bytes());
            }
            let ch = channels as usize;
            let n = size / 2 / ch;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let mut s = 0.0f64;
                for c in 0..ch {
                    let at = body + (i * ch + c) * 2;
                    s += f64::from(i16::from_le_bytes([b[at], b[at + 1]]));
                }
                out.push(s / channels as f64 / 32768.0);
            }
            return Ok((out, rate));
        }
        pos = body + size + size % 2;
    }
    Err(b"no audio data".to_vec())
}

/// Go's `Analyze`: the measures of one recording, or None when there is too little speech to say anything (under about a
/// second and a half).
pub fn analyze(x: &[f64], rate: i64) -> Option<Features> {
    let mut f = Features::default();
    let (frame, hop) = (rate * 25 / 1000, rate * 10 / 1000);
    if frame < 16 || (x.len() as i64) < frame * 10 {
        return None;
    }
    let (frame, hop) = (frame as usize, hop as usize);
    let mut rms = Vec::new();
    let mut i = 0usize;
    while i + frame <= x.len() {
        let mut s = 0.0f64;
        for v in &x[i..i + frame] {
            s += v * v;
        }
        rms.push((s / frame as f64).sqrt());
        i += hop;
    }
    let db: Vec<f64> = rms.iter().map(|&r| 20.0 * log10(go_max(r, 1e-6))).collect();
    let mut sorted = db.clone();
    sort_floats(&mut sorted);
    let floor = sorted[sorted.len() / 10];
    let peak = sorted[sorted.len() * 95 / 100];
    if peak - floor < 12.0 {
        return None; // nothing stands out from the room: no speech
    }
    let thr = go_max(floor + 9.0, peak - 28.0);
    let mut voiced = vec![false; db.len()];
    let mut nv = 0usize;
    let mut sum_db = 0.0f64;
    for (i, &d) in db.iter().enumerate() {
        if d > thr {
            voiced[i] = true;
            nv += 1;
            sum_db += d;
        }
    }
    f.voiced_sec = (nv * hop) as f64 / rate as f64;
    if f.voiced_sec < 1.2 {
        return None;
    }
    f.loud_db = sum_db / nv as f64;

    let mut zc = Vec::new();
    for (i, &v) in voiced.iter().enumerate() {
        if !v || i * hop + frame > x.len() {
            continue;
        }
        let seg = &x[i * hop..i * hop + frame];
        let n = (1..seg.len()).filter(|&k| (seg[k - 1] < 0.0) != (seg[k] < 0.0)).count();
        zc.push(n as f64 * rate as f64 / frame as f64);
    }
    if !zc.is_empty() {
        sort_floats(&mut zc);
        f.bright = zc[zc.len() / 2];
    }

    // pauses: gaps of 250 ms or more between speaking frames, inside the speech (not the quiet before and after)
    let first = voiced.iter().position(|&v| v)?;
    let last = voiced.iter().rposition(|&v| v)?;
    let min_gap = (250 * rate / 1000 / hop as i64) as usize;
    let (mut pause_frames, mut run) = (0usize, 0usize);
    for &v in &voiced[first..=last] {
        if v {
            if run >= min_gap {
                pause_frames += run;
            }
            run = 0;
        } else {
            run += 1;
        }
    }
    let span = (last - first + 1) as f64;
    f.pause_ratio = pause_frames as f64 / go_max(span - pause_frames as f64, 1.0);

    // pace: peaks in the smoothed level (one per syllable, roughly)
    let sm: Vec<f64> = (0..db.len())
        .map(|i| {
            let (mut s, mut n) = (0.0f64, 0usize);
            for j in i as i64 - 2..=i as i64 + 2 {
                if j >= 0 && (j as usize) < db.len() {
                    s += db[j as usize];
                    n += 1;
                }
            }
            s / n as f64
        })
        .collect();
    let mut peaks = 0usize;
    let mut last_peak: i64 = -100;
    for i in 2..sm.len().saturating_sub(2) {
        if voiced[i] && sm[i] > sm[i - 1] && sm[i] >= sm[i + 1] && sm[i] > thr + 3.0 && i as i64 - last_peak >= 8 {
            // it must rise clearly above the dip before it (a syllable, not wobble)
            let mut lo = sm[i];
            for j in (i.saturating_sub(12)..i).rev() {
                lo = go_min(lo, sm[j]);
            }
            if sm[i] - lo > 2.5 {
                peaks += 1;
                last_peak = i as i64;
            }
        }
    }
    f.rate = peaks as f64 / ((last - first + 1 - pause_frames) as f64 * hop as f64 / rate as f64);

    // pitch: autocorrelation over the speaking frames, 75 to 350 Hz
    let (lag_min, lag_max) = ((rate / 350) as usize, (rate / 75) as usize);
    let mut f0s = Vec::new();
    for i in first..=last {
        if !voiced[i] || i % 2 == 1 {
            continue;
        }
        if i * hop + frame > x.len() || frame <= lag_max {
            continue;
        }
        let seg = &x[i * hop..i * hop + frame];
        let mut e0 = 0.0f64;
        for v in seg {
            e0 += v * v;
        }
        if e0 < 1e-9 {
            continue;
        }
        let (mut best, mut best_lag) = (0.0f64, 0usize);
        let mut lag = lag_min;
        while lag <= lag_max && lag < frame / 2 + frame / 4 {
            let mut s = 0.0f64;
            for k in 0..frame - lag {
                s += seg[k] * seg[k + lag];
            }
            let c = s / e0;
            if c > best {
                best = c;
                best_lag = lag;
            }
            lag += 1;
        }
        if best >= 0.4 && best_lag > 0 {
            f0s.push(rate as f64 / best_lag as f64);
        }
    }
    if f0s.len() < 15 {
        return None; // too little voiced speech to know the pitch
    }
    sort_floats(&mut f0s);
    f.f0_median = f0s[f0s.len() / 2];
    let (lo, hi) = (f0s[f0s.len() / 10], f0s[f0s.len() * 9 / 10]);
    f.f0_range_st = 12.0 * gomath::log2(hi / lo);
    Some(f)
}

/// How many recordings must have been heard before any hint is given.
pub const MIN_SAMPLES: i64 = 12;

/// Moods a recording can be heard as, against the speaker's own usual.
pub const USUAL: &str = "usual";
pub const TENSE: &str = "tense";
pub const UPBEAT: &str = "upbeat";
pub const LOW: &str = "low";

/// How this person usually sounds: running averages that follow slow changes. A map Go keeps nil is None here (it is
/// written as null, and a nil count map is filled from the means).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Baseline {
    pub n: i64,
    pub mean: Option<BTreeMap<String, f64>>,
    pub var: Option<BTreeMap<String, f64>>,
    pub count: Option<BTreeMap<String, i64>>,
}

static FLOATS: Ty = Ty::Map(&Ty::Float);
static INTS: Ty = Ty::Map(&Ty::Int);
static BASELINE: Ty =
    Ty::Struct("tone.Baseline", "Baseline", &[Field("n", &Ty::Int), Field("mean", &FLOATS), Field("var", &FLOATS), Field("count", &INTS)]);
static ENTRY: Ty = Ty::Struct("tone.Entry", "Entry", &[Field("t", &Ty::Int), Field("mood", &Ty::Str)]);

fn min_sd(k: &str) -> f64 {
    match k {
        "loud" => 2.0,
        "f0" => 12.0,
        "range" => 1.5,
        "rate" => 0.6,
        "pause" => 0.08,
        "bright" => 250.0,
        _ => 0.0,
    }
}

fn get<T: Copy + Default>(m: &Option<BTreeMap<String, T>>, k: &str) -> T {
    m.as_ref().and_then(|m| m.get(k).copied()).unwrap_or_default()
}

impl Baseline {
    /// Go's `Update`: one recording folded in.
    pub fn update(&mut self, f: &Features) {
        if self.mean.is_none() {
            self.mean = Some(BTreeMap::new());
            self.var = Some(BTreeMap::new());
        } else if self.var.is_none() {
            // a file with means but no variances: they start at zero (Go stopped on the nil map until 2026-11-18)
            self.var = Some(BTreeMap::new());
        }
        self.fill_count();
        self.n = self.n.wrapping_add(1);
        let (mean, var, count) = (self.mean.as_mut().unwrap(), self.var.as_mut().unwrap(), self.count.as_mut().unwrap());
        for (k, v) in f.vec() {
            let n = count.get(k).copied().unwrap_or(0).wrapping_add(1);
            count.insert(k.to_string(), n);
            if n == 1 {
                mean.insert(k.to_string(), v);
                var.insert(k.to_string(), 0.0);
                continue;
            }
            let alpha = go_max(1.0 / n as f64, 0.03);
            let m = mean.get(k).copied().unwrap_or(0.0);
            let d = v - m;
            mean.insert(k.to_string(), m + alpha * d);
            let w = var.get(k).copied().unwrap_or(0.0);
            var.insert(k.to_string(), (1.0 - alpha) * (w + alpha * d * d));
        }
    }

    /// Go's `fillCount`: a baseline saved before counts existed has every measure it has a mean for seen N times.
    fn fill_count(&mut self) {
        if self.count.is_none() {
            let n = self.n;
            self.count = Some(self.mean.iter().flat_map(|m| m.keys()).map(|k| (k.clone(), n)).collect());
        }
    }

    /// Go's `Z`: how far each measure is from the usual, in its own standard deviations (0 with too little history).
    pub fn z(&mut self, f: &Features) -> BTreeMap<&'static str, f64> {
        self.fill_count();
        let mut z = BTreeMap::new();
        for (k, v) in f.vec() {
            if get(&self.count, k) < MIN_SAMPLES {
                z.insert(k, 0.0);
                continue;
            }
            let sd = go_max(get(&self.var, k).sqrt(), min_sd(k));
            z.insert(k, (v - get(&self.mean, k)) / sd);
        }
        z
    }

    /// Go's `Mood`: how this recording sounded next to the usual, or "" while there is too little history.
    pub fn mood(&mut self, f: &Features) -> &'static str {
        if self.n < MIN_SAMPLES {
            return "";
        }
        let z = self.z(f);
        let mut keys = vec!["loud", "rate", "range", "f0"];
        if get(&self.count, "bright") >= MIN_SAMPLES {
            keys.push("bright");
        }
        let mut arousal = 0.0f64;
        for k in &keys {
            arousal += z[k];
        }
        arousal /= keys.len() as f64;
        if z["rate"] >= 1.0 && z["pause"] <= -0.3 && z["range"] < 0.6 {
            TENSE
        } else if arousal >= 0.8 && z["pause"] <= 0.4 {
            UPBEAT
        } else if arousal <= -0.8 && z["pause"] >= 0.3 {
            LOW
        } else {
            USUAL
        }
    }

    /// Go's `json.Marshal(b)`.
    pub fn json(&self) -> Result<String, gojson::Error> {
        fn map<T: Copy>(
            out: &mut String,
            m: &Option<BTreeMap<String, T>>,
            w: impl Fn(&mut String, T) -> Result<(), gojson::Error>,
        ) -> Result<(), gojson::Error> {
            let Some(m) = m else {
                out.push_str("null");
                return Ok(());
            };
            out.push('{');
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                gojson::write_str(out, k, true);
                out.push(':');
                w(out, *v)?;
            }
            out.push('}');
            Ok(())
        }
        let mut out = format!("{{\"n\":{},\"mean\":", self.n);
        map(&mut out, &self.mean, gojson::write_f64)?;
        out.push_str(",\"var\":");
        map(&mut out, &self.var, gojson::write_f64)?;
        if self.count.as_ref().is_some_and(|c| !c.is_empty()) {
            out.push_str(",\"count\":");
            map(&mut out, &self.count, |o, v| {
                o.push_str(&v.to_string());
                Ok(())
            })?;
        }
        out.push('}');
        Ok(out)
    }

    /// Go's `Save`: written privately (owner only) through a temporary file. A baseline JSON cannot hold (a value grown
    /// past float64) is written as an empty file, as Go writes what its ignored Marshal error left.
    pub fn save(&self, path: &[u8]) -> Result<(), Vec<u8>> {
        testguard(path)?;
        mkdir_all(&gopath::dir(path), 0o700)?;
        let raw = self.json().map(String::into_bytes).unwrap_or_default();
        let tmp = [path, b".tmp"].concat();
        write_file(&tmp, &raw)?;
        rename(&tmp, path)
    }
}

/// Go's `HintFor`: the style note for a mood ("" for none).
pub fn hint_for(mood: &str) -> &'static str {
    match mood {
        TENSE => "The owner sounds rushed or tense: be brief and direct, no pleasantries; if it is not about a task, a calm word helps more than a joke.",
        UPBEAT => "The owner sounds animated or in a good mood: match that energy, and a light joke or a callback would land.",
        LOW => "The owner sounds low or tired: be warm and brief, no jokes unless they make one; if the moment fits you may ask once how they are doing, without saying how they sound.",
        _ => "",
    }
}

fn val_floats(v: Option<&Val>) -> Option<BTreeMap<String, f64>> {
    match v {
        Some(Val::Map(Some(m))) => Some(m.iter().map(|(k, x)| (k.clone(), if let Val::Float(f) = x { *f } else { 0.0 })).collect()),
        _ => None,
    }
}

/// Go's `Load`: the baseline file as Go's `json.Unmarshal` reads it (as far as it reads), or an empty one.
pub fn load(path: &[u8]) -> Baseline {
    match std::fs::read(os_path(path)) {
        Ok(raw) => parse_baseline(&raw),
        Err(_) => Baseline::default(),
    }
}

/// Load's reading of a file's bytes.
pub fn parse_baseline(raw: &[u8]) -> Baseline {
    let (v, _) = gostruct::unmarshal(&BASELINE, raw);
    let n = match gostruct::field(&BASELINE, &v, "n") {
        Some(Val::Int(n)) => *n,
        _ => 0,
    };
    let count = match gostruct::field(&BASELINE, &v, "count") {
        Some(Val::Map(Some(m))) => Some(m.iter().map(|(k, x)| (k.clone(), if let Val::Int(i) = x { *i } else { 0 })).collect()),
        _ => None,
    };
    Baseline { n, mean: val_floats(gostruct::field(&BASELINE, &v, "mean")), var: val_floats(gostruct::field(&BASELINE, &v, "var")), count }
}

// ---- files, as Go's os package writes them ----

/// `testguard.Allow` with Go's message naming the path byte for byte.
fn testguard(path: &[u8]) -> Result<(), Vec<u8>> {
    if crate::testguard::allow(path).is_ok() {
        return Ok(());
    }
    let mut m = format!("not written: this is a test run ({}=1), and ", crate::testguard::ENV).into_bytes();
    m.extend_from_slice(path);
    m.extend_from_slice(b" is outside the test's folders");
    Err(m)
}

/// Go's `os.MkdirAll(path, perm)`: the parent is the path up to its last separator (as written, not cleaned), a file in
/// the way is ENOTDIR, a folder made meanwhile is fine.
fn mkdir_all(path: &[u8], perm: u32) -> Result<(), Vec<u8>> {
    if let Ok(m) = std::fs::metadata(os_path(path)) {
        if m.is_dir() {
            return Ok(());
        }
        return Err(path_err("mkdir", path, &std::io::Error::from_raw_os_error(libc::ENOTDIR)));
    }
    let mut i = path.len();
    while i > 0 && path[i - 1] == b'/' {
        i -= 1;
    }
    while i > 0 && path[i - 1] != b'/' {
        i -= 1;
    }
    // Go: path[:i], i the index of that separator (0 when there is none)
    let parent = &path[..i.saturating_sub(1)];
    if !parent.is_empty() {
        mkdir_all(parent, perm)?;
    }
    match std::fs::DirBuilder::new().mode(perm).create(os_path(path)) {
        Ok(()) => Ok(()),
        Err(e) => match std::fs::symlink_metadata(os_path(path)) {
            Ok(m) if m.is_dir() => Ok(()),
            _ => Err(path_err("mkdir", path, &e)),
        },
    }
}

/// Go's `os.WriteFile(path, data, 0o600)`.
fn write_file(path: &[u8], data: &[u8]) -> Result<(), Vec<u8>> {
    let mut f = open_tmp(path)?;
    f.write_all(data).map_err(|e| path_err("write", path, &e))?;
    // Go's WriteFile reports a failed close (its write error first)
    close(f, path)
}

/// Go's `os.OpenFile(path, O_CREATE|O_TRUNC|O_WRONLY, 0o600)`: an existing file keeps its mode.
fn open_tmp(path: &[u8]) -> Result<std::fs::File, Vec<u8>> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(os_path(path))
        .map_err(|e| path_err("open", path, &e))
}

/// Go's `f.Close()` with its error ("close PATH: ...").
fn close(f: std::fs::File, path: &[u8]) -> Result<(), Vec<u8>> {
    use std::os::fd::IntoRawFd;
    let fd = f.into_raw_fd();
    // SAFETY: the descriptor is ours (taken from the File) and closed once
    if unsafe { libc::close(fd) } != 0 {
        return Err(path_err("close", path, &std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Go's `os.Rename` on Linux: onto a folder it is refused as "file exists" (Go's own check before the call), else
/// rename(2); the error names both paths.
fn rename(from: &[u8], to: &[u8]) -> Result<(), Vec<u8>> {
    let link_err = |e: &std::io::Error| {
        let mut m = b"rename ".to_vec();
        m.extend_from_slice(from);
        m.push(b' ');
        m.extend_from_slice(to);
        m.extend_from_slice(b": ");
        m.extend_from_slice(crate::goerr::text(e).as_bytes());
        m
    };
    if let Ok(fi) = std::fs::symlink_metadata(os_path(to)) {
        if fi.is_dir() {
            match std::fs::symlink_metadata(os_path(from)) {
                Err(e) => return Err(link_err(&e)),
                Ok(ofi) => {
                    if from == to || !(fi.dev() == ofi.dev() && fi.ino() == ofi.ino()) {
                        return Err(link_err(&std::io::Error::from_raw_os_error(libc::EEXIST)));
                    }
                }
            }
        }
    }
    std::fs::rename(os_path(from), os_path(to)).map_err(|e| link_err(&e))
}

/// Go's `os.Remove`: unlink, then rmdir; when both fail, rmdir's error unless it was ENOTDIR.
pub fn remove(path: &[u8]) -> Result<(), (Vec<u8>, bool)> {
    let e = match std::fs::remove_file(os_path(path)) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    let e1 = match std::fs::remove_dir(os_path(path)) {
        Ok(()) => return Ok(()),
        Err(e1) => e1,
    };
    let e = if e1.raw_os_error() != Some(libc::ENOTDIR) { e1 } else { e };
    let not_exist = e.raw_os_error() == Some(libc::ENOENT);
    Err((path_err("remove", path, &e), not_exist))
}

// ---- the mood history ----

/// One heard question: when (Unix seconds) and the mood it was heard as.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub t: i64,
    pub mood: String,
}

/// How many entries the file keeps (a few months of daily use).
const KEEP_ENTRIES: usize = 600;

/// bufio.Scanner's longest line (its default buffer, 64 KiB): a longer one stops the reading there.
const MAX_LINE: usize = 64 * 1024;

/// Go's `ReadHistory`: the entries in the file, oldest first, read line by line as Go's `bufio.Scanner` reads them (a
/// line longer than 64 KiB ends the reading; a CR before the newline is dropped); none when the file cannot be read.
pub fn read_history(path: &[u8]) -> Vec<Entry> {
    let Ok(mut f) = std::fs::File::open(os_path(path)) else { return Vec::new() };
    let mut data = Vec::new();
    if f.read_to_end(&mut data).is_err() {
        // the scanner stops at the first error; what came before is kept (a read fails at once on a folder)
        return Vec::new();
    }
    parse_history(&data)
}

/// ReadHistory's reading of a file's bytes.
pub fn parse_history(data: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let (line, next) = match rest.iter().position(|&c| c == b'\n') {
            Some(j) if j < MAX_LINE => (&rest[..j], &rest[j + 1..]),
            None if rest.len() < MAX_LINE => (rest, &rest[rest.len()..]),
            _ => break, // bufio.ErrTooLong
        };
        rest = next;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let (v, err) = gostruct::unmarshal(&ENTRY, line);
        if err.is_some() {
            continue;
        }
        let t = match gostruct::field(&ENTRY, &v, "t") {
            Some(Val::Int(t)) => *t,
            _ => 0,
        };
        let mood = gostruct::str_field(&ENTRY, &v, "mood");
        if t > 0 && !mood.is_empty() {
            out.push(Entry { t, mood: mood.to_string() });
        }
    }
    out
}

/// Go's `AppendHistory`: one entry added, only the newest 600 kept, written privately (owner only) through a temporary file.
pub fn append_history(path: &[u8], e: Entry) -> Result<(), Vec<u8>> {
    testguard(path)?;
    let mut h = read_history(path);
    h.push(e);
    if h.len() > KEEP_ENTRIES {
        h.drain(..h.len() - KEEP_ENTRIES);
    }
    mkdir_all(&gopath::dir(path), 0o700)?;
    let tmp = [path, b".tmp"].concat();
    let mut f = open_tmp(&tmp)?;
    let text = history_text(&h);
    // Go writes through a bufio.Writer and reports its first write error at Flush
    if let Err(err) = f.write_all(text.as_bytes()) {
        return Err(path_err("write", &tmp, &err));
    }
    close(f, &tmp)?;
    rename(&tmp, path)
}

/// The history file's text: one `{"t":..,"mood":".."}` line per entry, as Go's json.Marshal writes it.
pub fn history_text(h: &[Entry]) -> String {
    let mut text = String::new();
    for x in h {
        text.push_str(&format!("{{\"t\":{},\"mood\":", x.t));
        gojson::write_str(&mut text, &x.mood, true);
        text.push_str("}\n");
    }
    text
}

#[derive(Default)]
struct Count {
    n: i64,
    m: BTreeMap<String, i64>,
}

impl Count {
    fn add(&mut self, mood: &str) {
        self.n += 1;
        *self.m.entry(mood.to_string()).or_default() += 1;
    }
    fn of(&self, mood: &str) -> i64 {
        self.m.get(mood).copied().unwrap_or(0)
    }
}

fn words(mood: &str) -> &str {
    match mood {
        LOW => "low or tired",
        TENSE => "rushed or tense",
        UPBEAT => "in good spirits",
        m => m,
    }
}

/// Go's `time.Date(y, m, d, 0, 0, 0, 0, loc).Unix()`: the zone's offset at the wall time read as UTC, and when it has
/// one, the offset at the instant that gives (Go's start/end check comes to the same).
fn midnight(loc: &Location, y: i64, m: i64, d: i64) -> i64 {
    let unix = crate::gotime::days_from_civil(y, m as u32, d as u32) * 86_400;
    let off = loc.lookup(unix).1;
    if off == 0 {
        return unix;
    }
    unix - loc.lookup(unix - off).1
}

/// The civil date and hour of a Unix time in a zone.
fn civil(loc: &Location, sec: i64) -> (i64, i64, i64, i64) {
    let t = crate::gotime::Time::from_unix(sec + loc.lookup(sec).1, 0);
    let (y, m, d, h, _, _, _) = t.civil();
    (y, m, d, h)
}

/// Go's `Pattern(h, now)` with `now` = (Unix seconds, nanoseconds) in `loc`: one or two sentences about how the owner has
/// sounded lately, or "".
pub fn pattern(h: &[Entry], now_sec: i64, now_nanos: u32, loc: &Location) -> String {
    let (mut week, mut prev, mut today) = (Count::default(), Count::default(), Count::default());
    let mut evening: BTreeMap<&str, i64> = BTreeMap::new();
    let (y, m, d, _) = civil(loc, now_sec);
    let mid = midnight(loc, y, m, d);
    const DAY: i128 = 86_400 * 1_000_000_000;
    for e in h {
        // now.Sub(t) in nanoseconds (Go saturates where this cannot overflow; the comparisons below come out the same)
        let age = (i128::from(now_sec) - i128::from(e.t)) * 1_000_000_000 + i128::from(now_nanos);
        if age < 0 {
            continue;
        } else if age < 7 * DAY {
            week.add(&e.mood);
            if e.t >= mid {
                today.add(&e.mood);
            }
            let hr = civil(loc, e.t).3;
            if !(4..20).contains(&hr) {
                *evening.entry(&e.mood).or_default() += 1;
            }
        } else if age < 14 * DAY {
            prev.add(&e.mood);
        }
    }
    let mut out: Vec<String> = Vec::new();
    if today.n >= 3 {
        for mood in [LOW, TENSE] {
            if today.of(mood) * 2 > today.n {
                out.push(format!("Today they have sounded {} in {} of {} questions.", words(mood), today.of(mood), today.n));
                break;
            }
        }
    }
    if week.n >= 8 {
        for mood in [LOW, TENSE, UPBEAT] {
            let k = week.of(mood);
            let share = k as f64 / week.n as f64;
            if k < 3 || share < 0.25 {
                continue;
            }
            let mut s = format!("This week they have sounded {} in {} of {} questions", words(mood), k, week.n);
            if prev.n >= 8 {
                let was = prev.of(mood) as f64 / prev.n as f64;
                if share >= 2.0 * was {
                    s += &format!(", more than the week before ({} of {})", prev.of(mood), prev.n);
                }
            }
            if mood != UPBEAT && evening.get(mood).copied().unwrap_or(0) * 10 >= k * 6 {
                s += ", mostly late in the day";
            }
            out.push(s + ".");
            if out.len() == 2 {
                break;
            }
        }
    }
    out.join(" ")
}
