//! Go's io readers as the image and zlib code reads through them: `bytes.Reader`, `bufio.Reader` (4096 bytes, its
//! short reads and read-ahead, which decide where a PNG's chunk errors surface) and `io.ReadFull`, with Go's errors.

use std::fmt;

/// An error as Go's readers return them.
#[derive(Clone, Debug, PartialEq)]
pub enum GoErr {
    Eof,
    UnexpectedEof,
    NoProgress,
    Msg(String),
}

impl fmt::Display for GoErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            GoErr::Eof => f.write_str("EOF"),
            GoErr::UnexpectedEof => f.write_str("unexpected EOF"),
            GoErr::NoProgress => f.write_str("multiple Read calls return no data or error"),
            GoErr::Msg(m) => f.write_str(m),
        }
    }
}

/// Go's `io.Reader`.
pub trait GoRead {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoErr>);
}

/// Go's `io.ByteReader`.
pub trait GoByteRead {
    fn read_byte(&mut self) -> Result<u8, GoErr>;
}

/// Go's `bytes.Reader`.
pub struct BytesReader<'a> {
    pub s: &'a [u8],
    pub i: usize,
}

impl<'a> BytesReader<'a> {
    pub fn new(s: &'a [u8]) -> BytesReader<'a> {
        BytesReader { s, i: 0 }
    }
}

impl GoRead for BytesReader<'_> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoErr>) {
        if self.i >= self.s.len() {
            return (0, Some(GoErr::Eof));
        }
        let n = p.len().min(self.s.len() - self.i);
        p[..n].copy_from_slice(&self.s[self.i..self.i + n]);
        self.i += n;
        (n, None)
    }
}

impl GoByteRead for BytesReader<'_> {
    fn read_byte(&mut self) -> Result<u8, GoErr> {
        let c = *self.s.get(self.i).ok_or(GoErr::Eof)?;
        self.i += 1;
        Ok(c)
    }
}

/// Go's `bufio.Reader` of the default size.
pub struct Bufio<R> {
    pub rd: R,
    buf: Vec<u8>,
    r: usize,
    w: usize,
    err: Option<GoErr>,
}

impl<R: GoRead> Bufio<R> {
    pub fn new(rd: R) -> Bufio<R> {
        Bufio { rd, buf: vec![0; 4096], r: 0, w: 0, err: None }
    }

    fn read_err(&mut self) -> Option<GoErr> {
        self.err.take()
    }

    /// Go's `Peek`: the next n bytes, or None when fewer came (the error that cut them short is dropped, as Go's
    /// readErr does).
    pub fn peek(&mut self, n: usize) -> Option<&[u8]> {
        while self.w - self.r < n && self.w - self.r < self.buf.len() && self.err.is_none() {
            self.fill();
        }
        if self.w - self.r < n {
            self.read_err();
            return None;
        }
        Some(&self.buf[self.r..self.r + n])
    }

    fn fill(&mut self) {
        if self.r > 0 {
            self.buf.copy_within(self.r..self.w, 0);
            self.w -= self.r;
            self.r = 0;
        }
        for _ in 0..100 {
            let w = self.w;
            let (n, err) = self.rd.read(&mut self.buf[w..]);
            self.w += n;
            if err.is_some() {
                self.err = err;
                return;
            }
            if n > 0 {
                return;
            }
        }
        self.err = Some(GoErr::NoProgress);
    }
}

impl<R: GoRead> GoRead for Bufio<R> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoErr>) {
        if p.is_empty() {
            if self.w > self.r {
                return (0, None);
            }
            return (0, self.read_err());
        }
        if self.r == self.w {
            if self.err.is_some() {
                return (0, self.read_err());
            }
            if p.len() >= self.buf.len() {
                let (n, err) = self.rd.read(p);
                self.err = err;
                return (n, self.read_err());
            }
            self.r = 0;
            self.w = 0;
            let (n, err) = self.rd.read(&mut self.buf[..]);
            self.err = err;
            if n == 0 {
                return (0, self.read_err());
            }
            self.w += n;
        }
        let n = p.len().min(self.w - self.r);
        p[..n].copy_from_slice(&self.buf[self.r..self.r + n]);
        self.r += n;
        (n, None)
    }
}

impl<R: GoRead> GoByteRead for Bufio<R> {
    fn read_byte(&mut self) -> Result<u8, GoErr> {
        while self.r == self.w {
            if self.err.is_some() {
                return Err(self.read_err().unwrap_or(GoErr::Eof));
            }
            self.fill();
        }
        let c = self.buf[self.r];
        self.r += 1;
        Ok(c)
    }
}

/// Go's `io.ReadFull`: how many bytes came, and the error (EOF only when none did).
pub fn read_full<R: GoRead + ?Sized>(r: &mut R, buf: &mut [u8]) -> (usize, Option<GoErr>) {
    let mut n = 0;
    let mut err = None;
    while n < buf.len() && err.is_none() {
        let (nn, e) = r.read(&mut buf[n..]);
        n += nn;
        err = e;
    }
    if n >= buf.len() {
        (n, None)
    } else if n > 0 && err == Some(GoErr::Eof) {
        (n, Some(GoErr::UnexpectedEof))
    } else {
        (n, err)
    }
}
