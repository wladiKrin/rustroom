//! TIFF IFD writing, shared by the two DNG writers — M9.10's rendered DNG and M10.17a's HDR one.
//! Moved out of `dng.rs` unchanged.

// TIFF field types.
pub(crate) const BYTE: u16 = 1;
pub(crate) const ASCII: u16 = 2;
pub(crate) const SHORT: u16 = 3;
pub(crate) const LONG: u16 = 4;
pub(crate) const RATIONAL: u16 = 5;
pub(crate) const SRATIONAL: u16 = 10;
pub(crate) const FLOAT: u16 = 11;

/// One IFD entry: tag, type, count and the value's bytes (little-endian).
pub(crate) struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    bytes: Vec<u8>,
}

pub(crate) fn ascii(tag: u16, s: &str) -> Entry {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    Entry {
        tag,
        kind: ASCII,
        count: bytes.len() as u32,
        bytes,
    }
}

pub(crate) fn shorts(tag: u16, v: &[u16]) -> Entry {
    Entry {
        tag,
        kind: SHORT,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

pub(crate) fn longs(tag: u16, v: &[u32]) -> Entry {
    Entry {
        tag,
        kind: LONG,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

pub(crate) fn bytes(tag: u16, v: &[u8]) -> Entry {
    Entry {
        tag,
        kind: BYTE,
        count: v.len() as u32,
        bytes: v.to_vec(),
    }
}

pub(crate) fn floats(tag: u16, v: &[f32]) -> Entry {
    Entry {
        tag,
        kind: FLOAT,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

/// Signed rationals with a fixed denominator of 10⁶ — six decimals is past a colour matrix's own
/// precision.
pub(crate) fn srationals(tag: u16, v: &[f32]) -> Entry {
    Entry {
        tag,
        kind: SRATIONAL,
        count: v.len() as u32,
        bytes: v
            .iter()
            .flat_map(|x| {
                let n = (*x as f64 * 1e6).round() as i32;
                [n.to_le_bytes(), 1_000_000i32.to_le_bytes()].concat()
            })
            .collect(),
    }
}

pub(crate) fn rationals(tag: u16, v: &[(u32, u32)]) -> Entry {
    Entry {
        tag,
        kind: RATIONAL,
        count: v.len() as u32,
        bytes: v
            .iter()
            .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
            .collect(),
    }
}

/// The size an IFD with `entries` occupies, its out-of-line values included.
pub(crate) fn ifd_size(entries: &[Entry]) -> usize {
    2 + 12 * entries.len()
        + 4
        + entries
            .iter()
            .filter(|e| e.bytes.len() > 4)
            .map(|e| e.bytes.len().div_ceil(2) * 2)
            .sum::<usize>()
}

/// Serialise an IFD placed at `at`, its out-of-line values right after it. Sorted by tag, as TIFF
/// requires.
pub(crate) fn ifd_bytes(mut entries: Vec<Entry>, at: usize) -> Vec<u8> {
    entries.sort_by_key(|e| e.tag);
    let mut head = Vec::new();
    let mut tail = Vec::new();
    let data_at = at + 2 + 12 * entries.len() + 4;
    head.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in &entries {
        head.extend_from_slice(&e.tag.to_le_bytes());
        head.extend_from_slice(&e.kind.to_le_bytes());
        head.extend_from_slice(&e.count.to_le_bytes());
        if e.bytes.len() <= 4 {
            let mut v = e.bytes.clone();
            v.resize(4, 0);
            head.extend_from_slice(&v);
        } else {
            head.extend_from_slice(&((data_at + tail.len()) as u32).to_le_bytes());
            tail.extend_from_slice(&e.bytes);
            if tail.len() % 2 == 1 {
                tail.push(0); // word-align, as TIFF asks
            }
        }
    }
    head.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
    head.extend_from_slice(&tail);
    head
}
