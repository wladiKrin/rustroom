//! Rights and description written **into the exported file, and only there**.
//!
//! ⚠⚠ **THE ORIGINAL IS NEVER OPENED FOR WRITING, AND THAT IS WHAT DISSOLVED THE DESIGN
//! CONFLICT `docs/BACKLOG.md` HELD FOR MONTHS.** EXIF/IPTC editing was blocked on a choice
//! between *modifying the original* — which violates the non-destructive model Rustroom is
//! built on — and *a second XMP sidecar beside the JSON one*, which reopens the interop
//! question `02-architecture-and-tech` parked. Writing only into the export is neither: the
//! deliverable carries its credit, the negative is untouched, and nothing new appears on disk
//! beside the photographs.
//!
//! ⚠ The consequence, stated so it is not later found as a gap: these fields describe the
//! *export*, not the photograph. Nothing reads them back, they are not in the sidecar, and
//! re-exporting is what changes them.
//!
//! # Why XMP, and why hand-written
//!
//! ⚠⚠ **`kamadak-exif`, already in the tree, is a READER.** It has no writer, and neither does
//! the `image` crate — its encoders expose exactly one metadata hook, `set_icc_profile`. So
//! writing anything at all needed either a new dependency or container surgery.
//!
//! ⚠ **`img-parts` 0.4 was the obvious candidate and was rejected on coverage, not on cost.**
//! It is pure Rust, MIT/Apache-2.0, and its three dependencies (`bytes`, `crc32fast`,
//! `miniz_oxide`) are all already in this tree — so it would have been nearly free. But it
//! handles **JPEG, PNG and RIFF only**, and two of the five formats this crate writes are
//! TIFF. Adopting it would have left TIFF hand-rolled anyway, for a *second* code path rather
//! than one, and the JPEG/PNG halves are the easy two-thirds of the job.
//!
//! ⚠ XMP rather than EXIF/IPTC because one packet serves all three containers: an APP1 segment
//! in JPEG, an `iTXt` chunk in PNG, tag 700 in TIFF. EXIF would mean an IFD in JPEG, no
//! standard home in PNG, and IPTC-IIM is a fourth encoding again. It is also what Lightroom
//! writes, so the fields land where other software looks for them.

use crate::{ExportError, ExportFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How much of what the catalog knows goes into the exported file.
///
/// ⚠ **`All` is the default, for existing presets too (user's call).** This is the one
/// deliberate exception to the M6.1 seam's "defaults to do nothing" rule: an existing user's
/// next export gains camera, exposure, keywords and rating — which is the point of the
/// milestone, and what every other tool does by default. Recorded in spec §3.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataScope {
    /// Title, caption, rights, creator, rating, label, keywords and the capture facts.
    #[default]
    All,
    /// Rights and creator only — for a client who should not see the shoot's keywords.
    CopyrightOnly,
}

impl MetadataScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All metadata",
            Self::CopyrightOnly => "Copyright only",
        }
    }
}

/// Rights and description to write into every exported file, and how much else goes with them.
///
/// ⚠ Empty means "write nothing", not "write an empty string" — a blank copyright tag is worse
/// than an absent one, because a reader cannot tell it from a deliberate assertion of no rights.
///
/// ⚠ Since M6.2 these three strings are **defaults**: a photo's own title, caption, copyright or
/// creator (its catalog `Description`) wins over them — see [`FileMetadata::resolve`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportMetadata {
    pub copyright: String,
    pub creator: String,
    pub caption: String,
    pub scope: MetadataScope,
}

impl ExportMetadata {
    /// Whether the three default strings are all blank.
    ///
    /// ⚠ Trimmed, so a field holding only spaces counts as absent. A tag whose whole content is
    /// whitespace is the blank-copyright problem in a costume.
    pub fn is_empty(&self) -> bool {
        self.copyright.trim().is_empty()
            && self.creator.trim().is_empty()
            && self.caption.trim().is_empty()
    }

    /// Whether the section holds anything other than its defaults — what the export dialog's
    /// header dot shows.
    ///
    /// ⚠ Not "whether a packet will be written": under `All` one almost always is, and a dot
    /// that is always on is as useless as one that is never on.
    pub fn is_active(&self) -> bool {
        !self.is_empty() || self.scope != MetadataScope::default()
    }
}

