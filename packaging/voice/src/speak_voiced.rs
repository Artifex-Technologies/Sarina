//! labyrinth-sarina-speak-voiced TEXT: say TEXT with the voice service (labyrinth-voiced), sentence by sentence.
//!
//! Each sentence arrives as a wav file while the next is still being made: this plays it, writes the loudness of the speech for
//! the panel's voice island (speech.env: when playing starts, then one number 0 to 100 per 50 ms), and deletes it. Exit 0 if
//! something was said; 1 if the service could not be reached or sent nothing (the caller then falls back to the Piper voice);
//! 2 without TEXT. A port of the Python script of the same name in Labyrinth_OS.

use crate::json::{self, Value};
use crate::proc;
use crate::py::{self, Crash, Env, PyResult};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

extern "C" {
    fn pow(x: f64, y: f64) -> f64;
}

/// What can end the loop: an OSError or ValueError (the script catches them and stops), or anything else (it ends the program).
enum Stop {
    Caught,
    Crash(Crash),
}

impl From<Crash> for Stop {
    fn from(c: Crash) -> Stop {
        Stop::Crash(c)
    }
}

/// The 16-bit samples of a wav file as Python's wave module reads them (array("h") over the frames, whatever the sample width),
/// and its frame rate. Err(Caught): the file cannot be opened or its frames are an odd number of bytes; Err(Crash): it is not a
/// wav file the wave module reads.
fn read_wav(path: &str) -> Result<(Vec<i16>, u32), Stop> {
    let data = std::fs::read(path).map_err(|_| Stop::Caught)?;
    let bad = |m: &str| Stop::Crash(Crash::new(format!("wave.Error: {}", m)));
    if data.len() < 12 {
        return Err(Stop::Crash(Crash::new("EOFError")));
    }
    if &data[0..4] != b"RIFF" {
        return Err(bad("file does not start with RIFF id"));
    }
    if &data[8..12] != b"WAVE" {
        return Err(bad("not a WAVE file"));
    }
    let riff_end = (8 + u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize).min(data.len());
    let mut pos = 12;
    let mut fmt: Option<(u16, u32, u16)> = None; // channels, rate, sample width in bytes
    loop {
        if pos + 8 > riff_end {
            break;
        }
        let id = &data[pos..pos + 4];
        let size = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
        let body = pos + 8;
        let end = (body + size).min(riff_end); // a chunk is read through the RIFF chunk, so never past its end
        if id == b"fmt " {
            let f = &data[body..end];
            if f.len() < 16 {
                return Err(Stop::Crash(Crash::new("struct.error")));
            }
            let tag = u16::from_le_bytes([f[0], f[1]]);
            let channels = u16::from_le_bytes([f[2], f[3]]);
            let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            if tag != 1 && tag != 0xfffe {
                return Err(bad(&format!("unknown format: {}", tag)));
            }
            let bits = u16::from_le_bytes([f[14], f[15]]);
            let width = bits.div_ceil(8);
            if width == 0 {
                return Err(bad("bad sample width"));
            }
            if tag == 0xfffe {
                // WAVE_FORMAT_EXTENSIBLE: the sub-format must be PCM
                const PCM: [u8; 16] = [1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71];
                if f.len() < 40 {
                    return Err(Stop::Crash(Crash::new("struct.error")));
                }
                if f[24..40] != PCM {
                    return Err(bad("unknown extended format"));
                }
            }
            if channels == 0 {
                return Err(bad("bad # of channels"));
            }
            fmt = Some((channels, rate, width));
        } else if id == b"data" {
            let Some((channels, rate, width)) = fmt else { return Err(bad("data chunk before fmt chunk")) };
            let framesize = channels as usize * width as usize;
            let nframes = size / framesize;
            let take = (nframes * framesize).min(end - body);
            let bytes = &data[body..body + take];
            if bytes.len() % 2 != 0 {
                return Err(Stop::Caught); // array.frombytes: ValueError
            }
            let samples = bytes.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            return Ok((samples, rate));
        }
        pos = body + size + (size & 1);
    }
    Err(bad("fmt chunk and/or data chunk missing"))
}

/// Loudness per 50 ms, relative to the loudest moment (0 to 100; a pause is flat).
fn envelope(path: &str) -> Result<Vec<i64>, Stop> {
    let (data, rate) = read_wav(path)?;
    let step = std::cmp::max(1, (rate / 20) as usize);
    let mut rms: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let chunk = &data[i..(i + step).min(data.len())];
        let sum: u64 = chunk.iter().map(|&v| (v as i64 * v as i64) as u64).sum();
        rms.push((sum as f64 / chunk.len() as f64).sqrt() / 32768.0);
        i += step;
    }
    let peak = rms.iter().copied().fold(None, |m: Option<f64>, r| match m {
        None => Some(r),
        Some(m) => Some(if r > m { r } else { m }),
    });
    let peak = peak.unwrap_or(0.0);
    Ok(rms
        .iter()
        .map(|&r| {
            if peak > 1e-4 && r > 0.04 * peak {
                let x = unsafe { pow(r / peak, std::hint::black_box(0.9)) };
                let m = if x < 1.0 { x } else { 1.0 };
                (m * 100.0) as i64
            } else {
                0
            }
        })
        .collect())
}

