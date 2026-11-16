//! Go's `net/netip.ParseAddr` with its error texts, and `net.ParseIP`'s yes or no (which refuses zones): what url.Parse
//! says of a bracketed host and what crypto/x509 asks of a URI constraint.

fn err(input: &str, msg: &str, at: &str) -> String {
    let q = crate::gotext::quote;
    if at.is_empty() {
        format!("ParseAddr({}): {msg}", q(input))
    } else {
        format!("ParseAddr({}): {msg} (at {})", q(input), q(at))
    }
}

fn ipv4_fields(input: &str, off: usize, end: usize) -> Result<(), String> {
    let s = &input.as_bytes()[off..end];
    let (mut val, mut pos, mut dig_len) = (0u32, 0, 0);
    for i in 0..s.len() {
        let c = s[i];
        if c.is_ascii_digit() {
            if dig_len == 1 && val == 0 {
                return Err(err(input, "IPv4 field has octet with leading zero", ""));
            }
            val = val * 10 + u32::from(c - b'0');
            dig_len += 1;
            if val > 255 {
                return Err(err(input, "IPv4 field has value >255", ""));
            }
        } else if c == b'.' {
            if i == 0 || i == s.len() - 1 || s[i - 1] == b'.' {
                return Err(err(input, "IPv4 field must have at least one digit", &input[off + i..end]));
            }
            if pos == 3 {
                return Err(err(input, "IPv4 address too long", ""));
            }
            pos += 1;
            val = 0;
            dig_len = 0;
        } else {
            return Err(err(input, "unexpected character", &input[off + i..end]));
        }
    }
    if pos < 3 {
        return Err(err(input, "IPv4 address too short", ""));
    }
    Ok(())
}

/// What a parsed address is: IPv4, and whether it has a zone.
pub struct Addr {
    pub is4: bool,
    pub zone: bool,
}

fn ipv6(input: &str) -> Result<Addr, String> {
    let (mut s, zone) = match input.find('%') {
        Some(i) => {
            if input[i + 1..].is_empty() {
                return Err(err(input, "zone must be a non-empty string", ""));
            }
            (&input[..i], &input[i + 1..])
        }
        None => (input, ""),
    };
    let addr = Addr { is4: false, zone: !zone.is_empty() };
    let mut ellipsis: i32 = -1;
    if s.starts_with("::") {
        ellipsis = 0;
        s = &s[2..];
        if s.is_empty() {
            return Ok(addr);
        }
    }
    let mut i = 0;
    while i < 16 {
        let b = s.as_bytes();
        let mut off = 0;
        let mut acc = 0u32;
        while off < b.len() {
            let c = b[off];
            let d = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => break,
            };
            acc = (acc << 4) + u32::from(d);
            if off > 3 {
                return Err(err(input, "each group must have 4 or less digits", s));
            }
            if acc > 0xffff {
                return Err(err(input, "IPv6 field has value >=2^16", s));
            }
            off += 1;
        }
        if off == 0 {
            return Err(err(input, "each colon-separated field must have at least one digit", s));
        }
        if off < b.len() && b[off] == b'.' {
            if ellipsis < 0 && i != 12 {
                return Err(err(input, "embedded IPv4 address must replace the final 2 fields of the address", s));
            }
            if i + 4 > 16 {
                return Err(err(input, "too many hex fields to fit an embedded IPv4 at the end of the address", s));
            }
            let mut end = input.len();
            if !zone.is_empty() {
                end -= zone.len() + 1;
            }
            ipv4_fields(input, end - s.len(), end)?;
            s = "";
            i += 4;
            break;
        }
        i += 2;
        s = &s[off..];
        if s.is_empty() {
            break;
        }
        if !s.starts_with(':') {
            return Err(err(input, "unexpected character, want colon", s));
        } else if s.len() == 1 {
            return Err(err(input, "colon must be followed by more characters", s));
        }
        s = &s[1..];
        if s.starts_with(':') {
            if ellipsis >= 0 {
                return Err(err(input, "multiple :: in address", s));
            }
            ellipsis = i as i32;
            s = &s[1..];
            if s.is_empty() {
                break;
            }
        }
    }
    if !s.is_empty() {
        return Err(err(input, "trailing garbage after address", s));
    }
    if i < 16 {
        if ellipsis < 0 {
            return Err(err(input, "address string too short", ""));
        }
    } else if ellipsis >= 0 {
        return Err(err(input, "the :: must expand to at least one field of zeros", ""));
    }
    Ok(addr)
}

/// Go's `netip.ParseAddr`.
pub fn parse_addr(s: &str) -> Result<Addr, String> {
    for c in s.bytes() {
        match c {
            b'.' => return ipv4_fields(s, 0, s.len()).map(|()| Addr { is4: true, zone: false }),
            b':' => return ipv6(s),
            b'%' => return Err(err(s, "missing IPv6 address", "")),
            _ => {}
        }
    }
    Err(err(s, "unable to parse IP", ""))
}

/// Go's `net.ParseIP(s) != nil`.
pub fn parses_as_ip(s: &str) -> bool {
    parse_addr(s).is_ok_and(|a| !a.zone)
}