/// The largest XMP packet a JPEG APP1 segment can carry.
///
/// A JPEG segment's length field is a `u16` covering itself, so the payload is at most
/// `65535 - 2`, and the XMP namespace header eats 29 of that. Beyond it the standard answer is
/// ExtendedXMP, a second segment with a GUID — deliberately not built, because reaching this
/// needs ~65 KB of caption and the honest failure is better than the machinery.
const JPEG_MAX_PACKET: usize = 65535 - 2 - XMP_NS.len();

/// The APP1 marker the XMP specification requires, NUL included.
const XMP_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// The PNG text keyword the XMP specification requires.
const PNG_KEYWORD: &[u8] = b"XML:com.adobe.xmp";

/// TIFF tag 700 — `XMP`, as the DNG and TIFF/EP specifications name it.
const TIFF_TAG_XMP: u16 = 700;

/// Escape a string for XML *element content*.
///
/// ⚠ Only the three that matter in content. Quotes are not escaped because nothing here is
/// written into an attribute — and an escaper that silently covers a case it is not used for
/// is a claim nobody checks.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// The XMP packet for `meta`, or `None` when there is nothing to say.
///
/// ⚠ The properties are the Dublin Core ones Lightroom writes, so the values land where other
/// software already looks: `dc:rights` (exiftool "Rights"), `dc:creator` ("Creator"),
/// `dc:description` ("Description").
///
/// ⚠ `rights` and `description` are language alternatives (`rdf:Alt` with `x-default`) while
/// `creator` is an ordered list (`rdf:Seq`) — that asymmetry is the Dublin Core schema's, not a
/// slip: a caption can be translated, an author cannot.
pub fn xmp_packet(meta: &ExportMetadata) -> Option<String> {
    if meta.is_empty() {
        return None;
    }
    let mut body = String::new();
    let alt = |tag: &str, v: &str| {
        format!(
            "   <{tag}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{tag}>\n",
            escape(v)
        )
    };
    if !meta.copyright.trim().is_empty() {
        body.push_str(&alt("dc:rights", meta.copyright.trim()));
    }
    if !meta.creator.trim().is_empty() {
        body.push_str(&format!(
            "   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n",
            escape(meta.creator.trim())
        ));
    }
    if !meta.caption.trim().is_empty() {
        body.push_str(&alt("dc:description", meta.caption.trim()));
    }
    Some(format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Rustroom\">\n\
         \x20<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20 <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
         {body}\
         \x20 </rdf:Description>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"w\"?>"
    ))
}

/// Write `meta` into the file at `path`, which `save` has just encoded.
///
/// ⚠ A **post-pass over the encoded bytes**, not a hook inside the encoder, because the `image`
/// crate offers no such hook for anything but ICC. The file is read, rewritten and replaced;
/// the pixels are never re-encoded, so this cannot change what the photograph looks like.
///
/// ⚠ A no-op when `meta` is empty — including no read and no write — so an export by anyone who
/// has not filled the fields in is byte-for-byte what it was before this existed.
pub fn embed(path: &Path, format: ExportFormat, meta: &ExportMetadata) -> Result<(), ExportError> {
    let Some(packet) = xmp_packet(meta) else {
        return Ok(());
    };
    let bytes = std::fs::read(path).map_err(image::ImageError::IoError)?;
    let out = match format {
        ExportFormat::Jpeg => jpeg_with_xmp(&bytes, packet.as_bytes())?,
        ExportFormat::Png8 | ExportFormat::Png16 => png_with_xmp(&bytes, packet.as_bytes())?,
        ExportFormat::Tiff8 | ExportFormat::Tiff16 => tiff_with_xmp(&bytes, packet.as_bytes())?,
    };
    std::fs::write(path, out).map_err(image::ImageError::IoError)?;
    Ok(())
}

// ─── JPEG ──────────────────────────────────────────────────────────────────────────────────

