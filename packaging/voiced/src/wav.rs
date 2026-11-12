//! Wav files the way the Python engine read and wrote them with soundfile (libsndfile): the reference recording read as
//! 32-bit floats (all channels averaged, brought to 24 kHz with SciPy's `resample_poly` when it has another rate), and each
//! sentence written as 16-bit PCM, mono, 24 kHz, with libsndfile's 44-byte header and its conversion (`floor(x * 32768)`).

use std::f64::consts::PI;

pub const SR: u32 = 24000;

/// The reference recording: mono float32 samples at 24 kHz.
pub fn load_audio(path: &str) -> Result<Vec<f32>, String> {
    let data = std::fs::read(path).map_err(|e| format!("Error opening {:?}: {}", path, e))?;
    let (frames, channels, rate) = parse_wav(&data).map_err(|e| format!("Error opening {:?}: {}", path, e))?;
    let mono: Vec<f32> = if channels == 1 {
        frames
    } else {
        // a.mean(axis=1) in float32: the channels added in order, then divided by their number
        frames
            .chunks(channels)
            .map(|f| {
                let mut s = 0.0f32;
                for &x in f {
                    s += x;
                }
                s / channels as f32
            })
            .collect()
    };
    if rate == SR {
        return Ok(mono);
    }
    let g = gcd(rate as u64, SR as u64);
    let y = resample_poly(&mono.iter().map(|&x| x as f64).collect::<Vec<_>>(), SR as u64 / g, rate as u64 / g);
    Ok(y.into_iter().map(|x| x as f32).collect())
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// (interleaved samples as float32, channels, rate) of a RIFF/WAVE file: PCM 8/16/24/32-bit or IEEE float 32/64-bit,
/// plain or WAVE_FORMAT_EXTENSIBLE, scaled as libsndfile scales them (an integer sample divided by 2**(bits-1)).
fn parse_wav(d: &[u8]) -> Result<(Vec<f32>, usize, u32), String> {
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return Err("Format not recognised.".into());
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, usize, u32, u16)> = None;
    while pos + 8 <= d.len() {
        let id = &d[pos..pos + 4];
        let size = u32le(d, pos + 4) as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(d.len());
        if id == b"fmt " {
            let f = &d[body..end];
            if f.len() < 16 {
                return Err("bad fmt chunk".into());
            }
            let mut tag = u16le(f, 0);
            let channels = u16le(f, 2) as usize;
            let rate = u32le(f, 4);
            let bits = u16le(f, 14);
            if tag == 0xfffe {
                if f.len() < 26 {
                    return Err("bad fmt chunk".into());
                }
                tag = u16le(f, 24); // the sub-format's first two bytes: 1 PCM, 3 float
            }
            if channels == 0 {
                return Err("no channels".into());
            }
            fmt = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            let (tag, channels, rate, bits) = fmt.ok_or("data chunk before fmt chunk")?;
            let b = &d[body..end];
            let samples: Vec<f32> = match (tag, bits) {
                (1, 8) => b.iter().map(|&x| (x as i32 - 128) as f32 * (1.0 / 128.0)).collect(),
                (1, 16) => b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 * (1.0 / 32768.0)).collect(),
                (1, 24) => b.chunks_exact(3).map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) as f32) * (1.0 / 2147483648.0)).collect(),
                (1, 32) => b.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 * (1.0 / 2147483648.0)).collect(),
                (3, 32) => b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
                (3, 64) => b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap()) as f32).collect(),
                _ => return Err(format!("unsupported wav format {} with {} bits", tag, bits)),
            };
            let n = samples.len() / channels * channels;
            let mut samples = samples;
            samples.truncate(n);
            return Ok((samples, channels, rate));
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    Err("no data chunk".into())
}

