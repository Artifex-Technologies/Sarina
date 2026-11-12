//! Go's `context.Context`, as far as Sarina's clients use it: cancelled by hand (`context.WithCancel`) or at a deadline
//! (`context.WithDeadline`), with Go's error texts ("context canceled", "context deadline exceeded"). A request that runs
//! under a context registers its socket with it, so a cancel shuts the socket and wakes a read or a TLS handshake that is
//! blocked on it at once (Go's transport closes the connection the same way); a wait (the retry's delay) wakes too.

use std::net::{Shutdown, TcpStream};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Go's `context.Canceled` text.
pub const CANCELED: &str = "context canceled";
/// Go's `context.DeadlineExceeded` text.
pub const DEADLINE_EXCEEDED: &str = "context deadline exceeded";

/// A context. Clones share it: a cancel through any clone is seen by all.
#[derive(Clone)]
pub struct Ctx {
    inner: Arc<Inner>,
}

struct Inner {
    deadline: Option<Instant>,
    state: Mutex<State>,
    cv: Condvar,
}

#[derive(Default)]
struct State {
    canceled: bool,
    next: u64,
    socks: Vec<(u64, Sock)>,
}

/// A socket a cancel shuts.
pub enum Sock {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Sock {
    fn shut(&self) {
        let _ = match self {
            Sock::Tcp(s) => s.shutdown(Shutdown::Both),
            Sock::Unix(s) => s.shutdown(Shutdown::Both),
        };
    }
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx::background()
    }
}

impl Ctx {
    /// A context without a deadline, done only when cancelled.
    pub fn background() -> Ctx {
        Ctx { inner: Arc::new(Inner { deadline: None, state: Mutex::new(State::default()), cv: Condvar::new() }) }
    }

    /// A context done at `deadline` (or when cancelled before).
    pub fn with_deadline(deadline: Instant) -> Ctx {
        Ctx { inner: Arc::new(Inner { deadline: Some(deadline), state: Mutex::new(State::default()), cv: Condvar::new() }) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Cancels: every registered socket is shut and every wait wakes.
    pub fn cancel(&self) {
        let mut st = self.lock();
        st.canceled = true;
        for (_, s) in &st.socks {
            s.shut();
        }
        st.socks.clear();
        self.inner.cv.notify_all();
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.inner.deadline
    }

    /// Go's `ctx.Err()`: None while the context is live.
    pub fn err(&self) -> Option<&'static str> {
        if self.lock().canceled {
            return Some(CANCELED);
        }
        match self.inner.deadline {
            Some(d) if Instant::now() >= d => Some(DEADLINE_EXCEEDED),
            _ => None,
        }
    }

    /// Waits `d`, or less when the context ends first; false then.
    pub fn sleep(&self, d: Duration) -> bool {
        let until = Instant::now().checked_add(d);
        let mut st = self.lock();
        loop {
            if st.canceled {
                return false;
            }
            let now = Instant::now();
            if self.inner.deadline.is_some_and(|x| now >= x) {
                return false;
            }
            let Some(u) = until else {
                st = self.inner.cv.wait(st).unwrap_or_else(|p| p.into_inner());
                continue;
            };
            if now >= u {
                return true;
            }
            let mut wait = u - now;
            if let Some(x) = self.inner.deadline {
                wait = wait.min(x - now);
            }
            st = self.inner.cv.wait_timeout(st, wait).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    /// Registers a socket until the guard is dropped; a context already cancelled shuts it at once.
    pub fn watch(&self, s: Sock) -> Watch {
        let mut st = self.lock();
        if st.canceled {
            s.shut();
            return Watch { ctx: None, id: 0 };
        }
        st.next += 1;
        let id = st.next;
        st.socks.push((id, s));
        Watch { ctx: Some(self.clone()), id }
    }
}

/// A socket's registration with a context, dropped with the connection.
pub struct Watch {
    ctx: Option<Ctx>,
    id: u64,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(c) = &self.ctx {
            c.lock().socks.retain(|(i, _)| *i != self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_and_deadline() {
        let c = Ctx::background();
        assert_eq!(c.err(), None);
        assert!(c.sleep(Duration::from_millis(1)));
        let c2 = c.clone();
        let t = std::thread::spawn(move || c2.sleep(Duration::from_secs(30)));
        std::thread::sleep(Duration::from_millis(20));
        c.cancel();
        assert!(!t.join().unwrap());
        assert_eq!(c.err(), Some(CANCELED));
        let d = Ctx::with_deadline(Instant::now() + Duration::from_millis(30));
        let start = Instant::now();
        assert!(!d.sleep(Duration::from_secs(30)));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(d.err(), Some(DEADLINE_EXCEEDED));
    }

    #[test]
    fn cancel_wakes_a_blocked_read() {
        use std::io::Read;
        let ln = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut s = TcpStream::connect(ln.local_addr().unwrap()).unwrap();
        let _peer = ln.accept().unwrap();
        let c = Ctx::background();
        let _w = c.watch(Sock::Tcp(s.try_clone().unwrap()));
        let c2 = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            c2.cancel();
        });
        let start = Instant::now();
        let mut b = [0u8; 8];
        let n = s.read(&mut b).unwrap_or(0);
        assert_eq!(n, 0);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
