//! Go: `time.Time` as far as Sarina's files need it: an instant read from RFC 3339 the way Go's `time.Time` JSON reading
//! does (Heimdall's grants file), compared with the clock. From Heimdall's `gotime.rs` (the same Go behaviour).

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds from Go's zero time (January 1, year 1, UTC) to the Unix epoch.
const GO_ZERO_TO_UNIX: i64 = 62_135_596_800;

/// An instant with the offset it was written with. The zero value is Go's zero time (year 1).
#[derive(Clone, Copy, Debug, Default)]
pub struct Time {
    /// Seconds since Go's zero time, UTC.
    secs: i64,
    nanos: u32,
    /// The UTC offset in seconds the time was written with.
    offset: i32,
}

impl PartialEq for Time {
    /// The same instant (Go's `Equal`), whatever the offset.
    fn eq(&self, o: &Time) -> bool {
        self.secs == o.secs && self.nanos == o.nanos
    }
}
impl Eq for Time {}

impl PartialOrd for Time {
    fn partial_cmp(&self, o: &Time) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Time {
    fn cmp(&self, o: &Time) -> std::cmp::Ordering {
        (self.secs, self.nanos).cmp(&(o.secs, o.nanos))
    }
}

impl Time {
    pub fn now() -> Time {
        let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Time { secs: d.as_secs() as i64 + GO_ZERO_TO_UNIX, nanos: d.subsec_nanos(), offset: 0 }
    }

    pub fn from_unix(secs: i64, nanos: u32) -> Time {
        Time { secs: secs + GO_ZERO_TO_UNIX, nanos, offset: 0 }
    }

    pub fn unix(&self) -> i64 {
        self.secs - GO_ZERO_TO_UNIX
    }

    pub fn nanos(&self) -> u32 {
        self.nanos
    }

    pub fn offset(&self) -> i32 {
        self.offset
    }

    pub fn before(&self, o: &Time) -> bool {
        self < o
    }

