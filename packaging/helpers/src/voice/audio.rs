//! Push-to-talk capture and playback through ALSA's command-line tools (`arecord`, `aplay` from alsa-utils), as the
//! Python's `audio_io.py`: a terminal reports whole lines, not key presses, so push-to-talk is Enter to start (the caller)
//! and Enter to stop; a timer stops a recording nobody stops.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// One rate through the whole pipeline (whisper.cpp wants 16 kHz).
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// `shutil.which(name)`: the first executable file of that name on PATH.
pub fn which(name: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    if name.contains('/') {
        return std::fs::metadata(name).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).then(|| name.to_string());
    }
    let path = std::env::var("PATH").unwrap_or_else(|_| "/bin:/usr/bin".into());
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = format!("{dir}/{name}");
        if std::fs::metadata(&p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
            return Some(p);
        }
    }
    None
}

pub struct Alsa {
    pub arecord: String,
    pub aplay: String,
}

impl Alsa {
    /// Both tools, looked up once (as the Python did when it was imported).
    pub fn find() -> Option<Alsa> {
        Some(Alsa { arecord: which("arecord")?, aplay: which("aplay")? })
    }
}

/// How a recording ended for the caller: the stop line was read, or reading it failed.
pub enum Stop {
    Line,
    Eof,
    Interrupt,
}

/// Records one utterance into `wav_path`: start now, stop when `wait_stop` returns (or after `max_seconds`).
pub fn record(
    alsa: &Alsa,
    wav_path: &std::path::Path,
    device: Option<&str>,
    max_seconds: f64,
    echo: &mut dyn FnMut(&str),
    wait_stop: &mut dyn FnMut() -> Stop,
) -> Result<Stop, String> {
    let mut cmd = Command::new(&alsa.arecord);
    cmd.args(["-q", "-f", "S16_LE", "-r", &DEFAULT_SAMPLE_RATE.to_string(), "-c", "1", "-t", "wav"]);
    if let Some(d) = device.filter(|d| !d.is_empty()) {
        cmd.args(["-D", d]);
    }
    cmd.arg(wav_path);
    let child = Arc::new(Mutex::new(cmd.spawn().map_err(|e| crate::voice::os_error(&e, &alsa.arecord))?));
    let (cancel, cancelled) = mpsc::channel::<()>();
    let timer_child = child.clone();
    // threading.Timer(max_seconds, proc.terminate): a negative or zero wait fires at once, no finite number means no timer
    let timer = (!max_seconds.is_nan()).then(|| {
        std::thread::spawn(move || {
            let wait = if max_seconds.is_finite() { Some(Duration::from_secs_f64(max_seconds.clamp(0.0, 1e9))) } else { None };
            let fired = match wait {
                Some(w) => matches!(cancelled.recv_timeout(w), Err(mpsc::RecvTimeoutError::Timeout)),
                None => false,
            };
            if fired {
                terminate(&mut timer_child.lock().unwrap_or_else(|e| e.into_inner()));
            }
        })
    });
    echo("Recording... press Enter to stop.");
    let how = wait_stop();
    let _ = cancel.send(());
    if let Some(t) = timer {
        let _ = t.join();
    }
    let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
    terminate(&mut c);
    let _ = c.wait();
    Ok(how)
}

/// `Popen.terminate()` if it still runs (SIGTERM; arecord then finishes the file's header).
fn terminate(c: &mut Child) {
    if let Ok(None) = c.try_wait() {
        unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
    }
}

/// `play_audio`: a complete WAV file's bytes through aplay; its error output when it fails.
pub fn play(alsa: &Alsa, wav: &[u8], device: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new(&alsa.aplay);
    cmd.arg("-q");
    if let Some(d) = device.filter(|d| !d.is_empty()) {
        cmd.args(["-D", d]);
    }
    cmd.arg("-");
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| crate::voice::os_error(&e, &alsa.aplay))?;
    let mut stdin = child.stdin.take();
    let data = wav.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(w) = stdin.as_mut() {
            let _ = w.write_all(&data);
        }
    });
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(format!("aplay failed: {}", sarina_voice::py::strip(&String::from_utf8_lossy(&out.stderr))));
    }
    Ok(())
}
