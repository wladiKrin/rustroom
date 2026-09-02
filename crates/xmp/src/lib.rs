//! Reading and writing the few XMP fields Rustroom shares with Lightroom: rating, colour label
//! and keywords, in a `stem.xmp` sidecar.
//!
//! ⚠ Zero GUI and zero catalog dependency: the app decides what a label *means* and where a
//! sidecar lives; this crate only turns text into a [`Sidecar`] and back.
//!
//! ⚠ `export::metadata::xmp_packet` also writes an XMP packet, for *export* metadata
//! (copyright, caption). It is deliberately not reused here — making `export` depend on this
//! crate, or the reverse, is a layering change the milestone does not need. Two small packet
//! skeletons is the honest cost, and both carry this note.

/// What a sidecar says about a photo, in the fields this crate owns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidecar {
    /// `xmp:Rating`. `None` when the sidecar carries no rating.
    pub rating: Option<u8>,
    /// `xmp:Label`, as the raw XMP text. ⚠ Mapping it to a colour is the app's job — the
    /// catalog's four labels are not Adobe's five.
    pub label: Option<String>,
    /// Keyword paths, root first: `["Europe", "France", "Paris"]`. A flat `dc:subject` is a path
    /// of one.
    pub keywords: Vec<Vec<String>>,
}

/// Why a sidecar could not be read or rewritten.
///
/// ⚠ Never a panic: the file is hand-editable and may be written by any other application.
#[derive(Debug, thiserror::Error)]
pub enum XmpError {
    #[error("malformed XML: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("malformed XML: {0}")]
    Attr(#[from] quick_xml::events::attributes::AttrError),
    #[error("malformed XML: an element is still open at the end of the document")]
    Unclosed,
    #[error("not an XMP document: it has no rdf:Description to write into")]
    NoDescription,
}

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

const NS_XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/";
const NS_DC: &[u8] = b"http://purl.org/dc/elements/1.1/";
const NS_LR: &[u8] = b"http://ns.adobe.com/lightroom/1.0/";
const NS_RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The four fields this crate owns, wherever and however they appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owned {
    Rating,
    Label,
    Subject,
    Hierarchical,
}

/// Which owned field a resolved name is, if any.
///
/// ⚠⚠ **By namespace, never by prefix.** `xap:Rating` is the same property as `xmp:Rating` —
/// `xap` was Adobe's original prefix for the namespace and older files still carry it — and a
/// writer is free to bind any prefix at all. Matching the prefix would silently miss those files.
fn owned(ns: &ResolveResult<'_>, local: &[u8]) -> Option<Owned> {
    let ResolveResult::Bound(ns) = ns else {
        return None;
    };
    match (ns.as_ref(), local) {
        (NS_XMP, b"Rating") => Some(Owned::Rating),
        (NS_XMP, b"Label") => Some(Owned::Label),
        (NS_DC, b"subject") => Some(Owned::Subject),
        (NS_LR, b"hierarchicalSubject") => Some(Owned::Hierarchical),
        _ => None,
    }
}

fn is_rdf(ns: &ResolveResult<'_>, local: &[u8], want: &[u8]) -> bool {
    matches!(ns, ResolveResult::Bound(ns) if ns.as_ref() == NS_RDF) && local == want
}

/// What an element is to this crate, decided while the resolver's borrow is still live.
///
/// ⚠ `read_resolved_event` hands back a `ResolveResult` that borrows the **reader**, so nothing
/// can be read from the reader until it is dropped. Classifying into a `Copy` value first is what
/// lets the caller go on to `read_text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Owned(Owned),
    Description,
    Li,
    Other,
}

fn classify(ns: &ResolveResult<'_>, e: &BytesStart<'_>) -> Kind {
    let local = e.local_name();
    if let Some(o) = owned(ns, local.as_ref()) {
        Kind::Owned(o)
    } else if is_rdf(ns, local.as_ref(), b"Description") {
        Kind::Description
    } else if is_rdf(ns, local.as_ref(), b"li") {
        Kind::Li
    } else {
        Kind::Other
    }
}

/// The unescaped text content of the element `e` just opened, consuming its end tag.
fn text_of(reader: &mut NsReader<&[u8]>, e: &BytesStart<'_>) -> Result<String, XmpError> {
    let raw = reader.read_text(e.name())?;
    let raw = raw.decode().map_err(quick_xml::Error::from)?;
    let text = quick_xml::escape::unescape(&raw).map_err(quick_xml::Error::from)?;
    Ok(text.into_owned())
}

