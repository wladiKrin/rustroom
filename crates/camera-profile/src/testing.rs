//! Builders shared by this crate's test modules.
//!
//! ⚠ One builder, not one per test module: two copies of a byte-level format writer drift, and a
//! drifted builder produces tests that pass against files nothing else can read.

/// Build a minimal little-endian DCP carrying the given IFD entries.
///
/// ⚠ Synthetic because real profiles are always self-consistent — the guards for malformed
/// ones can only be reached by constructing one, and an unreachable guard is an untested one.
pub fn synth(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
    let mut head = Vec::new();
    head.extend_from_slice(b"II");
    head.extend_from_slice(&0x4352u16.to_le_bytes());
    head.extend_from_slice(&8u32.to_le_bytes());
    let mut ifd = Vec::new();
    ifd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    // Payloads follow the IFD and its terminating next-offset word.
    let mut payload_at = 8 + 2 + entries.len() * 12 + 4;
    let mut payloads = Vec::new();
    for (tag, typ, cnt, bytes) in entries {
        ifd.extend_from_slice(&tag.to_le_bytes());
        ifd.extend_from_slice(&typ.to_le_bytes());
        ifd.extend_from_slice(&cnt.to_le_bytes());
        if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            ifd.extend_from_slice(&inline);
        } else {
            ifd.extend_from_slice(&(payload_at as u32).to_le_bytes());
            payloads.extend_from_slice(bytes);
            payload_at += bytes.len();
        }
    }
    ifd.extend_from_slice(&0u32.to_le_bytes());
    head.extend_from_slice(&ifd);
    head.extend_from_slice(&payloads);
    head
}

pub fn matrix_bytes(v: f32) -> Vec<u8> {
    let mut b = Vec::new();
    for _ in 0..9 {
        b.extend_from_slice(&((v * 10000.0) as i32).to_le_bytes());
        b.extend_from_slice(&10000i32.to_le_bytes());
    }
    b
}

/// A directory removed on drop, so tests never touch the real config directory.
pub struct TempDir(std::path::PathBuf);

impl TempDir {
    pub fn new() -> Self {
        Self(test_dir::path("profiles"))
    }
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
