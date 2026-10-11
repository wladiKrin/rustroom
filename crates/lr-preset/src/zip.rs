//! The entries of a `.zip` pack — enough of the format for preset packs: the central directory, and
//! **stored** or **deflate** entries (`flate2`, already in the build). An encrypted entry or another
//! method is counted, never read; a zip64 archive is refused.

use crate::Error;
use std::io::Read;

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

fn short() -> Error {
    Error::Malformed("the zip ends early".into())
}

fn u16_at(b: &[u8], at: usize) -> Result<usize, Error> {
    let s = b.get(at..at + 2).ok_or_else(short)?;
    Ok(u16::from_le_bytes([s[0], s[1]]) as usize)
}

fn u32_at(b: &[u8], at: usize) -> Result<usize, Error> {
    let s = b.get(at..at + 4).ok_or_else(short)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

/// Not a preset's file and not a failure: macOS's `__MACOSX/` copies and `._` resource forks.
fn is_mac_litter(path: &str) -> bool {
    path.starts_with("__MACOSX/")
        || path
            .rsplit('/')
            .next()
            .is_some_and(|base| base.starts_with("._"))
}

/// One file in a zip: its `/`-separated path and its bytes.
pub type ZipFile = (String, Vec<u8>);

/// The files in a zip — `(path, bytes)`, `/`-separated — and how many could not be read.
///
/// ⚠ macOS's `__MACOSX/` folder and its `._name` resource forks are skipped silently: they are not
/// presets and not failures, and a pack zipped on a Mac is full of them.
pub fn entries(bytes: &[u8]) -> Result<(Vec<ZipFile>, usize), Error> {
    // The end-of-central-directory record, searched from the end (a comment may follow it).
    let lowest = bytes.len().saturating_sub(22 + 65_535);
    let eocd = (lowest..=bytes.len().saturating_sub(22))
        .rev()
        .find(|&i| u32_at(bytes, i).ok() == Some(EOCD as usize))
        .ok_or_else(|| Error::Malformed("not a zip".into()))?;
    // ⚠ The archive's comment must be all there — GitHub writes the commit id into it, and a download
    // cut short inside it is still a truncated file.
    if eocd + 22 + u16_at(bytes, eocd + 20)? != bytes.len() {
        return Err(short());
    }
    let count = u16_at(bytes, eocd + 10)?;
    let cd_size = u32_at(bytes, eocd + 12)?;
    let cd_offset = u32_at(bytes, eocd + 16)?;
    if count == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
        return Err(Error::Malformed("a zip64 archive".into()));
    }
    let mut files = Vec::new();
    let mut unreadable = 0;
    let mut at = cd_offset;
    for _ in 0..count {
        if u32_at(bytes, at)? != CENTRAL as usize {
            return Err(Error::Malformed("a broken central directory".into()));
        }
        let flags = u16_at(bytes, at + 8)?;
        let method = u16_at(bytes, at + 10)?;
        let comp = u32_at(bytes, at + 20)?;
        let name_len = u16_at(bytes, at + 28)?;
        let extra_len = u16_at(bytes, at + 30)?;
        let comment_len = u16_at(bytes, at + 32)?;
        let local = u32_at(bytes, at + 42)?;
        let name = bytes.get(at + 46..at + 46 + name_len).ok_or_else(short)?;
        let path = String::from_utf8_lossy(name).replace('\\', "/");
        at += 46 + name_len + extra_len + comment_len;
        if path.ends_with('/') || is_mac_litter(&path) {
            continue;
        }
        if flags & 1 != 0 || !matches!(method, 0 | 8) {
            unreadable += 1;
            continue;
        }
        if u32_at(bytes, local)? != LOCAL as usize {
            return Err(Error::Malformed("a broken local header".into()));
        }
        let data_at = local + 30 + u16_at(bytes, local + 26)? + u16_at(bytes, local + 28)?;
        let data = bytes.get(data_at..data_at + comp).ok_or_else(short)?;
        let out = if method == 0 {
            data.to_vec()
        } else {
            let mut v = Vec::new();
            match flate2::read::DeflateDecoder::new(data).read_to_end(&mut v) {
                Ok(_) => v,
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            }
        };
        files.push((path, out));
    }
    Ok((files, unreadable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn archive() -> Vec<u8> {
        std::fs::read(fixtures().join("questtion.zip")).unwrap()
    }

    /// A zip of `files`, every one **stored** — for the cases a real archive does not have, built from
    /// a real file's bytes.
    fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let offset = out.len() as u32;
            let crc = 0u32; // Not checked by the reader.
            let local = |sig: u32, out: &mut Vec<u8>| out.extend_from_slice(&sig.to_le_bytes());
            local(0x0403_4b50, &mut out);
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            local(0x0201_4b50, &mut central);
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// ⚠⚠ GitHub's real archive: every preset comes out under its folder, byte for byte the file
    /// committed beside it — which proves the deflate path, not only the stored one.
    #[test]
    fn the_real_archive_gives_its_presets_byte_for_byte() {
        let (files, unreadable) = entries(&archive()).unwrap();
        assert_eq!(unreadable, 0);
        let xmps: Vec<&ZipFile> = files.iter().filter(|(p, _)| p.ends_with(".xmp")).collect();
        assert_eq!(xmps.len(), 60, "the sample: the pack's 60 presets");
        for name in ["Preset 4.xmp", "Play Prom Try 2.xmp", "BW 2 +1.2.xmp"] {
            let (path, bytes) = xmps
                .iter()
                .find(|(p, _)| p.ends_with(&format!("/Presets/{name}")))
                .unwrap_or_else(|| panic!("{name} missing"));
            let own = std::fs::read(fixtures().join("questtion").join(name)).unwrap();
            assert_eq!(bytes, &own, "{path} differs from the committed copy");
        }
    }

    /// A pack zipped on a Mac: the resource forks are skipped, not counted, and the preset comes out.
    #[test]
    fn mac_resource_forks_are_skipped() {
        let real = std::fs::read(fixtures().join("questtion/Preset 4.xmp")).unwrap();
        let zip = stored_zip(&[
            ("Pack/Warm.xmp", &real),
            ("__MACOSX/Pack/._Warm.xmp", b"\\x00\\x05\\x16\\x07"),
            ("Pack/._Warm.xmp", b"\\x00\\x05\\x16\\x07"),
            ("Pack/", b""),
        ]);
        let (files, unreadable) = entries(&zip).unwrap();
        assert_eq!(unreadable, 0);
        assert_eq!(
            files.len(),
            1,
            "{:?}",
            files.iter().map(|f| &f.0).collect::<Vec<_>>()
        );
        assert_eq!(files[0].0, "Pack/Warm.xmp");
        assert_eq!(files[0].1, real);
    }

    /// A truncated archive is an error, never a panic; so is something that is not a zip.
    #[test]
    fn a_truncated_archive_is_an_error() {
        let zip = archive();
        for cut in [10, zip.len() / 2, zip.len() - 5] {
            assert!(entries(&zip[..cut]).is_err(), "cut at {cut}");
        }
        assert!(entries(b"not a zip").is_err());
    }
}