/// Insert an XMP APP1 segment after the leading APP0, or after SOI when there is none.
///
/// ⚠ The placement was chosen from what our own encoder actually emits, not from the
/// specification alone: `image`'s JPEG encoder writes **SOI, APP0 (JFIF), APP2 (ICC)**, and
/// JFIF must stay first. Dumping those bytes is what settled it — see `examples/probe.rs`.
pub fn jpeg_with_xmp(bytes: &[u8], packet: &[u8]) -> Result<Vec<u8>, ExportError> {
    if bytes.len() < 2 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(ExportError::Metadata("not a JPEG".into()));
    }
    if packet.len() > JPEG_MAX_PACKET {
        return Err(ExportError::Metadata(format!(
            "metadata is {} bytes, more than a JPEG APP1 segment can carry ({JPEG_MAX_PACKET})",
            packet.len()
        )));
    }
    // Skip any leading APP0 segments. Anything else — APP2/ICC included — the XMP goes before.
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF && bytes[at + 1] == 0xE0 {
        let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if len < 2 {
            return Err(ExportError::Metadata("malformed JPEG segment".into()));
        }
        at += 2 + len;
    }
    if at > bytes.len() {
        return Err(ExportError::Metadata("truncated JPEG".into()));
    }

    let payload_len = XMP_NS.len() + packet.len();
    let mut out = Vec::with_capacity(bytes.len() + payload_len + 4);
    out.extend_from_slice(&bytes[..at]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    // ⚠ The length field counts **itself** and the payload, but not the two marker bytes.
    out.extend_from_slice(&((payload_len + 2) as u16).to_be_bytes());
    out.extend_from_slice(XMP_NS);
    out.extend_from_slice(packet);
    out.extend_from_slice(&bytes[at..]);
    Ok(out)
}

// ─── PNG ───────────────────────────────────────────────────────────────────────────────────

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Insert an XMP `iTXt` chunk before the first `IDAT`.
///
/// ⚠ Before `IDAT` rather than at the end: the PNG specification allows a text chunk anywhere
/// after `IHDR`, but a reader that streams and stops at the image data would never see one
/// placed after it.
pub fn png_with_xmp(bytes: &[u8], packet: &[u8]) -> Result<Vec<u8>, ExportError> {
    if bytes.len() < 8 || bytes[..8] != PNG_SIG {
        return Err(ExportError::Metadata("not a PNG".into()));
    }
    let mut at = 8;
    let idat = loop {
        if at + 8 > bytes.len() {
            return Err(ExportError::Metadata("PNG has no image data".into()));
        }
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let kind = &bytes[at + 4..at + 8];
        if kind == b"IDAT" {
            break at;
        }
        at = at
            .checked_add(12 + len)
            .ok_or_else(|| ExportError::Metadata("malformed PNG chunk".into()))?;
    };

    // iTXt: keyword \0 compression-flag compression-method language \0 translated \0 text.
    // ⚠ Compression flag 0 — uncompressed. The XMP specification *requires* it: a compressed
    // XMP packet is invisible to readers that scan for the packet header rather than parse the
    // chunk, which is how several of them work.
    let mut data = Vec::with_capacity(PNG_KEYWORD.len() + packet.len() + 5);
    data.extend_from_slice(PNG_KEYWORD);
    data.extend_from_slice(&[0, 0, 0, 0, 0]);
    data.extend_from_slice(packet);

    let mut out = Vec::with_capacity(bytes.len() + data.len() + 12);
    out.extend_from_slice(&bytes[..idat]);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"iTXt");
    crc.update(&data);
    out.extend_from_slice(b"iTXt");
    out.extend_from_slice(&data);
    // ⚠ The CRC covers the chunk **type and data**, never the length. A CRC over the length as
    // well is self-consistent and rejected by every decoder.
    out.extend_from_slice(&crc.finalize().to_be_bytes());
    out.extend_from_slice(&bytes[idat..]);
    Ok(out)
}

// ─── TIFF ──────────────────────────────────────────────────────────────────────────────────