    /// Go's `t.Sub(o)` in nanoseconds (saturating as Go's does).
    pub fn sub_nanos(&self, o: &Time) -> i64 {
        let d = (self.secs as i128 - o.secs as i128) * 1_000_000_000 + (self.nanos as i128 - o.nanos as i128);
        d.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// Go's `t.Add(d)` for `d` nanoseconds.
    pub fn add_nanos(&self, d: i64) -> Time {
        let total = self.nanos as i64 + d.rem_euclid(1_000_000_000);
        let secs = self.secs + d.div_euclid(1_000_000_000) + total / 1_000_000_000;
        Time { secs, nanos: (total % 1_000_000_000) as u32, offset: self.offset }
    }

    /// Go's `t.UTC().Format(time.RFC3339)` for years 0 to 9999: "2026-10-05T09:00:00Z".
    pub fn format_rfc3339_utc(&self) -> String {
        let days = self.unix().div_euclid(86_400);
        let rem = self.unix().rem_euclid(86_400);
        // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
    }

    /// Go's `time.Parse(time.RFC3339, s)`, which Go 1.27's JSON reading of a `time.Time` uses (the v1 API keeps the loose
    /// parse: a one-digit hour, an offset up to 24:60, a comma before the fraction, fraction digits past nine dropped), with
    /// Go's error texts.
    pub fn parse_rfc3339(s: &str) -> Result<Time, String> {
        const LAYOUT: &str = "2006-01-02T15:04:05Z07:00";
        let b = s.as_bytes();
        let cannot = |value: &[u8], elem: &str| {
            format!(
                "parsing time {} as {}: cannot parse {} as {}",
                quote(b),
                quote(LAYOUT.as_bytes()),
                quote(value),
                quote(elem.as_bytes())
            )
        };
        let range = |what: &str| format!("parsing time {}: {what} out of range", quote(b));
        let digit = |v: &[u8], i: usize| v.get(i).is_some_and(u8::is_ascii_digit);
        // Go's getnum: one or two digits (two when fixed)
        let getnum = |v: &[u8], fixed: bool| -> Option<(i64, usize)> {
            if !digit(v, 0) {
                return None;
            }
            if !digit(v, 1) {
                return if fixed { None } else { Some(((v[0] - b'0') as i64, 1)) };
            }
            Some((((v[0] - b'0') * 10 + (v[1] - b'0')) as i64, 2))
        };
        let mut v = b;
        let lit = |v: &mut &[u8], c: u8| -> Result<(), String> {
            if v.first() == Some(&c) {
                *v = &v[1..];
                Ok(())
            } else {
                Err(cannot(v, std::str::from_utf8(&[c]).unwrap_or_default()))
            }
        };
        // 2006
        if v.len() < 4 || !digit(v, 0) || !v[..4].iter().all(u8::is_ascii_digit) {
            return Err(cannot(v, "2006"));
        }
        let year: i64 = v[..4].iter().fold(0, |n, c| n * 10 + (c - b'0') as i64);
        v = &v[4..];
        lit(&mut v, b'-')?;
        let Some((month, n)) = getnum(v, true) else { return Err(cannot(v, "01")) };
        if !(1..=12).contains(&month) {
            return Err(range("month"));
        }
        v = &v[n..];
        lit(&mut v, b'-')?;
        let Some((day, n)) = getnum(v, true) else { return Err(cannot(v, "02")) };
        v = &v[n..];
        lit(&mut v, b'T')?;
        let Some((hour, n)) = getnum(v, false) else { return Err(cannot(v, "15")) };
        if hour >= 24 {
            return Err(range("hour"));
        }
        v = &v[n..];
        lit(&mut v, b':')?;
        let Some((min, n)) = getnum(v, true) else { return Err(cannot(v, "04")) };
        if min >= 60 {
            return Err(range("minute"));
        }
        v = &v[n..];
        lit(&mut v, b':')?;
        let Some((sec, n)) = getnum(v, true) else { return Err(cannot(v, "05")) };
        if sec >= 60 {
            return Err(range("second"));
        }
        v = &v[n..];
        let mut nanos = 0u32;
        if v.len() >= 2 && (v[0] == b'.' || v[0] == b',') && digit(v, 1) {
            let mut n = 2;
            while digit(v, n) {
                n += 1;
            }
            // Go reads at most nine digits and drops the rest
            let digits = &v[1..n.min(10)];
            let mut ns: u32 = digits.iter().fold(0, |x, c| x * 10 + (c - b'0') as u32);
            for _ in digits.len()..9 {
                ns *= 10;
            }
            nanos = ns;
            v = &v[n..];
        }
        // Z07:00
        let offset: i64 = if v.first() == Some(&b'Z') {
            v = &v[1..];
            0
        } else {
            let hold = v;
            if v.len() < 6 || v[3] != b':' {
                return Err(cannot(hold, "Z07:00"));
            }
            let hr = getnum(&v[1..3], true);
            let mm = if hr.is_some() { getnum(&v[4..6], true) } else { None };
            let (h, m) = (hr.map_or(0, |x| x.0), mm.map_or(0, |x| x.0));
            if h > 24 {
                return Err(range("time zone offset hour"));
            }
            if m > 60 {
                return Err(range("time zone offset minute"));
            }
            if hr.is_none() || mm.is_none() || (v[0] != b'+' && v[0] != b'-') {
                return Err(cannot(hold, "Z07:00"));
            }
            let o = (h * 60 + m) * 60;
            let o = if v[0] == b'-' { -o } else { o };
            v = &v[6..];
            o
        };
        if !v.is_empty() {
            return Err(format!("parsing time {}: extra text: {}", quote(b), quote(v)));
        }
        if day < 1 || day > days_in_month(year, month as u32) as i64 {
            return Err(range("day"));
        }
        let days = days_from_civil(year, month as u32, day as u32);
        let local = days * 86_400 + hour * 3600 + min * 60 + sec;
        Ok(Time { secs: local - offset + GO_ZERO_TO_UNIX, nanos, offset: offset as i32 })
    }
}

/// The time package's own `quote`: a byte below space or above ASCII as `\xNN` (every byte of a rune), `"` and `\\`
/// escaped.
fn quote(s: &[u8]) -> String {
    let mut out = String::from("\"");
    let mut i = 0;
    while i < s.len() {
        let (r, w) = crate::gotext::decode_rune(&s[i..]);
        let c = r.unwrap_or('\u{FFFD}');
        if (c as u32) < 0x20 || (c as u32) >= 0x80 {
            // Go: a bad byte is one byte wide; U+FFFD itself three
            let width = if r.is_none() { 1 } else { w };
            for &x in &s[i..i + width] {
                out.push_str(&format!("\\x{x:02x}"));
            }
            i += width;
        } else {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
            i += w;
        }
    }
    out.push('"');
    out
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// From Limbo's port (rust/limbo/src/gotime.rs), with its Go-written answers.
/// time.Duration's String: "72h3m0.5s", "1.5ms", "0s"; the largest unit first, no trailing zeros in the fraction.
pub fn duration_string(d: i64) -> String {
    fn frac(buf: &mut Vec<u8>, mut v: u64, prec: u32) -> u64 {
        let mut print = false;
        for _ in 0..prec {
            let digit = (v % 10) as u8;
            print = print || digit != 0;
            if print {
                buf.push(b'0' + digit);
            }
            v /= 10;
        }
        if print {
            buf.push(b'.');
        }
        v
    }
    fn int(buf: &mut Vec<u8>, mut v: u64) {
        if v == 0 {
            buf.push(b'0');
            return;
        }
        while v > 0 {
            buf.push(b'0' + (v % 10) as u8);
            v /= 10;
        }
    }
    // built backwards, as Go fills its buffer from the end
    let mut b: Vec<u8> = vec![];
    let neg = d < 0;
    let mut u = d.unsigned_abs();
    if u < 1_000_000_000 {
        b.push(b's');
        let prec = match u {
            0 => return "0s".into(),
            1..=999 => {
                b.push(b'n');
                0
            }
            1000..=999_999 => {
                b.extend_from_slice("µ".as_bytes().iter().rev().copied().collect::<Vec<u8>>().as_slice());
                3
            }
            _ => {
                b.push(b'm');
                6
            }
        };
        u = frac(&mut b, u, prec);
        int(&mut b, u);
    } else {
        b.push(b's');
        u = frac(&mut b, u, 9);
        int(&mut b, u % 60);
        u /= 60;
        if u > 0 {
            b.push(b'm');
            int(&mut b, u % 60);
            u /= 60;
            if u > 0 {
                b.push(b'h');
                int(&mut b, u);
            }
        }
    }
    if neg {
        b.push(b'-');
    }
    b.reverse();
    String::from_utf8(b).unwrap_or_default()
}

#[cfg(test)]
mod duration_tests {
    /// Go's own answers (time.Duration(m) * time.Minute, then plain durations), written by a Go program.
    #[test]
    fn duration_string_is_gos() {
        let minutes: [(i64, &str); 11] = [
            (0, "0s"),
            (1, "1m0s"),
            (30, "30m0s"),
            (120, "2h0m0s"),
            (121, "2h1m0s"),
            (-5, "-5m0s"),
            (1 << 40, "1428016h22m54.643421184s"),
            (i64::MAX, "-1m0s"),
            (i64::MIN, "0s"),
            (153722867280912930, "-8s"),
            (1537228672809129301, "-20s"),
        ];
        for (m, want) in minutes {
            assert_eq!(super::duration_string(m.wrapping_mul(60_000_000_000)), want, "{m} minutes");
        }
        let plain: [(i64, &str); 12] = [
            (1, "1ns"),
            (999, "999ns"),
            (1000, "1µs"),
            (1500, "1.5µs"),
            (999999, "999.999µs"),
            (1000000, "1ms"),
            (1500000, "1.5ms"),
            (999999999, "999.999999ms"),
            (1000000000, "1s"),
            (1000000001, "1.000000001s"),
            (3723000000000, "1h2m3s"),
            (i64::MIN, "-2562047h47m16.854775808s"),
        ];
        for (d, want) in plain {
            assert_eq!(super::duration_string(d), want, "{d}");
        }
    }
}
