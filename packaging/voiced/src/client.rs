//! `sarina-voiced check`: a silent client for the voice service, to see that it works and how quickly, without playing
//! anything (the files are read for their length and deleted).

use sarina_voice::json::{self, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

fn wav_seconds(path: &str) -> Option<f64> {
    let d = std::fs::read(path).ok()?;
    if d.len() < 44 || &d[0..4] != b"RIFF" {
        return None;
    }
    let rate = u32::from_le_bytes([d[24], d[25], d[26], d[27]]) as f64;
    let bytes_per_frame = u16::from_le_bytes([d[32], d[33]]) as f64;
    let data = u32::from_le_bytes([d[40], d[41], d[42], d[43]]) as f64;
    Some(data / bytes_per_frame / rate)
}

fn request(sock: &str, req: &str) -> std::io::Result<BufReader<UnixStream>> {
    let mut s = UnixStream::connect(sock)?;
    s.set_read_timeout(Some(Duration::from_secs(600)))?;
    s.write_all(format!("{}\n", req).as_bytes())?;
    Ok(BufReader::new(s))
}

/// Exit status: 0 when every sentence arrived, 1 otherwise.
pub fn check(sock: &str, text: &str) -> i32 {
    let ping = || -> Option<bool> {
        let mut r = request(sock, "{\"op\": \"ping\"}").ok()?;
        let mut line = String::new();
        r.read_line(&mut line).ok()?;
        json::loads(line.trim()).ok()?.get("loaded").map(|v| v.truthy())
    };
    match ping() {
        Some(loaded) => println!(
            "service at {}: answering, voice {}",
            sock,
            if loaded { "loaded" } else { "not loaded (the first sentence waits for it)" }
        ),
        None => {
            println!("service at {}: no answer", sock);
            return 1;
        }
    }
    let req = json::dumps(&json::obj(vec![("op", json::s("speak")), ("text", json::s(text))]));
    let t0 = Instant::now();
    let mut r = match request(sock, &req) {
        Ok(r) => r,
        Err(e) => {
            println!("cannot ask: {}", e);
            return 1;
        }
    };
    let (mut first, mut play_end, mut gaps, mut n) = (None::<f64>, 0.0f64, 0.0f64, 0);
    loop {
        let mut line = String::new();
        match r.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let now = t0.elapsed().as_secs_f64();
        let Ok(msg) = json::loads(line.trim()) else { break };
        if let Some(Value::Str(path)) = msg.get("wav") {
            let ms = |k: &str| match msg.get(k) {
                Some(Value::Int(s)) => s.parse::<f64>().unwrap_or(0.0) / 1000.0,
                _ => 0.0,
            };
            let secs = wav_seconds(path).unwrap_or(0.0);
            let _ = std::fs::remove_file(path);
            let dur = secs + ms("pause_before_ms") + ms("pause_after_ms");
            n += 1;
            let said = msg.get("text").map(|v| v.py_str()).unwrap_or_default();
            println!("{:7.2} s  sentence {} ({:.2} s of speech): {}", now, n, secs, said);
            match first {
                None => {
                    first = Some(now);
                    play_end = now + dur;
                }
                Some(_) => {
                    gaps += (now - play_end).max(0.0);
                    play_end = play_end.max(now) + dur;
                }
            }
        } else if msg.get("done").is_some() {
            println!("{:7.2} s  done", now);
            break;
        } else if let Some(e) = msg.get("error") {
            println!("{:7.2} s  error: {}", now, e.py_str());
            return 1;
        }
    }
    match first {
        Some(f) => {
            println!(
                "first sentence after {:.2} s; a listener would have heard {:.2} s of gaps between sentences; the last one ends at {:.2} s",
                f, gaps, play_end
            );
            0
        }
        None => {
            println!("nothing was said");
            1
        }
    }
}