/// int(v or 0) for a pause in the service's message.
fn ms(v: Option<&Value>) -> Result<i128, Stop> {
    let v = match v {
        Some(v) if v.truthy() => v,
        _ => return Ok(0),
    };
    match v {
        Value::Int(s) => s.parse::<i128>().map_err(|_| Stop::Crash(Crash::new("OverflowError"))),
        Value::Bool(b) => Ok(*b as i128),
        Value::Float(f) => {
            if f.is_nan() {
                Err(Stop::Caught)
            } else if f.is_infinite() {
                Err(Stop::Crash(Crash::new("OverflowError")))
            } else {
                Ok(f.trunc() as i128)
            }
        }
        Value::Str(s) => py::parse_int(s).ok_or(Stop::Caught),
        _ => Err(Stop::Crash(Crash::new("TypeError: int() argument"))),
    }
}

/// time.sleep(min(ms, 1500) / 1000): a negative pause is a ValueError.
fn pause(v: Option<&Value>) -> Result<(), Stop> {
    let n = ms(v)?.min(1500);
    if n < 0 {
        return Err(Stop::Caught);
    }
    std::thread::sleep(Duration::from_millis(n as u64));
    Ok(())
}

/// json.dumps({"op": "speak", "text": text}) for the text as it came on the command line (a byte that is not UTF-8 is written
/// as Python's surrogate escape, \udcXX).
pub fn request(text: &[u8]) -> String {
    let mut out = String::from("{\"op\": \"speak\", \"text\": ");
    let mut s = String::new();
    let mut rest = text;
    let mut escaped = String::from("\"");
    loop {
        match std::str::from_utf8(rest) {
            Ok(v) => {
                s.push_str(v);
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                s.push_str(std::str::from_utf8(&rest[..good]).unwrap());
                // flush what is valid, then the bad byte as its surrogate
                let mut part = String::new();
                json::dump_str(&s, &mut part);
                escaped.push_str(&part[1..part.len() - 1]);
                s.clear();
                escaped.push_str(&format!("\\udc{:02x}", rest[good]));
                rest = &rest[good + 1..];
            }
        }
    }
    let mut part = String::new();
    json::dump_str(&s, &mut part);
    escaped.push_str(&part[1..part.len() - 1]);
    escaped.push('"');
    out.push_str(&escaped);
    out.push('}');
    out
}

pub fn main(args: &[std::ffi::OsString]) -> i32 {
    use std::os::unix::ffi::OsStrExt;
    if args.is_empty() {
        return 2;
    }
    let text: Vec<u8> = args.iter().map(|a| a.as_bytes().to_vec()).collect::<Vec<_>>().join(&b' ');
    let environ = Env::from_process();
    let run = py::join(
        &match environ.truthy("XDG_RUNTIME_DIR") {
            Some(d) => d.to_string(),
            None => format!("/run/user/{}", py::getuid()),
        },
        "labyrinth-sarina",
    );
    let sock = environ.get("LABYRINTH_VOICED_SOCKET").map(|s| s.to_string()).unwrap_or_else(|| py::join(&run, "voiced.sock"));
    let player = environ.get_or("LABYRINTH_PLAYER", "pw-play").to_string();
    match speak(&text, &run, &sock, &player) {
        Ok(code) => code,
        Err(Crash(what)) => {
            eprintln!("Traceback: {}", what);
            1
        }
    }
}

fn speak(text: &[u8], run: &str, sock: &str, player: &str) -> PyResult<i32> {
    let timeout = Some(Duration::from_secs(60)); // the first sentence may wait for the models to load
    let stream = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(_) => return Ok(1),
    };
    if stream.set_read_timeout(timeout).is_err() || stream.set_write_timeout(timeout).is_err() {
        return Ok(1);
    }
    let mut w = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return Ok(1),
    };
    if w.write_all(format!("{}\n", request(text)).as_bytes()).and_then(|_| w.flush()).is_err() {
        return Ok(1);
    }
    let mut said = false;
    let env = py::join(run, "speech.env");
    let result = (|| -> Result<(), Stop> {
        let mut r = BufReader::new(stream);
        let run_abs = py::abspath(run)?;
        loop {
            let mut line = Vec::new();
            let n = r.read_until(b'\n', &mut line).map_err(|_| Stop::Caught)?;
            if n == 0 {
                break;
            }
            let msg = json::loads_bytes(&line).map_err(|_| Stop::Caught)?;
            if !matches!(msg, Value::Obj(_)) {
                return Err(Stop::Crash(Crash::new("AttributeError: object has no attribute 'get'")));
            }
            let done = msg.get("done").map(|v| v.truthy()).unwrap_or(false);
            let error = msg.get("error").filter(|v| v.truthy());
            if done || error.is_some() {
                if let Some(e) = error {
                    eprintln!("voiced: {}", e.py_str());
                }
                break;
            }
            let path = match msg.get("wav") {
                None => continue,
                Some(v) if !v.truthy() => continue,
                Some(Value::Str(p)) => p.clone(),
                Some(_) => return Err(Stop::Crash(Crash::new("TypeError: expected str"))),
            };
            if py::dirname(&py::abspath(&path)?) != run_abs {
                continue; // only files the service made in our private folder
            }
            let step = (|| -> Result<(), Stop> {
                pause(msg.get("pause_before_ms"))?;
                let levels = envelope(&path)?;
                let body = format!(
                    "{}\n{}\n",
                    (py::time_now() * 1000.0) as i64 + 120,
                    levels.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(" ")
                );
                py::write_text(&env, &body).map_err(|_| Stop::Caught)?;
                let mut c = proc::command(&[player.to_string(), path.clone()], None, None);
                c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
                c.status().map_err(|_| Stop::Caught)?;
                said = true;
                pause(msg.get("pause_after_ms"))?;
                Ok(())
            })();
            let _ = std::fs::remove_file(&path);
            step?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&env);
    match result {
        Ok(()) | Err(Stop::Caught) => Ok(if said { 0 } else { 1 }),
        Err(Stop::Crash(c)) => Err(c),
    }
}
