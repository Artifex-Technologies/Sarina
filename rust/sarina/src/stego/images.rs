//! Go's `image` package as stego's `DecodeImage` uses it: the format sniffed from the first bytes (gif, jpeg, png, in
//! Go's registration order, through image's bufio), the decoded image types, and each pixel turned into NRGBA as
//! `color.NRGBAModel` does (testdata/parity/stegolsb.json).

use super::goio::{Bufio, BytesReader, GoErr};

/// The image types Go's PNG and GIF decoders return.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Gray,
    Gray16,
    Rgba,
    Nrgba,
    Rgba64,
    Nrgba64,
    Paletted,
}

impl Kind {
    fn bytes(self) -> usize {
        match self {
            Kind::Gray | Kind::Paletted => 1,
            Kind::Gray16 => 2,
            Kind::Rgba | Kind::Nrgba => 4,
            Kind::Rgba64 | Kind::Nrgba64 => 8,
        }
    }
}

/// A palette entry: color.RGBA (alpha-premultiplied) or color.NRGBA.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pal {
    Rgba([u8; 4]),
    Nrgba([u8; 4]),
}

/// An image with its bounds (a GIF frame need not start at 0,0) and pixels as Go lays them out.
#[derive(Clone, Debug)]
pub struct Img {
    pub kind: Kind,
    pub min_x: usize,
    pub min_y: usize,
    pub w: usize,
    pub h: usize,
    pub pix: Vec<u8>,
    /// a paletted image's palette (for PNG, Go's underlying array of 256: Go re-slices it to the indices used, which
    /// only keeps At from going past it)
    pub palette: Vec<Pal>,
}

impl Img {
    pub fn new(kind: Kind, min_x: usize, min_y: usize, w: usize, h: usize) -> Img {
        Img { kind, min_x, min_y, w, h, pix: vec![0; w * h * kind.bytes()], palette: Vec::new() }
    }

    pub fn set(&mut self, x: usize, y: usize, px: &[u8]) {
        let n = self.kind.bytes();
        let i = (y * self.w + x) * n;
        self.pix[i..i + n].copy_from_slice(&px[..n]);
    }

    pub fn set_row(&mut self, y: usize, row: &[u8]) {
        let n = self.w * self.kind.bytes();
        self.pix[y * n..y * n + n].copy_from_slice(&row[..n]);
    }

    /// Go's png `mergePassInto`.
    pub fn merge(&mut self, src: &Img, (xf, yf, xo, yo): (usize, usize, usize, usize)) {
        let bpp = self.kind.bytes();
        let stride = self.w * bpp;
        let mut s = 0;
        for y in 0..src.h {
            let base = (y * yf + yo) * stride + xo * bpp;
            for x in 0..src.w {
                let d = base + x * xf * bpp;
                self.pix[d..d + bpp].copy_from_slice(&src.pix[s..s + bpp]);
                s += bpp;
            }
        }
    }

    /// The pixel at (x, y) of the image's own grid (0-based) as `color.NRGBAModel.Convert(img.At(...))`, or, for RGBA and
    /// NRGBA images, the stored bytes as DecodeImage reads them.
    pub fn nrgba(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.w + x) * self.kind.bytes();
        let p = &self.pix[i..i + self.kind.bytes()];
        let w16 = |h: u8, l: u8| u32::from(h) << 8 | u32::from(l);
        match self.kind {
            Kind::Rgba | Kind::Nrgba => [p[0], p[1], p[2], p[3]],
            Kind::Gray => [p[0], p[0], p[0], 0xff],
            Kind::Gray16 => [p[0], p[0], p[0], 0xff],
            Kind::Rgba64 => premultiplied(w16(p[0], p[1]), w16(p[2], p[3]), w16(p[4], p[5]), w16(p[6], p[7])),
            Kind::Nrgba64 => {
                let a = w16(p[6], p[7]);
                premultiplied(w16(p[0], p[1]) * a / 0xffff, w16(p[2], p[3]) * a / 0xffff, w16(p[4], p[5]) * a / 0xffff, a)
            }
            Kind::Paletted => match self.palette[usize::from(p[0])] {
                Pal::Nrgba(c) => c,
                Pal::Rgba([r, g, b, a]) => {
                    let s = |v: u8| u32::from(v) | u32::from(v) << 8;
                    premultiplied(s(r), s(g), s(b), s(a))
                }
            },
        }
    }
}

/// Go's `nrgbaModel` of a colour whose `RGBA()` is (r, g, b, a).
fn premultiplied(r: u32, g: u32, b: u32, a: u32) -> [u8; 4] {
    if a == 0xffff {
        return [(r >> 8) as u8, (g >> 8) as u8, (b >> 8) as u8, 0xff];
    }
    if a == 0 {
        return [0, 0, 0, 0];
    }
    [((r * 0xffff / a) >> 8) as u8, ((g * 0xffff / a) >> 8) as u8, ((b * 0xffff / a) >> 8) as u8, (a >> 8) as u8]
}

/// Go's `bufio.Reader.Peek` as image's sniffing uses it: true when the first bytes match the magic ('?' matches any).
fn peek_matches(r: &mut Bufio<BytesReader>, magic: &[u8]) -> bool {
    r.peek(magic.len()).is_some_and(|b| b.iter().zip(magic).all(|(c, m)| c == m || *m == b'?'))
}

/// The registered formats in Go's order.
const FORMATS: [(&str, &[u8]); 3] = [("gif", b"GIF8?a"), ("jpeg", b"\xff\xd8"), ("png", b"\x89PNG\r\n\x1a\n")];

fn sniff(r: &mut Bufio<BytesReader>) -> Option<&'static str> {
    FORMATS.iter().find(|(_, m)| peek_matches(r, m)).map(|(n, _)| *n)
}

const UNKNOWN: &str = "image: unknown format";

/// Go's `image.DecodeConfig`: (width, height, format).
pub fn decode_config(b: &[u8]) -> Result<(usize, usize), GoErr> {
    let mut r = Bufio::new(BytesReader::new(b));
    match sniff(&mut r) {
        Some("png") => super::png::Decoder::new(r).decode_config(),
        Some("gif") => super::gif::decode_config(r),
        Some(_) => super::jpeg::Decoder::new(b).decode_config(),
        None => Err(GoErr::Msg(UNKNOWN.into())),
    }
}

/// Go's `image.Decode`: the picture and its format's name.
pub fn decode(b: &[u8]) -> Result<(Img, &'static str), GoErr> {
    let mut r = Bufio::new(BytesReader::new(b));
    match sniff(&mut r) {
        Some("png") => super::png::Decoder::new(r).decode().map(|m| (m, "png")),
        Some("gif") => super::gif::decode(r).map(|m| (m, "gif")),
        Some(_) => super::jpeg::Decoder::new(b).decode().map(|m| (m, "jpeg")),
        None => Err(GoErr::Msg(UNKNOWN.into())),
    }
}
