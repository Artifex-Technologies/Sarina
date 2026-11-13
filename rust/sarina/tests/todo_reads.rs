//! Go's TestTodoToolsDoNotWaitOnNamedPipes and TestTodoToolsDoNotReadHugeFiles (2026-11-12, night): a named pipe under
//! the to-do list's or the plan's file name is refused at once (not waited on) by every to-do and plan tool, and a file
//! past the bound is not read (this test binary counts what it allocates).

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sarina::gojson;
use sarina::gopath;
use sarina::tools::registry::{Context, Registry};
use sarina::tools::todo::todo_tools;

struct Counting;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is passed to the system allocator unchanged; only the size asked for is counted.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(l.size() as u64, Ordering::Relaxed);
        // SAFETY: the caller's layout, as given
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: p came from alloc or realloc above with this layout
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCATED.fetch_add(n as u64, Ordering::Relaxed);
        // SAFETY: p came from this allocator with layout l
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn workspace(name: &str) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("sarina-todo-reads-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".port_sessions")).unwrap();
    let ws = String::from_utf8(gopath::eval_symlinks(dir.to_str().unwrap().as_bytes()).unwrap()).unwrap();
    let store = format!("{ws}/.port_sessions");
    (ws, store)
}

fn context(ws: &str) -> Context {
    let mut tc = Context { root: ws.to_owned(), max_output_chars: 12000, ..Context::default() };
    tc.permissions.allow_file_write = true;
    tc
}

fn args(json: &str) -> Option<std::collections::BTreeMap<String, sarina::goany::Any>> {
    gojson::unmarshal_any(json.as_bytes()).0.into_object().ok()
}

#[test]
fn todo_tools_do_not_wait_on_named_pipes() {
    for (name, json, ok) in [
        ("task_list", "{}", true),
        ("task_create", r#"{"title":"design","task_id":"a"}"#, true),
        ("task_update", r#"{"task_id":"a","status":"completed"}"#, false),
        ("todo_write", r#"{"items":[{"title":"one"}]}"#, true),
        ("plan_get", "{}", true),
        ("update_plan", r#"{"items":[{"step":"one","status":"pending"}]}"#, true),
    ] {
        let (ws, store) = workspace(name);
        let pipes = [format!("{store}/task_runtime.json"), format!("{store}/plan_runtime.json")];
        for p in &pipes {
            let c = CString::new(p.as_str()).unwrap();
            // SAFETY: mkfifo on a path this test made
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ws2 = ws.clone();
        std::thread::spawn(move || {
            let r = Registry::new(todo_tools()).execute(name, args(json), &context(&ws2));
            let _ = tx.send((r.ok, r.content));
        });
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok((got, content)) => assert_eq!(got, ok, "{name}: {content}"),
            Err(_) => {
                // let the stuck readers go (writers that close at once), so the test binary can end
                for _ in 0..200 {
                    for p in &pipes {
                        let c = CString::new(p.as_str()).unwrap();
                        // SAFETY: open and close of a path this test made
                        let fd = unsafe { libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
                        if fd >= 0 {
                            // SAFETY: fd was just opened here
                            unsafe { libc::close(fd) };
                        }
                    }
                    if rx.recv_timeout(Duration::from_millis(50)).is_ok() {
                        break;
                    }
                }
                panic!("{name} waited on a named pipe");
            }
        }
        let _ = std::fs::remove_dir_all(&ws);
    }
}

#[test]
fn todo_tools_do_not_read_huge_files() {
    let (ws, store) = workspace("huge");
    for name in ["task_runtime.json", "plan_runtime.json"] {
        std::fs::File::create(format!("{store}/{name}")).unwrap().set_len(80 << 20).unwrap();
    }
    let reg = Registry::new(todo_tools());
    let tc = context(&ws);
    for name in ["task_list", "plan_get"] {
        let before = ALLOCATED.load(Ordering::Relaxed);
        let r = reg.execute(name, args("{}"), &tc);
        let got = ALLOCATED.load(Ordering::Relaxed) - before;
        assert!(r.ok, "{name}: {}", r.content);
        assert!(got < 8 << 20, "{name} allocated {got} bytes for an 80 MiB file");
    }
    let _ = std::fs::remove_dir_all(&ws);
}
