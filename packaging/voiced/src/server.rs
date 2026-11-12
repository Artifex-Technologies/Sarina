//! The socket: $XDG_RUNTIME_DIR/labyrinth-sarina/voiced.sock (private to the user), one JSON request line per connection,
//! the same requests and answers as labyrinth-voiced:
//!
//!   {"op":"speak","text":"Hello. How are you?"}  ->  {"wav": "...", "text": "Hello.", "pause_before_ms": 0, "pause_after_ms": 0}
//!                                                     per sentence, then {"done": true} (or {"error": "..."})
//!   {"op":"ping"}    ->  {"ok": true, "loaded": false, "engine": "chatterbox"}
//!   {"op":"warm"}    ->  {"ok": true} at once; loads the voice in the background
//!   {"op":"unload"}  ->  frees the graphics card memory, then {"ok": true}

use crate::voice::{now, Service};
use sarina_voice::json::{self, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// The longest request line read (a speak request is a few kilobytes; the Python had no limit).
const MAX_LINE: u64 = 4 << 20;

pub fn serve(service: Arc<Service>, sock: &str) -> std::io::Result<()> {
    let run = &service.settings.run;
    if !run.is_dir() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(run)?;
    }
    match std::fs::remove_file(sock) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(sock)?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))?;

    let idle = service.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(30));
        if idle.loaded.load(Ordering::SeqCst) && now() - *idle.last.lock().unwrap_or_else(|e| e.into_inner()) > idle.settings.idle {
            // only when nothing holds the voice (lock.locked() in the Python)
            if let Ok(mut slot) = idle.lock.try_lock() {
                *slot = None;
                idle.loaded.store(false, Ordering::SeqCst);
                drop(slot);
                eprintln!("voiced: idle, graphics card memory freed");
            }
        }
    });

    for conn in listener.incoming() {
        match conn {
            Ok(conn) => {
                let s = service.clone();
                std::thread::spawn(move || handle(&s, conn));
            }
            Err(e) => eprintln!("voiced: accept: {}", e),
        }
    }
    Ok(())
}

use std::os::unix::fs::DirBuilderExt;

fn handle(service: &Arc<Service>, conn: UnixStream) {
    let mut w = match conn.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut send = |line: &str| -> bool { w.write_all(format!("{}\n", line).as_bytes()).and_then(|_| w.flush()).is_ok() };
    let mut line = Vec::new();
    if BufReader::new((&conn).take(MAX_LINE)).read_until(b'\n', &mut line).is_err() {
        return;
    }
    let req = if line.is_empty() {
        Value::Obj(Vec::new())
    } else {
        match json::loads_bytes(&line) {
            Ok(v) => v,
            Err(_) => return, // not JSON: the connection is closed, nothing said (as the Python did)
        }
    };
    if !matches!(req, Value::Obj(_)) {
        eprintln!("voiced: a request that is not a JSON object");
        return;
    }
    let ok = || json::dumps(&json::obj(vec![("ok", Value::Bool(true))]));
    match req.get("op").and_then(|v| v.as_str()) {
        Some("ping") => {
            let loaded = service.loaded.load(Ordering::SeqCst);
            send(&json::dumps(&json::obj(vec![
                ("ok", Value::Bool(true)),
                ("loaded", Value::Bool(loaded)),
                ("engine", json::s("chatterbox")),
            ])));
        }
        Some("warm") => {
            send(&ok());
            let s = service.clone();
            std::thread::spawn(move || {
                let mut slot = s.lock.lock().unwrap_or_else(|e| e.into_inner());
                s.touch();
                if let Err(e) = s.load(&mut slot) {
                    eprintln!("voiced: could not load the voice: {}", e);
                }
            });
        }
        Some("unload") => {
            service.unload();
            send(&ok());
        }
        Some("speak") => {
            // str(req.get("text", "")): a value that is not a string is said as Python writes it
            let text = match req.get("text") {
                None => String::new(),
                Some(v) => v.py_str(),
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| service.speak(&text, &mut send)));
            let msg = match r {
                Ok(Ok(())) => json::dumps(&json::obj(vec![("done", Value::Bool(true))])),
                Ok(Err(e)) => json::dumps(&json::obj(vec![("error", json::s(&e.chars().take(200).collect::<String>()))])),
                Err(_) => json::dumps(&json::obj(vec![("error", json::s("internal error"))])),
            };
            send(&msg);
        }
        _ => {}
    }
}