/// Append an XMP value and a rewritten IFD0 carrying tag 700, then point the header at it.
///
/// ⚠⚠ **The IFD is REWRITTEN AT THE END OF THE FILE rather than grown in place, and that is the
/// whole trick.** Adding an entry makes IFD0 twelve bytes longer, which would shift every byte
/// after it and invalidate every offset in the file. Appending a fresh copy and patching the
/// header's IFD0 pointer moves nothing: entries whose values live outside the IFD keep pointing
/// at data that has not moved, and the old IFD bytes are simply orphaned.
///
/// ⚠ BigTIFF (magic 43) is left untouched rather than corrupted. `image`'s encoder writes
/// classic TIFF, so this cannot happen today — the check is what stops it happening silently if
/// that ever changes.
pub fn tiff_with_xmp(bytes: &[u8], packet: &[u8]) -> Result<Vec<u8>, ExportError> {
    if bytes.len() < 8 {
        return Err(ExportError::Metadata("not a TIFF".into()));
    }
    let le = match &bytes[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(ExportError::Metadata("not a TIFF".into())),
    };
    let u16_at = |i: usize| {
        let b = [bytes[i], bytes[i + 1]];
        if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    };
    let u32_at = |i: usize| {
        let b: [u8; 4] = bytes[i..i + 4].try_into().unwrap();
        if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    if u16_at(2) != 42 {
        return Err(ExportError::Metadata(
            "not a classic TIFF — BigTIFF carries no XMP here".into(),
        ));
    }
    let ifd = u32_at(4) as usize;
    if ifd + 2 > bytes.len() {
        return Err(ExportError::Metadata("TIFF IFD is out of range".into()));
    }
    let count = u16_at(ifd) as usize;
    let entries_at = ifd + 2;
    let next_at = entries_at + count * 12;
    if next_at + 4 > bytes.len() {
        return Err(ExportError::Metadata("TIFF IFD is truncated".into()));
    }

    let mut out = bytes.to_vec();
    // ⚠ Every TIFF offset must be even. Nothing here needs the alignment, but readers exist
    // that assume it, and a pad byte is free.
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let xmp_off = out.len() as u32;
    out.extend_from_slice(packet);
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let new_ifd = out.len() as u32;

    // The existing entries, minus any tag 700 already present, plus ours — ascending by tag,
    // which the TIFF specification requires.
    let mut kept: Vec<(u16, [u8; 12])> = Vec::with_capacity(count + 1);
    for i in 0..count {
        let e = entries_at + i * 12;
        let tag = u16_at(e);
        if tag == TIFF_TAG_XMP {
            continue;
        }
        kept.push((tag, bytes[e..e + 12].try_into().unwrap()));
    }
    let mut ours = [0u8; 12];
    let put16 = |dst: &mut [u8], v: u16| {
        dst.copy_from_slice(&if le { v.to_le_bytes() } else { v.to_be_bytes() })
    };
    let put32 = |dst: &mut [u8], v: u32| {
        dst.copy_from_slice(&if le { v.to_le_bytes() } else { v.to_be_bytes() })
    };
    put16(&mut ours[0..2], TIFF_TAG_XMP);
    // Type 1 = BYTE, which is what the XMP specification names for tag 700.
    put16(&mut ours[2..4], 1);
    put32(&mut ours[4..8], packet.len() as u32);
    put32(&mut ours[8..12], xmp_off);
    kept.push((TIFF_TAG_XMP, ours));
    kept.sort_by_key(|(t, _)| *t);

    let mut hdr = [0u8; 2];
    put16(&mut hdr, kept.len() as u16);
    out.extend_from_slice(&hdr);
    for (_, e) in &kept {
        out.extend_from_slice(e);
    }
    // ⚠ The original next-IFD pointer, carried across. Dropping it would silently truncate a
    // multi-page TIFF to its first page.
    out.extend_from_slice(&bytes[next_at..next_at + 4]);

    let mut off = [0u8; 4];
    put32(&mut off, new_ifd);
    out[4..8].copy_from_slice(&off);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    fn meta(copyright: &str, creator: &str, caption: &str) -> ExportMetadata {
        ExportMetadata {
            copyright: copyright.into(),
            creator: creator.into(),
            caption: caption.into(),
            scope: MetadataScope::All,
        }
    }

    #[test]
    fn the_scope_is_all_unless_changed_and_activity_follows_either() {
        // ⚠ `All` is the default, for existing presets too (user's call, spec §3): an export
        // gains camera, keywords and rating from now on. That is *not* hidden state, so the
        // header dot must not show for it — `is_active` is "a field filled OR the scope
        // changed", the rule `a_section_that_is_doing_something_says_so_in_its_header` guards.
        let m = ExportMetadata::default();
        assert_eq!(m.scope, MetadataScope::All);
        assert!(!m.is_active(), "an untouched section must not claim activity");
        let filled = ExportMetadata {
            copyright: "(c)".into(),
            ..ExportMetadata::default()
        };
        assert!(filled.is_active());
        let narrowed = ExportMetadata {
            scope: MetadataScope::CopyrightOnly,
            ..ExportMetadata::default()
        };
        assert!(narrowed.is_active(), "a changed scope is state worth showing");
    }

    #[test]
    fn a_preset_written_before_the_scope_existed_loads_as_all() {
        // The exact JSON M6.1b wrote. Container-level `#[serde(default)]` carries the new field.
        let json = r#"{"copyright":"(c) me","creator":"","caption":""}"#;
        let m: ExportMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(m.scope, MetadataScope::All);
        assert_eq!(m.copyright, "(c) me");
    }

    #[test]
    fn an_empty_metadata_writes_no_packet() {
        assert!(xmp_packet(&ExportMetadata::default()).is_none());
        // ⚠ Whitespace counts as absent, or a stray space produces a blank copyright tag —
        // worse than none, because nothing can tell it from a deliberate assertion.
        assert!(xmp_packet(&meta("  ", "\t", "\n")).is_none());
    }

    #[test]
    fn each_field_lands_in_its_own_property() {
        let p = xmp_packet(&meta("(c) 2026 W", "Wladi", "A cat")).unwrap();
        // ⚠ Asserted as `<property>…value…</property>`, not merely "the string appears" — the
        // discriminating sabotage is swapping two fields, which a bare `contains` cannot see.
        assert!(
            p.contains("<dc:rights><rdf:Alt><rdf:li xml:lang=\"x-default\">(c) 2026 W</rdf:li>"),
            "{p}"
        );
        assert!(
            p.contains("<dc:creator><rdf:Seq><rdf:li>Wladi</rdf:li></rdf:Seq></dc:creator>"),
            "{p}"
        );
        assert!(
            p.contains("<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">A cat</rdf:li>"),
            "{p}"
        );
    }

    #[test]
    fn an_absent_field_produces_no_element() {
        let p = xmp_packet(&meta("(c) me", "", "")).unwrap();
        assert!(p.contains("dc:rights"));
        assert!(!p.contains("dc:creator"), "{p}");
        assert!(!p.contains("dc:description"), "{p}");
    }

    #[test]
    fn xml_special_characters_are_escaped() {
        // ⚠ An unescaped `&` makes the whole packet ill-formed, so every reader drops *all*
        // the metadata rather than one field — a silent total loss from one ampersand.
        let p = xmp_packet(&meta("Smith & Sons <2026>", "", "")).unwrap();
        assert!(p.contains("Smith &amp; Sons &lt;2026&gt;"), "{p}");
        assert!(!p.contains("Smith & Sons"), "{p}");
    }

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1).max(1) as f32;
                img.set_pixel(x, y, [v, 1.0 - v, 0.5]);
            }
        }
        img
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let i = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("rustroom_meta_{}_{i}_{name}", std::process::id()))
    }

    /// Encode `img`, embed `m`, and hand back the file's bytes and the pixels a decoder reads.
    ///
    /// ⚠ Decoding the *result* is the load-bearing half: a container edited with a wrong length,
    /// a wrong CRC or a dangling offset still contains the marker string, so a byte search alone
    /// would pass on a file no reader can open.
    fn round_trip(fmt: ExportFormat, m: &ExportMetadata) -> (Vec<u8>, Vec<u8>) {
        let img = ramp(9, 5);
        let path = tmp(&format!("rt.{}", fmt.extension()));
        crate::save(&img, &path, fmt, 95, color::OutputSpace::Srgb).unwrap();
        let before = image::open(&path).unwrap().to_rgb8().into_raw();
        embed(&path, fmt, m).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let after = image::open(&path).unwrap().to_rgb8().into_raw();
        assert_eq!(
            before, after,
            "{fmt:?}: embedding metadata changed the pixels"
        );
        let _ = std::fs::remove_file(&path);
        (bytes, after)
    }

    #[test]
    fn every_format_carries_the_packet_and_still_decodes() {
        let m = meta("(c) 2026 Rustroom", "Wladi", "Probe");
        for fmt in ExportFormat::ALL {
            let (bytes, _) = round_trip(fmt, &m);
            // ⚠ `x:xmpmeta`, not the `http://ns.adobe.com/xap/1.0/` URL — that string is the
            // **JPEG APP1 segment header**, and appears in no other container. Asserting on it
            // is a test that claims to be about XMP and is really about one format's framing.
            let needle = b"<x:xmpmeta";
            assert!(
                bytes.windows(needle.len()).any(|w| w == needle),
                "{fmt:?}: no XMP packet in the file"
            );
            let c = b"(c) 2026 Rustroom";
            assert!(
                bytes.windows(c.len()).any(|w| w == c),
                "{fmt:?}: the copyright is not in the file"
            );
        }
    }

    #[test]
    fn an_empty_metadata_leaves_the_file_byte_identical() {
        // ⚠ The whole guard against "harmless" rewriting: an export by anyone who has not
        // filled these fields in must be exactly what it was before this module existed.
        for fmt in ExportFormat::ALL {
            let img = ramp(7, 4);
            let path = tmp(&format!("noop.{}", fmt.extension()));
            crate::save(&img, &path, fmt, 95, color::OutputSpace::Srgb).unwrap();
            let before = std::fs::read(&path).unwrap();
            embed(&path, fmt, &ExportMetadata::default()).unwrap();
            let after = std::fs::read(&path).unwrap();
            assert_eq!(before, after, "{fmt:?}");
            let _ = std::fs::remove_file(&path);
        }
    }

    /// An IFD reader written **independently of the writer above**, so it can witness a bug in
    /// it — the same reason the golden harness keeps its own copy of the quantizer.
    fn tiff_tag_700(bytes: &[u8]) -> Option<Vec<u8>> {
        let le = &bytes[..2] == b"II";
        let g16 = |i: usize| {
            let b = [bytes[i], bytes[i + 1]];
            if le {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            }
        };
        let g32 = |i: usize| {
            let b: [u8; 4] = bytes[i..i + 4].try_into().unwrap();
            if le {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        };
        let ifd = g32(4) as usize;
        let n = g16(ifd) as usize;
        let mut prev = 0u16;
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            let tag = g16(e);
            assert!(tag >= prev, "IFD entries are not in ascending tag order");
            prev = tag;
            if tag == 700 {
                // ⚠ The count is at `e + 4` and the value at `e + 8`. Reading both from `e + 8`
                // is the first thing this reader got wrong, and it failed loudly — which is the
                // argument for writing it independently rather than calling the writer's helpers.
                let count = g32(e + 4) as usize;
                // ⚠ A value of four bytes or fewer lives *inside* the entry rather than at an
                // offset. An XMP packet never is, so the writer always uses an offset — this
                // asserts that premise rather than assuming it.
                assert!(count > 4, "an XMP value short enough to be inline");
                let at = g32(e + 8) as usize;
                return Some(bytes[at..at + count].to_vec());
            }
        }
        None
    }

    #[test]
    fn a_tiff_gains_tag_700_and_keeps_its_other_tags() {
        let img = ramp(9, 5);
        let path = tmp("tags.tif");
        crate::save(
            &img,
            &path,
            ExportFormat::Tiff8,
            95,
            color::OutputSpace::Srgb,
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let n_before = {
            let le = &before[..2] == b"II";
            let ifd = if le {
                u32::from_le_bytes(before[4..8].try_into().unwrap())
            } else {
                u32::from_be_bytes(before[4..8].try_into().unwrap())
            } as usize;
            if le {
                u16::from_le_bytes([before[ifd], before[ifd + 1]])
            } else {
                u16::from_be_bytes([before[ifd], before[ifd + 1]])
            }
        };
        assert!(tiff_tag_700(&before).is_none(), "fixture check: no XMP yet");

        embed(&path, ExportFormat::Tiff8, &meta("(c) me", "", "")).unwrap();
        let after = std::fs::read(&path).unwrap();
        let value = tiff_tag_700(&after).expect("tag 700 after embedding");
        assert!(String::from_utf8_lossy(&value).contains("(c) me"));

        // ⚠ The entry count must go up by exactly one. Losing an existing tag — the ICC
        // profile, the strip offsets — would still leave a file that opens, so "it decodes"
        // cannot see it.
        let le = &after[..2] == b"II";
        let ifd = if le {
            u32::from_le_bytes(after[4..8].try_into().unwrap())
        } else {
            u32::from_be_bytes(after[4..8].try_into().unwrap())
        } as usize;
        let n_after = if le {
            u16::from_le_bytes([after[ifd], after[ifd + 1]])
        } else {
            u16::from_be_bytes([after[ifd], after[ifd + 1]])
        };
        assert_eq!(
            n_after,
            n_before + 1,
            "the IFD lost or gained the wrong tags"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn embedding_twice_replaces_rather_than_duplicates() {
        // ⚠ TIFF only: it is the one container where the writer has to *remove* a previous
        // value, since JPEG and PNG both let a reader take the first packet it finds.
        let img = ramp(9, 5);
        let path = tmp("twice.tif");
        crate::save(
            &img,
            &path,
            ExportFormat::Tiff8,
            95,
            color::OutputSpace::Srgb,
        )
        .unwrap();
        embed(&path, ExportFormat::Tiff8, &meta("first", "", "")).unwrap();
        embed(&path, ExportFormat::Tiff8, &meta("second", "", "")).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let v = String::from_utf8_lossy(&tiff_tag_700(&bytes).unwrap()).into_owned();
        assert!(v.contains("second"), "{v}");
        assert!(!v.contains("first"), "{v}");
        let _ = std::fs::remove_file(&path);
    }

    /// CRC-32/ISO-HDLC, written out here so it can witness a bug in the writer's use of
    /// `crc32fast` — the reason the golden harness keeps its own copy of the quantizer.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn the_png_chunk_declares_a_correct_length_and_checksum() {
        // ⚠⚠ **THIS TEST EXISTS BECAUSE A SABOTAGE WENT UNCAUGHT.** Corrupting the CRC left the
        // whole suite green: the `image` crate's PNG decoder does not verify an ancillary
        // chunk's checksum, so "the file still decodes" cannot see it — while `exiftool` and
        // every strict reader would reject the chunk and show no metadata at all.
        let (bytes, _) = round_trip(ExportFormat::Png8, &meta("(c) me", "", ""));
        let mut at = 8;
        loop {
            assert!(at + 8 <= bytes.len(), "no iTXt chunk found");
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            if kind == b"iTXt" {
                let data = &bytes[at + 8..at + 8 + len];
                assert!(data.starts_with(PNG_KEYWORD), "the wrong text chunk");
                assert!(
                    String::from_utf8_lossy(data).ends_with("<?xpacket end=\"w\"?>"),
                    "the declared length does not cover the packet exactly"
                );
                let stored =
                    u32::from_be_bytes(bytes[at + 8 + len..at + 12 + len].try_into().unwrap());
                assert_eq!(
                    stored,
                    crc32(&bytes[at + 4..at + 8 + len]),
                    "the chunk checksum is wrong"
                );
                break;
            }
            at += 12 + len;
        }
    }

    #[test]
    fn a_jpeg_segment_declares_its_own_length() {
        let (bytes, _) = round_trip(ExportFormat::Jpeg, &meta("(c) me", "", ""));
        // Find the XMP APP1 and check the declared length matches what follows.
        let mut at = 2;
        loop {
            assert!(at + 4 <= bytes.len(), "no APP1 found");
            let (marker, len) = (
                bytes[at + 1],
                u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize,
            );
            if marker == 0xE1 {
                let payload = &bytes[at + 4..at + 2 + len];
                assert!(payload.starts_with(XMP_NS), "APP1 is not the XMP one");
                assert!(
                    String::from_utf8_lossy(payload).ends_with("<?xpacket end=\"w\"?>"),
                    "the declared length does not cover the packet exactly"
                );
                break;
            }
            at += 2 + len;
        }
    }

    #[test]
    fn a_packet_too_large_for_a_jpeg_is_refused_rather_than_truncated() {
        // ⚠ Refused, never trimmed: a silently truncated XMP packet is ill-formed XML, so the
        // whole of the metadata is lost rather than the tail of the caption.
        let huge = "x".repeat(JPEG_MAX_PACKET + 1);
        let err = jpeg_with_xmp(&[0xFF, 0xD8, 0xFF, 0xD9], huge.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("APP1"), "{err}");
        // ⚠ And the other two containers have no such limit, which is why the check is here
        // rather than in `embed`.
        assert!(
            tiff_with_xmp(&[], huge.as_bytes()).is_err(),
            "empty is not a TIFF"
        );
    }

    #[test]
    fn a_file_that_is_not_the_format_it_claims_is_refused() {
        assert!(jpeg_with_xmp(b"not a jpeg", b"x").is_err());
        assert!(png_with_xmp(b"not a png at all", b"x").is_err());
        assert!(tiff_with_xmp(b"nope", b"x").is_err());
    }
}
