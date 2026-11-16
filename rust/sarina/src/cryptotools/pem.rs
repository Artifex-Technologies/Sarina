//! Go's encoding/pem `Decode`: the next whole block (its type and bytes) and the rest, skipping anything that only looks
//! like one (a header not at a line start, a broken base64 body, an end line naming another type, text after it).

use super::decode::base64_std;

pub(crate) struct Block {
    pub kind: Vec<u8>,
    pub bytes: Vec<u8>,
}

const START: &[u8] = b"\n-----BEGIN ";
const END: &[u8] = b"\n-----END ";
const EOL: &[u8] = b"-----";

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn rfind(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).rposition(|w| w == n)
}

fn trim_right_space_tab(mut b: &[u8]) -> &[u8] {
    while let [rest @ .., b' ' | b'\t'] = b {
        b = rest;
    }
    b
}

/// Go's `getLine`: (the line without its end and trailing blanks, the rest, bytes consumed).
fn get_line(data: &[u8]) -> (&[u8], &[u8], usize) {
    let (mut i, j) = match data.iter().position(|&c| c == b'\n') {
        None => (data.len(), data.len()),
        Some(i) => (i, i + 1),
    };
    if j > i && i > 0 && data[i - 1] == b'\r' {
        i -= 1;
    }
    (trim_right_space_tab(&data[..i]), &data[j..], j)
}

/// Go's `pem.Decode`.
pub(crate) fn decode(data: &[u8]) -> (Option<Block>, &[u8]) {
    let mut rest = data;
    let mut end_trailer_index: isize = 0;
    loop {
        if end_trailer_index < 0 || end_trailer_index as usize > rest.len() {
            return (None, data);
        }
        rest = &rest[end_trailer_index as usize..];
        let Some(end_index) = find(rest, END) else {
            return (None, data);
        };
        let mut end_index = end_index as isize;
        end_trailer_index = end_index + END.len() as isize;
        let begin_index = match rfind(&rest[..end_index as usize], &START[1..]) {
            Some(b) if b == 0 || rest[b - 1] == b'\n' => b,
            _ => continue,
        };
        let skip = (begin_index + START.len() - 1) as isize;
        rest = &rest[skip as usize..];
        end_index -= skip;
        end_trailer_index -= skip;
        let (type_line, r, consumed) = get_line(rest);
        rest = r;
        end_index -= consumed as isize;
        end_trailer_index -= consumed as isize;
        let Some(type_line) = type_line.strip_suffix(EOL) else {
            continue;
        };
        let mut headers = false;
        loop {
            if rest.is_empty() {
                return (None, data);
            }
            let (line, next, consumed) = get_line(rest);
            if !line.contains(&b':') {
                break;
            }
            headers = true;
            rest = next;
            end_index -= consumed as isize;
            end_trailer_index -= consumed as isize;
        }
        if headers && end_index < 0 {
            continue;
        }
        // Go slices rest[endTrailerIndex:], which panics past the end; Decode is never handed such input (the end
        // marker was found inside rest), so a negative index here can only come from the headers' case above
        let Ok(eti) = usize::try_from(end_trailer_index) else {
            continue;
        };
        if eti > rest.len() {
            continue;
        }
        let end_trailer = &rest[eti..];
        let trailer_len = type_line.len() + EOL.len();
        if end_trailer.len() < trailer_len {
            continue;
        }
        let (head, rest_of_end_line) = end_trailer.split_at(trailer_len);
        if !head.starts_with(type_line) || !head.ends_with(EOL) {
            continue;
        }
        if !get_line(rest_of_end_line).0.is_empty() {
            continue;
        }
        let mut bytes = Vec::new();
        if end_index > 0 {
            let body: Vec<u8> = rest[..end_index as usize].iter().copied().filter(|&c| c != b' ' && c != b'\t').collect();
            match base64_std(&body) {
                Ok(b) => bytes = b,
                Err(_) => continue,
            }
        }
        let after = (end_index + END.len() as isize - 1) as usize;
        let (_, r, _) = get_line(&rest[after..]);
        return (Some(Block { kind: type_line.to_vec(), bytes }), r);
    }
}