/// `"4"` → 4. ⚠ Also `"4.0"` and, clamped to zero, Bridge's `"-1"` for a reject: the flag does
/// not round-trip (spec §1), so a negative rating is simply "no stars".
fn parse_rating(t: &str) -> Option<u8> {
    let t = t.trim();
    let v = t
        .parse::<i64>()
        .ok()
        .or_else(|| t.parse::<f64>().ok().map(|f| f.round() as i64))?;
    Some(v.clamp(0, u8::MAX as i64) as u8)
}

/// `Europe|France|Paris` → the path. Empty components are dropped.
fn split_path(t: &str) -> Vec<String> {
    t.split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn push_unique(paths: &mut Vec<Vec<String>>, p: Vec<String>) {
    if !p.is_empty() && !paths.contains(&p) {
        paths.push(p);
    }
}

/// Read the owned attributes off an `rdf:Description` tag.
fn description_attributes(
    reader: &NsReader<&[u8]>,
    e: &BytesStart<'_>,
    out: &mut Sidecar,
) -> Result<(), XmpError> {
    for a in e.attributes() {
        let a = a?;
        let (ns, local) = reader.resolver().resolve_attribute(a.key);
        match owned(&ns, local.as_ref()) {
            Some(Owned::Rating) => out.rating = parse_rating(&a.unescape_value()?),
            Some(Owned::Label) => out.label = Some(a.unescape_value()?.into_owned()),
            _ => {}
        }
    }
    Ok(())
}

/// Parse a sidecar's text.
///
/// ⚠ Every `rdf:Description` is scanned, not only the first: exiftool writes one per namespace,
/// so the rating and the keywords can sit in different ones.
///
/// ⚠ Keywords: `lr:hierarchicalSubject` **wins** when it is present. Lightroom writes every
/// keyword in the flat `dc:subject` bag too, so reading both would hand back the hierarchy twice
/// — once as a tree and once as a set of roots.
pub fn read(text: &str) -> Result<Sidecar, XmpError> {
    let mut reader = NsReader::from_str(text);
    let mut out = Sidecar::default();
    let mut flat: Vec<Vec<String>> = Vec::new();
    let mut hier: Vec<Vec<String>> = Vec::new();
    // Which keyword bag the reader is inside, if any.
    let mut bag: Option<Owned> = None;
    let mut depth: usize = 0;
    loop {
        let (ns, ev) = reader.read_resolved_event()?;
        match ev {
            Event::Start(e) => {
                let kind = classify(&ns, &e);
                drop(ns);
                match kind {
                    Kind::Owned(Owned::Rating) => {
                        out.rating = parse_rating(&text_of(&mut reader, &e)?)
                    }
                    Kind::Owned(Owned::Label) => out.label = Some(text_of(&mut reader, &e)?),
                    Kind::Owned(o) => {
                        bag = Some(o);
                        depth += 1;
                    }
                    Kind::Li if bag.is_some() => {
                        let t = text_of(&mut reader, &e)?;
                        match bag {
                            Some(Owned::Hierarchical) => push_unique(&mut hier, split_path(&t)),
                            _ => {
                                push_unique(&mut flat, split_path(&t).into_iter().take(1).collect())
                            }
                        }
                    }
                    Kind::Description => {
                        description_attributes(&reader, &e, &mut out)?;
                        depth += 1;
                    }
                    Kind::Li | Kind::Other => depth += 1,
                }
            }
            Event::Empty(e) => {
                if classify(&ns, &e) == Kind::Description {
                    description_attributes(&reader, &e, &mut out)?;
                }
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                if let Some(b) = bag {
                    if owned(&ns, e.local_name().as_ref()) == Some(b) {
                        bag = None;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(XmpError::Unclosed);
    }
    out.keywords = if hier.is_empty() { flat } else { hier };
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sidecar the way Lightroom writes one: rating and label as attributes of the single
    /// `rdf:Description`, keywords in both forms.
    const LIGHTROOM: &str = r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="4"
   xmp:Label="Red"
   crs:Version="15.0"
   crs:Exposure2012="+0.50"
   crs:Contrast2012="+12">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Europe</rdf:li>
     <rdf:li>France</rdf:li>
     <rdf:li>Paris</rdf:li>
     <rdf:li>Street</rdf:li>
    </rdf:Bag>
   </dc:subject>
   <lr:hierarchicalSubject>
    <rdf:Bag>
     <rdf:li>Europe|France|Paris</rdf:li>
     <rdf:li>Street</rdf:li>
    </rdf:Bag>
   </lr:hierarchicalSubject>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>64, 56</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:Look>
    <rdf:Description
     crs:Name="Adobe Color"
     crs:Amount="1.000000"
     crs:UUID="B952C231111CD8E0EC4F9EA8236B0C09">
    <crs:Parameters>
     <rdf:Description
      crs:Version="15.0"
      crs:ProcessVersion="11.0"/>
    </crs:Parameters>
    </rdf:Description>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn a_rating_and_label_read_from_attributes() {
        let s = read(LIGHTROOM).unwrap();
        assert_eq!(s.rating, Some(4));
        assert_eq!(s.label.as_deref(), Some("Red"));
    }

    #[test]
    fn a_rating_and_label_read_from_elements() {
        // ⚠ Some writers put the properties in child elements rather than attributes — both
        // are legal XMP for a simple property, and Lightroom itself reads both.
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/">
   <xmp:Rating>3</xmp:Rating>
   <xmp:Label>Blue</xmp:Label>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = read(text).unwrap();
        assert_eq!(s.rating, Some(3));
        assert_eq!(s.label.as_deref(), Some("Blue"));
    }

    #[test]
    fn the_old_xap_prefix_is_the_same_namespace() {
        // ⚠ `xap` was Adobe's original prefix for `http://ns.adobe.com/xap/1.0/` and older files
        // still carry it. Matching on the prefix would miss them; matching on the namespace does
        // not.
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xap="http://ns.adobe.com/xap/1.0/" xap:Rating="2"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        assert_eq!(read(text).unwrap().rating, Some(2));
    }

    #[test]
    fn a_rating_in_a_second_description_is_found() {
        // ⚠ exiftool writes one `rdf:Description` per namespace, so the rating is not
        // necessarily in the first one.
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
   <dc:subject><rdf:Bag><rdf:li>Cat</rdf:li></rdf:Bag></dc:subject>
  </rdf:Description>
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/">
   <xmp:Rating>5</xmp:Rating>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = read(text).unwrap();
        assert_eq!(s.rating, Some(5));
        assert_eq!(s.keywords, vec![vec!["Cat".to_string()]]);
    }

    #[test]
    fn a_missing_rating_reads_as_none_rather_than_zero() {
        // The distinction the app relies on: absent is absent, not "rated zero".
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""/>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = read(text).unwrap();
        assert_eq!(s, Sidecar::default());
    }

    // ─── keywords ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn hierarchical_subjects_parse_to_paths() {
        let s = read(LIGHTROOM).unwrap();
        assert_eq!(
            s.keywords,
            vec![
                vec!["Europe".to_string(), "France".into(), "Paris".into()],
                vec!["Street".to_string()],
            ]
        );
    }

    #[test]
    fn a_flat_subject_is_a_path_of_one() {
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Paris</rdf:li>
     <rdf:li>Street</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = read(text).unwrap();
        assert_eq!(
            s.keywords,
            vec![vec!["Paris".to_string()], vec!["Street".to_string()]]
        );
    }

    #[test]
    fn the_hierarchical_form_wins_when_both_are_present() {
        // ⚠ The discriminating half of the two tests above: `LIGHTROOM` carries `Europe`,
        // `France` and `Paris` as three flat subjects too. Reading the flat bag as well would
        // yield six paths, and the hierarchy would arrive twice — once as a tree and once as
        // three root keywords.
        let s = read(LIGHTROOM).unwrap();
        assert_eq!(
            s.keywords.len(),
            2,
            "the flat bag leaked in: {:?}",
            s.keywords
        );
        assert!(s.keywords.iter().all(|p| !p.is_empty()));
    }

    #[test]
    fn an_entity_in_a_keyword_is_unescaped() {
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:lr="http://ns.adobe.com/lightroom/1.0/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Label="R&amp;D">
   <lr:hierarchicalSubject>
    <rdf:Bag>
     <rdf:li>Rock &amp; Roll|B&#252;hne &lt;2&gt;</rdf:li>
    </rdf:Bag>
   </lr:hierarchicalSubject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = read(text).unwrap();
        assert_eq!(
            s.keywords,
            vec![vec!["Rock & Roll".to_string(), "Bühne <2>".to_string()]]
        );
        assert_eq!(s.label.as_deref(), Some("R&D"));
    }
}