/// The sentence as libsndfile writes it for soundfile.write(path, wav, 24000, subtype="PCM_16"): a 44-byte header and
/// each sample as floor(x * 32768), which is what soundfile's libsndfile (1.2.2) gives for every input measured (the
/// samples are within ±0.95, so nothing clips).
pub fn write_pcm16(path: &str, wav: &[f32]) -> std::io::Result<()> {
    let data_len = (wav.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + wav.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SR.to_le_bytes());
    out.extend_from_slice(&(SR * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &x in wav {
        let v = (x * 32768.0f32).floor();
        let v = v.clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, out)
}

/// SciPy's resample_poly(x, up, down) with its default Kaiser window (beta 5) and zero padding, in double precision.
pub fn resample_poly(x: &[f64], up: u64, down: u64) -> Vec<f64> {
    let g = gcd(up, down);
    let (up, down) = (up / g, down / g);
    if up == 1 && down == 1 {
        return x.to_vec();
    }
    let n_in = x.len() as u64;
    let n_out = n_in * up;
    let n_out = n_out / down + u64::from(n_out % down != 0);
    let max_rate = up.max(down);
    let f_c = 1.0 / max_rate as f64;
    let half_len = 10 * max_rate;
    let mut h = firwin_kaiser(2 * half_len as usize + 1, f_c, 5.0);
    for v in h.iter_mut() {
        *v *= up as f64;
    }
    let n_pre_pad = down - half_len % down;
    let mut n_post_pad = 0u64;
    let n_pre_remove = (half_len + n_pre_pad) / down;
    let out_len = |len_h: u64| ((n_in.max(1) - 1) * up + len_h - 1) / down + 1;
    while out_len(h.len() as u64 + n_pre_pad + n_post_pad) < n_out + n_pre_remove {
        n_post_pad += 1;
    }
    let mut hp = vec![0.0; n_pre_pad as usize];
    hp.extend_from_slice(&h);
    hp.extend(std::iter::repeat_n(0.0, n_post_pad as usize));
    // upfirdn: upsample by `up` (zeros between), filter with hp, keep every `down`-th sample
    let y_len = out_len(hp.len() as u64) as usize;
    let end = ((n_pre_remove + n_out) as usize).min(y_len);
    let mut y = Vec::with_capacity(end.saturating_sub(n_pre_remove as usize));
    for k in n_pre_remove as usize..end {
        let m = k as u64 * down; // position in the upsampled signal
        let mut s = 0.0;
        // x[i] sits at i*up; hp index j = m - i*up
        let i_hi = (m / up).min(n_in.saturating_sub(1));
        let i_lo = if m + 1 > hp.len() as u64 { (m + 1 - hp.len() as u64).div_ceil(up) } else { 0 };
        let mut i = i_lo;
        while i <= i_hi && n_in > 0 {
            s += hp[(m - i * up) as usize] * x[i as usize];
            i += 1;
        }
        y.push(s);
    }
    y
}

/// scipy.signal.firwin(numtaps, cutoff, window=("kaiser", beta)): a low-pass windowed sinc, scaled to a gain of 1 at 0 Hz.
fn firwin_kaiser(numtaps: usize, cutoff: f64, beta: f64) -> Vec<f64> {
    let alpha = 0.5 * (numtaps as f64 - 1.0);
    let sinc = |x: f64| if x == 0.0 { 1.0 } else { (PI * x).sin() / (PI * x) };
    let mut h: Vec<f64> = (0..numtaps)
        .map(|n| {
            let m = n as f64 - alpha;
            cutoff * sinc(cutoff * m)
        })
        .collect();
    let i0b = bessel_i0(beta);
    for (n, v) in h.iter_mut().enumerate() {
        let r = (n as f64 - alpha) / alpha;
        *v *= bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
    }
    let s: f64 = h.iter().sum();
    for v in h.iter_mut() {
        *v /= s;
    }
    h
}

/// The modified Bessel function of the first kind, order 0 (power series; converges quickly for the arguments used).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut term, mut sum) = (1.0f64, 1.0f64);
    for k in 1..200 {
        term *= q / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}
