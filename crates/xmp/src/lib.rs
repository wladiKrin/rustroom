//! Reading and writing the few XMP fields Rustroom shares with Lightroom: rating, colour label,
//! keywords and the four description fields, in a `stem.xmp` sidecar.
//!
//! ⚠ Zero GUI and zero catalog dependency: the app decides what a label *means* and where a
//! sidecar lives; this crate only turns text into a [`Sidecar`] and back.
//!
//! ⚠ `export::metadata::xmp_packet` also writes an XMP packet, for *export* metadata
//! (copyright, caption). It is deliberately not reused here — making `export` depend on this
//! crate, or the reverse, is a layering change the milestone does not need. Two small packet
//! skeletons is the honest cost, and both carry this note.

/// What a sidecar says about a photo, in the fields this crate owns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sidecar {
    /// `xmp:Rating`. `None` when the sidecar carries no rating.
    pub rating: Option<u8>,
    /// `xmp:Label`, as the raw XMP text. ⚠ Mapping it to a colour is the app's job — the
    /// catalog's four labels are not Adobe's five.
    pub label: Option<String>,
    /// Keyword paths, root first: `["Europe", "France", "Paris"]`. A flat `dc:subject` is a path
    /// of one.
    pub keywords: Vec<Vec<String>>,
    /// `dc:title`, the `x-default` alternative.
    pub title: Option<String>,
    /// `dc:description` — what the app calls the caption.
    pub caption: Option<String>,
    /// `dc:rights` — what the app calls the copyright.
    pub rights: Option<String>,
    /// `dc:creator`. ⚠ A multi-entry sequence reads as its **first** and is written back as one:
    /// the catalog holds one creator string, and a co-credit is not a field Rustroom has.
    pub creator: Option<String>,
    /// `exif:GPSLatitude` / `exif:GPSLongitude` as decimal degrees, south and west negative (M10.18b).
    ///
    /// ⚠⚠ **Read only — [`write`] never emits it**, and a merge keeps another program's untouched:
    /// Rustroom keeps the locations it sets in its catalog (the author's call, 2026-10-28).
    pub gps: Option<(f64, f64)>,
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
    /// ⚠ Unreachable in practice, and a variant rather than an `expect`: the merge slices the
    /// source at event boundaries, which are always character boundaries in a UTF-8 document.
    /// A panic in a file-reading path is never acceptable, so the impossible case is an error.
    #[error("internal: an event span did not land on a character boundary")]
    Internal,
}

/// Produce the text to write beside a photo.
///
/// ⚠⚠ **With `existing`, this MERGES — it never replaces.** A folder edited in Lightroom already
/// has `stem.xmp` files carrying that person's entire develop history (`crs:*`), and writing a
/// fresh packet over one would destroy it silently, with nothing to notice until they reopened the
/// folder in Lightroom. So the document is streamed through, the four owned fields are dropped
/// wherever they appear — attribute or element — ours are injected into the first
/// `rdf:Description`, and **everything else is copied byte for byte**.
///
/// ⚠ A fresh packet is the same code path applied to a built-in skeleton, so there is exactly one
/// injection implementation rather than two that can disagree.
pub fn write(sidecar: &Sidecar, existing: Option<&str>) -> Result<String, XmpError> {
    merge(existing.unwrap_or(SKELETON), sidecar)
}

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

const NS_XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/";
const NS_DC: &[u8] = b"http://purl.org/dc/elements/1.1/";
const NS_LR: &[u8] = b"http://ns.adobe.com/lightroom/1.0/";
const NS_RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_EXIF: &[u8] = b"http://ns.adobe.com/exif/1.0/";

/// `exif:GPSLatitude` (true) or `exif:GPSLongitude` (false) — read, never owned, so a merge-write keeps it.
fn gps_half(ns: &ResolveResult<'_>, local: &[u8]) -> Option<bool> {
    match (ns, local) {
        (ResolveResult::Bound(n), b"GPSLatitude") if n.as_ref() == NS_EXIF => Some(true),
        (ResolveResult::Bound(n), b"GPSLongitude") if n.as_ref() == NS_EXIF => Some(false),
        _ => None,
    }
}

/// XMP's GPS coordinate, `"DDD,MM.mmmmK"` or `"DDD,MM,SSK"` with K one of N S E W, as signed degrees.
pub fn parse_gps_coordinate(t: &str) -> Option<f64> {
    let t = t.trim();
    let (body, k) = t.split_at(t.len().checked_sub(1)?);
    let sign = match k {
        "N" | "E" | "n" | "e" => 1.0,
        "S" | "W" | "s" | "w" => -1.0,
        _ => return None,
    };
    let parts: Vec<f64> = body
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    let v = match parts.as_slice() {
        [d, m] => d + m / 60.0,
        [d, m, s] => d + m / 60.0 + s / 3600.0,
        _ => return None,
    };
    v.is_finite().then_some(sign * v)
}

/// The eight fields this crate owns, wherever and however they appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owned {
    Rating,
    Label,
    Subject,
    Hierarchical,
    Title,
    Description,
    Rights,
    Creator,
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
        (NS_DC, b"title") => Some(Owned::Title),
        (NS_DC, b"description") => Some(Owned::Description),
        (NS_DC, b"rights") => Some(Owned::Rights),
        (NS_DC, b"creator") => Some(Owned::Creator),
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
    /// A GPS half — `true` for the latitude.
    Gps(bool),
    Description,
    Li,
    Other,
}

fn classify(ns: &ResolveResult<'_>, e: &BytesStart<'_>) -> Kind {
    let local = e.local_name();
    if let Some(o) = owned(ns, local.as_ref()) {
        Kind::Owned(o)
    } else if let Some(lat) = gps_half(ns, local.as_ref()) {
        Kind::Gps(lat)
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

/// Set `slot` to `t` unless it already holds a value — the first `rdf:li` of an `Alt` or `Seq`
/// wins, and an attribute seen earlier is not overwritten by an element seen later.
fn first(slot: &mut Option<String>, t: String) {
    if slot.is_none() {
        *slot = Some(t);
    }
}

/// Read the owned attributes off an `rdf:Description` tag.
fn description_attributes(
    reader: &NsReader<&[u8]>,
    e: &BytesStart<'_>,
    out: &mut Sidecar,
    gps: &mut (Option<f64>, Option<f64>),
) -> Result<(), XmpError> {
    for a in e.attributes() {
        let a = a?;
        let (ns, local) = reader.resolver().resolve_attribute(a.key);
        // ⚠ `normalized_value`, not the deprecated `unescape_value`: it additionally normalises
        // line endings and tabs inside an attribute the way the XML specification requires.
        // `Implicit1_0` is the version to ask for — an XMP packet carries no XML declaration, and
        // the specification says 1.0 is then assumed. Caught by clippy's `deprecated` lint, which
        // a green test run says nothing about.
        let value = || a.normalized_value(quick_xml::XmlVersion::Implicit1_0);
        match gps_half(&ns, local.as_ref()) {
            Some(true) => gps.0 = parse_gps_coordinate(&value()?),
            Some(false) => gps.1 = parse_gps_coordinate(&value()?),
            None => {}
        }
        match owned(&ns, local.as_ref()) {
            Some(Owned::Rating) => out.rating = parse_rating(&value()?),
            Some(Owned::Label) => out.label = Some(value()?.into_owned()),
            Some(Owned::Title) => first(&mut out.title, value()?.into_owned()),
            Some(Owned::Description) => first(&mut out.caption, value()?.into_owned()),
            Some(Owned::Rights) => first(&mut out.rights, value()?.into_owned()),
            Some(Owned::Creator) => first(&mut out.creator, value()?.into_owned()),
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
    let mut gps: (Option<f64>, Option<f64>) = (None, None);
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
                            Some(Owned::Subject) => {
                                push_unique(&mut flat, split_path(&t).into_iter().take(1).collect())
                            }
                            Some(Owned::Title) => first(&mut out.title, t),
                            Some(Owned::Description) => first(&mut out.caption, t),
                            Some(Owned::Rights) => first(&mut out.rights, t),
                            Some(Owned::Creator) => first(&mut out.creator, t),
                            Some(Owned::Rating) | Some(Owned::Label) | None => {}
                        }
                    }
                    Kind::Gps(lat) => {
                        let v = parse_gps_coordinate(&text_of(&mut reader, &e)?);
                        if lat {
                            gps.0 = v;
                        } else {
                            gps.1 = v;
                        }
                    }
                    Kind::Description => {
                        description_attributes(&reader, &e, &mut out, &mut gps)?;
                        depth += 1;
                    }
                    Kind::Li | Kind::Other => depth += 1,
                }
            }
            Event::Empty(e) => {
                if classify(&ns, &e) == Kind::Description {
                    description_attributes(&reader, &e, &mut out, &mut gps)?;
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
    out.gps = match gps {
        (Some(la), Some(lo)) if la.abs() <= 90.0 && lo.abs() <= 180.0 => Some((la, lo)),
        _ => None,
    };
    Ok(out)
}

/// The document a fresh sidecar is merged into.
///
/// ⚠ Deliberately an *empty* `rdf:Description`: writing a fresh packet then exercises the same
/// re-opening path a real empty description takes, so that branch is covered by every write
/// rather than by one test.
const SKELETON: &str = concat!(
    "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n",
    "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Rustroom\">\n",
    " <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
    "  <rdf:Description rdf:about=\"\"/>\n",
    " </rdf:RDF>\n",
    "</x:xmpmeta>\n",
    "<?xpacket end=\"w\"?>\n",
);

/// The byte range of every attribute in a raw start tag, with its key, the range **including the
/// whitespace before it**.
///
/// ⚠⚠ Written here rather than taken from `quick-xml`, which does not expose attribute offsets —
/// and offsets are the whole point: removing an attribute by its span leaves every other byte of
/// the tag, its line breaks and its indentation exactly as the other application wrote them.
/// Rebuilding the tag from parsed attributes would reflow a Lightroom sidecar's `crs:` block onto
/// one line, which is a diff nobody wants to see against their own file.
///
/// ⚠ Stops at the first thing it does not understand, so a tag it cannot parse is simply left
/// whole. The conservative direction: the worst case is an owned field left in place, never a
/// destroyed one.
fn attribute_spans(raw: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let b = raw.as_bytes();
    let mut i = 0;
    if b.first() == Some(&b'<') {
        i += 1;
    }
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
        i += 1;
    }
    let mut out = Vec::new();
    loop {
        let ws = i;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] == b'>' || b[i] == b'/' {
            break;
        }
        let key_start = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key_end = i;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            break;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || (b[i] != b'"' && b[i] != b'\'') {
            break;
        }
        let quote = b[i];
        i += 1;
        while i < b.len() && b[i] != quote {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        i += 1;
        match raw.get(key_start..key_end) {
            Some(key) => out.push((ws..i, key)),
            None => break,
        }
    }
    out
}

/// `raw` — the start tag as written — with every owned attribute's span removed.
///
/// ⚠ Returns the raw text unchanged when nothing is owned, so a foreign `rdf:Description` (the
/// one inside a `crs:Look`, for instance) comes out byte for byte.
fn strip_owned_attributes(
    reader: &NsReader<&[u8]>,
    e: &BytesStart<'_>,
    raw: &str,
) -> Result<String, XmpError> {
    // ⚠ Iterated for its errors alone: a malformed attribute must surface as an `Err` rather
    // than being quietly stepped over by the scanner below.
    for a in e.attributes() {
        a?;
    }
    let mut out = String::with_capacity(raw.len());
    let mut at = 0;
    for (span, key) in attribute_spans(raw) {
        let (ns, local) = reader
            .resolver()
            .resolve_attribute(quick_xml::name::QName(key.as_bytes()));
        if owned(&ns, local.as_ref()).is_some() {
            out.push_str(raw.get(at..span.start).ok_or(XmpError::Internal)?);
            at = span.end;
        }
    }
    out.push_str(raw.get(at..).ok_or(XmpError::Internal)?);
    Ok(out)
}

fn esc(s: &str) -> String {
    quick_xml::escape::escape(s).into_owned()
}

/// A `dc:Bag` of `items` under `tag`, with `tag`'s namespace declared inline.
///
/// ⚠ Every injected element carries its own `xmlns:` declaration, so nothing here has to know
/// which prefixes the host document happens to bind — a re-declaration to the same URI is legal
/// and a document binding a prefix differently is shadowed only inside our own element.
fn bag(tag: &str, ns: &str, items: &[String]) -> String {
    let mut s = format!(
        "   <{tag} xmlns:{prefix}=\"{ns}\">\n    <rdf:Bag xmlns:rdf=\"{NS_RDF_STR}\">\n",
        prefix = tag.split(':').next().unwrap_or(tag)
    );
    for it in items {
        s.push_str(&format!("     <rdf:li>{}</rdf:li>\n", esc(it)));
    }
    s.push_str(&format!("    </rdf:Bag>\n   </{tag}>\n"));
    s
}

const NS_RDF_STR: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const NS_DC_STR: &str = "http://purl.org/dc/elements/1.1/";

/// A language-alternative property with one `x-default` entry — `dc:title`, `dc:description`,
/// `dc:rights`.
fn lang_alt(tag: &str, v: &str) -> String {
    format!(
        "   <{tag} xmlns:dc=\"{NS_DC_STR}\"><rdf:Alt xmlns:rdf=\"{NS_RDF_STR}\">\
         <rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></{tag}>\n",
        esc(v)
    )
}

/// A sequence with one entry — `dc:creator`.
fn seq_one(tag: &str, v: &str) -> String {
    format!(
        "   <{tag} xmlns:dc=\"{NS_DC_STR}\"><rdf:Seq xmlns:rdf=\"{NS_RDF_STR}\">\
         <rdf:li>{}</rdf:li></rdf:Seq></{tag}>\n",
        esc(v)
    )
}

/// The children injected into the first `rdf:Description`.
///
/// ⚠ Simple properties go in as **elements**, not attributes, which is what lets each one declare
/// its own namespace. Both forms are legal XMP and Lightroom, exiftool and this crate's own
/// reader all accept either.
///
/// ⚠ Keywords are written in **both** forms, Lightroom's own habit: the hierarchy in
/// `lr:hierarchicalSubject`, and each path's **leaf** in the flat `dc:subject` bag, which is all
/// an application that reads only the flat form can use.
fn injection(s: &Sidecar) -> String {
    let mut out = String::from("\n");
    if let Some(r) = s.rating {
        out.push_str(&format!(
            "   <xmp:Rating xmlns:xmp=\"{}\">{r}</xmp:Rating>\n",
            std::str::from_utf8(NS_XMP).unwrap_or_default()
        ));
    }
    if let Some(l) = &s.label {
        out.push_str(&format!(
            "   <xmp:Label xmlns:xmp=\"{}\">{}</xmp:Label>\n",
            std::str::from_utf8(NS_XMP).unwrap_or_default(),
            esc(l)
        ));
    }
    if let Some(t) = &s.title {
        out.push_str(&lang_alt("dc:title", t));
    }
    if let Some(c) = &s.caption {
        out.push_str(&lang_alt("dc:description", c));
    }
    if let Some(r) = &s.rights {
        out.push_str(&lang_alt("dc:rights", r));
    }
    if let Some(c) = &s.creator {
        out.push_str(&seq_one("dc:creator", c));
    }
    if !s.keywords.is_empty() {
        let mut flat: Vec<String> = Vec::new();
        for p in &s.keywords {
            if let Some(leaf) = p.last() {
                if !flat.contains(leaf) {
                    flat.push(leaf.clone());
                }
            }
        }
        let paths: Vec<String> = s.keywords.iter().map(|p| p.join("|")).collect();
        out.push_str(&bag(
            "dc:subject",
            std::str::from_utf8(NS_DC).unwrap_or_default(),
            &flat,
        ));
        out.push_str(&bag(
            "lr:hierarchicalSubject",
            std::str::from_utf8(NS_LR).unwrap_or_default(),
            &paths,
        ));
    }
    out.push_str("  ");
    out
}

/// Drop the trailing spaces and tabs a removed element left behind on its line.
fn trim_line_padding(out: &mut String) {
    while out.ends_with(' ') || out.ends_with('\t') {
        out.pop();
    }
}

fn merge(text: &str, s: &Sidecar) -> Result<String, XmpError> {
    let mut reader = NsReader::from_str(text);
    let mut out = String::with_capacity(text.len() + 256);
    // Everything before this byte has been emitted, or deliberately dropped.
    let mut pos = 0usize;
    let mut injected = false;
    let mut depth = 0usize;
    loop {
        let start = pos;
        let (ns, ev) = reader.read_resolved_event()?;
        // ⚠⚠ Classified **before anything else touches the reader**. `read_resolved_event`
        // hands back a `ResolveResult` that is a reborrow of `&mut self`, so while it is alive
        // the reader cannot even be read from — not `buffer_position`, not `resolver`. Reducing
        // it to a `Copy` value first is what makes the rest of the loop possible at all.
        let kind = match &ev {
            Event::Start(e) | Event::Empty(e) => classify(&ns, e),
            _ => Kind::Other,
        };
        drop(ns);
        pos = reader.buffer_position() as usize;
        let raw = text.get(start..pos).ok_or(XmpError::Internal)?;
        match ev {
            Event::Start(e) => {
                match kind {
                    Kind::Owned(_) => {
                        // ⚠ The whole subtree goes, not just the open tag: `dc:subject` holds a
                        // bag, and leaving its children behind would produce a stray `rdf:Bag`.
                        let end_name = e.to_end().into_owned();
                        reader.read_to_end(end_name.name())?;
                        pos = reader.buffer_position() as usize;
                        trim_line_padding(&mut out);
                    }
                    Kind::Description => {
                        let rewritten = strip_owned_attributes(&reader, &e, raw)?;
                        depth += 1;
                        out.push_str(&rewritten);
                        if !injected {
                            out.push_str(&injection(s));
                            injected = true;
                        }
                    }
                    _ => {
                        depth += 1;
                        out.push_str(raw);
                    }
                }
            }
            Event::Empty(e) => {
                match kind {
                    Kind::Owned(_) => {
                        trim_line_padding(&mut out);
                    }
                    Kind::Description => {
                        let rewritten = strip_owned_attributes(&reader, &e, raw)?;
                        if injected {
                            out.push_str(&rewritten);
                        } else {
                            // ⚠ `<rdf:Description …/>` has to be **opened** to hold children.
                            // The name is taken from the tag as written, so a document using a
                            // different prefix for the rdf namespace still closes correctly.
                            let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                            let head = rewritten
                                .trim_end()
                                .strip_suffix('>')
                                .map(str::trim_end)
                                .and_then(|t| t.strip_suffix('/'))
                                .ok_or(XmpError::Internal)?
                                .to_string();
                            out.push_str(head.trim_end());
                            out.push('>');
                            out.push_str(&injection(s));
                            out.push_str(&format!("</{name}>"));
                            injected = true;
                        }
                    }
                    _ => {
                        out.push_str(raw);
                    }
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                out.push_str(raw);
            }
            Event::Eof => break,
            _ => {
                out.push_str(raw);
            }
        }
    }
    if depth != 0 {
        return Err(XmpError::Unclosed);
    }
    if !injected {
        // ⚠ Refused rather than replaced with a fresh packet. Overwriting a document we did not
        // recognise is exactly the destruction the merge exists to prevent.
        return Err(XmpError::NoDescription);
    }
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

    /// The four description fields the way Lightroom writes them — as elements — beside a
    /// develop setting that must survive.
    const DESCRIBED: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:Exposure2012="+0.50">
   <dc:title>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Harbour at dusk</rdf:li>
    </rdf:Alt>
   </dc:title>
   <dc:description>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Boats &amp; nets</rdf:li>
    </rdf:Alt>
   </dc:description>
   <dc:rights>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">© 2026 W</rdf:li>
    </rdf:Alt>
   </dc:rights>
   <dc:creator>
    <rdf:Seq>
     <rdf:li>W</rdf:li>
     <rdf:li>Second Credit</rdf:li>
    </rdf:Seq>
   </dc:creator>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn the_four_description_fields_read_from_elements() {
        let s = read(DESCRIBED).unwrap();
        assert_eq!(s.title.as_deref(), Some("Harbour at dusk"));
        assert_eq!(
            s.caption.as_deref(),
            Some("Boats & nets"),
            "entity not unescaped"
        );
        assert_eq!(s.rights.as_deref(), Some("© 2026 W"));
        // ⚠ A multi-entry creator reads as its **first**: the catalog holds one creator string
        // and a co-credit is not a field Rustroom has. Spec §4's accepted caveat.
        assert_eq!(s.creator.as_deref(), Some("W"));
    }

    #[test]
    fn a_description_field_reads_from_an_attribute() {
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
   dc:title="Attr title" dc:rights="Attr rights"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        let s = read(text).unwrap();
        assert_eq!(s.title.as_deref(), Some("Attr title"));
        assert_eq!(s.rights.as_deref(), Some("Attr rights"));
        assert_eq!(s.caption, None);
    }

    #[test]
    fn a_fresh_packet_writes_the_description_fields_and_omits_unset_ones() {
        let s = Sidecar {
            title: Some("T".into()),
            caption: None,
            rights: Some("R & Co".into()),
            creator: Some("C".into()),
            ..Sidecar::default()
        };
        let text = write(&s, None).unwrap();
        assert!(text.contains("<dc:title"), "{text}");
        assert!(!text.contains("dc:description"), "{text}");
        assert!(text.contains("R &amp; Co"), "{text}");
        assert!(text.contains("<rdf:Seq"), "creator is a sequence:\n{text}");
        assert_eq!(read(&text).unwrap(), s, "did not survive the round trip");
    }

    #[test]
    fn merging_replaces_a_foreign_title_and_keeps_every_other_byte() {
        let ours = Sidecar {
            title: Some("Our title".into()),
            caption: None,
            ..Sidecar::default()
        };
        let out = write(&ours, Some(DESCRIBED)).unwrap();
        assert!(
            out.contains(r#"crs:Exposure2012="+0.50""#),
            "develop setting lost:\n{out}"
        );
        let back = read(&out).unwrap();
        assert_eq!(back.title.as_deref(), Some("Our title"));
        // ⚠ Owned fields we do not set are **removed**, not kept: the catalog holds the truth
        // (a newer sidecar imported before we ever wrote), and this is what lets a caption
        // deleted in Rustroom disappear from Lightroom — the rule the rating already follows.
        assert_eq!(
            back.caption, None,
            "a stale caption survived the merge:\n{out}"
        );
        assert_eq!(back.rights, None);
        assert_eq!(back.creator, None);
        assert_eq!(
            out.matches("<dc:title").count(),
            1,
            "a second title appeared:\n{out}"
        );
    }

    // ─── writing: a fresh packet ──────────────────────────────────────────────────────────

    fn sample() -> Sidecar {
        Sidecar {
            rating: Some(2),
            label: Some("Blue".into()),
            keywords: vec![
                vec!["Europe".into(), "France".into(), "Paris".into()],
                vec!["Cat".into()],
            ],
            ..Sidecar::default()
        }
    }

    #[test]
    fn a_fresh_packet_reads_back_as_written() {
        let s = sample();
        let text = write(&s, None).unwrap();
        assert_eq!(read(&text).unwrap(), s);
    }

    #[test]
    fn a_fresh_packet_writes_both_keyword_forms() {
        // ⚠ Both, because an application reading only the flat `dc:subject` bag must still see
        // the keywords — and one reading the hierarchy must get the tree rather than three roots.
        let text = write(&sample(), None).unwrap();
        assert!(text.contains("<dc:subject"), "no flat bag:\n{text}");
        assert!(
            text.contains("<lr:hierarchicalSubject"),
            "no hierarchy:\n{text}"
        );
        assert!(
            text.contains("<rdf:li>Europe|France|Paris</rdf:li>"),
            "the path was not joined:\n{text}"
        );
        // The flat bag carries the **leaf**, which is what a flat reader can act on.
        assert!(text.contains("<rdf:li>Paris</rdf:li>"), "no leaf:\n{text}");
        assert!(
            !text.contains("<rdf:li>Europe</rdf:li>"),
            "an interior level leaked into the flat bag:\n{text}"
        );
    }

    #[test]
    fn an_unset_field_is_absent_from_a_fresh_packet() {
        let text = write(&Sidecar::default(), None).unwrap();
        assert!(!text.contains("Rating"), "{text}");
        assert!(!text.contains("Label"), "{text}");
        assert!(!text.contains("subject"), "{text}");
        // And it is still a document this crate can read back.
        assert_eq!(read(&text).unwrap(), Sidecar::default());
    }

    #[test]
    fn a_keyword_with_markup_characters_is_escaped() {
        // ⚠ The failure this prevents is not cosmetic: an unescaped `&` makes the whole sidecar
        // unparseable, by us and by Lightroom.
        let s = Sidecar {
            rating: None,
            label: Some("R&D".into()),
            keywords: vec![vec!["Rock & Roll".into(), "<2>".into()]],
            ..Sidecar::default()
        };
        let text = write(&s, None).unwrap();
        assert!(text.contains("Rock &amp; Roll"), "{text}");
        assert_eq!(read(&text).unwrap(), s, "it did not survive the round trip");
    }

    // ─── writing: the merge ───────────────────────────────────────────────────────────────

    #[test]
    fn merging_keeps_every_foreign_byte_and_changes_only_the_rating() {
        // ⚠⚠ **The load-bearing test of the whole feature.** A folder edited in Lightroom already
        // has sidecars carrying that person's entire develop history; writing a fresh packet over
        // one destroys it silently, and nothing notices until they reopen the folder in Lightroom.
        let out = write(&sample(), Some(LIGHTROOM)).unwrap();

        let before: Vec<&str> = LIGHTROOM.lines().filter(|l| l.contains("crs:")).collect();
        let after: Vec<&str> = out.lines().filter(|l| l.contains("crs:")).collect();
        assert!(before.len() > 5, "the fixture lost its develop settings");
        assert_eq!(before, after, "a crs: line changed\n--- after ---\n{out}");

        let back = read(&out).unwrap();
        assert_eq!(back, sample(), "our own fields did not survive");
    }

    #[test]
    fn merging_replaces_owned_child_elements_in_place() {
        // The fixture already carries a rating attribute and both keyword bags. They must be
        // *replaced*, not joined by a second copy — two ratings in one description is a document
        // whose meaning depends on which one a reader happens to take.
        let out = write(&sample(), Some(LIGHTROOM)).unwrap();
        assert_eq!(out.matches("<dc:subject").count(), 1, "{out}");
        // ⚠ Matched on the tag's *start*, not on `hierarchicalSubject>`: the injected open tag
        // carries an inline `xmlns:lr=…` and therefore does not end in the bare name. A first
        // draft of this assertion demanded two and was simply wrong about correct code.
        assert_eq!(out.matches("<lr:hierarchicalSubject").count(), 1, "{out}");
        assert_eq!(out.matches("</lr:hierarchicalSubject>").count(), 1, "{out}");
        assert_eq!(out.matches("<xmp:Rating").count(), 1, "{out}");
        assert_eq!(
            out.matches("xmp:Rating=").count(),
            0,
            "the old attribute form survived:\n{out}"
        );
        assert_eq!(out.matches("xmp:Label=").count(), 0, "{out}");
        assert_eq!(read(&out).unwrap(), sample());
    }

    #[test]
    fn merging_into_an_empty_description_tag_opens_it() {
        let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" tiff:Make="FUJIFILM" xmlns:tiff="http://ns.adobe.com/tiff/1.0/"/>
 </rdf:RDF>
</x:xmpmeta>"#;
        let out = write(&sample(), Some(text)).unwrap();
        assert!(
            out.contains("tiff:Make=\"FUJIFILM\""),
            "a foreign attribute was lost:\n{out}"
        );
        assert!(
            out.contains("</rdf:Description>"),
            "the tag was not opened:\n{out}"
        );
        assert_eq!(read(&out).unwrap(), sample());
    }

    #[test]
    fn a_nested_foreign_description_is_left_alone() {
        // ⚠ `crs:Look` contains an `rdf:Description` of its own. Injecting into every description
        // rather than the first would put our rating inside somebody's look preset.
        let out = write(&sample(), Some(LIGHTROOM)).unwrap();
        assert_eq!(out.matches("<xmp:Rating").count(), 1, "{out}");
        assert!(out.contains("crs:Name=\"Adobe Color\""), "{out}");
    }

    #[test]
    fn a_document_with_no_description_is_refused() {
        // ⚠ Refused, never replaced: overwriting a file we did not recognise is precisely the
        // destruction the merge exists to prevent.
        let text = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><foo/></x:xmpmeta>";
        assert!(matches!(
            write(&sample(), Some(text)),
            Err(XmpError::NoDescription)
        ));
    }

    // ─── malformed input ──────────────────────────────────────────────────────────────────

    /// M10.18b: XMP's `DDD,MM.mmmmK`, in all four hemispheres, and the three-part form.
    #[test]
    fn xmp_reads_gps_in_all_four_hemispheres() {
        let near = |a: Option<f64>, b: f64| assert!((a.unwrap() - b).abs() < 1e-6, "{a:?} vs {b}");
        near(parse_gps_coordinate("38,42.834N"), 38.7139);
        near(parse_gps_coordinate("9,8.364W"), -9.1394);
        near(parse_gps_coordinate("33,52.1S"), -(33.0 + 52.1 / 60.0));
        near(parse_gps_coordinate("151,12.5E"), 151.0 + 12.5 / 60.0);
        near(parse_gps_coordinate("38,42,50.04N"), 38.7139);
        assert_eq!(parse_gps_coordinate("38.7"), None);
    }

    fn with_gps(lat: &str, lon: &str) -> String {
        format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
             <rdf:Description rdf:about=\"\" xmlns:exif=\"http://ns.adobe.com/exif/1.0/\" \
              xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"3\" exif:GPSLatitude=\"{lat}\">\
             <exif:GPSLongitude>{lon}</exif:GPSLongitude></rdf:Description></rdf:RDF></x:xmpmeta>"
        )
    }

    /// Both forms — the latitude an attribute, the longitude an element — as darktable and Lightroom
    /// write them.
    #[test]
    fn a_sidecars_gps_is_read_in_attribute_and_element_form() {
        let s = read(&with_gps("38,42.834N", "9,8.364W")).unwrap();
        let (la, lo) = s.gps.expect("a position");
        assert!((la - 38.7139).abs() < 1e-6 && (lo + 9.1394).abs() < 1e-6);
        assert_eq!(s.rating, Some(3));
    }

    /// ⚠⚠ The author's call: Rustroom never writes a position — but a merge keeps another program's.
    #[test]
    fn xmp_write_keeps_another_programs_gps_and_adds_none() {
        let theirs = with_gps("38,42.834N", "9,8.364W");
        let out = write(
            &Sidecar {
                rating: Some(5),
                gps: Some((1.0, 2.0)),
                ..Sidecar::default()
            },
            Some(&theirs),
        )
        .unwrap();
        let back = read(&out).unwrap();
        assert_eq!(back.rating, Some(5));
        let (la, _) = back.gps.expect("their position was kept");
        assert!((la - 38.7139).abs() < 1e-6, "ours was written: {la}");
        let fresh = write(
            &Sidecar {
                gps: Some((1.0, 2.0)),
                ..Sidecar::default()
            },
            None,
        )
        .unwrap();
        assert!(!fresh.contains("GPS"), "a position was written: {fresh}");
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        // The file is hand-editable and written by other applications; a panic here would take
        // the whole folder open down with it.
        assert!(read("<a><b></a>").is_err());
        assert!(write(&sample(), Some("<a><b></a>")).is_err());
    }

    #[test]
    fn an_unclosed_document_is_an_error() {
        // ⚠ A truncated file — half-written by an interrupted export, say — ends cleanly at EOF
        // with elements still open. Without the depth check it reads as an empty sidecar, and a
        // merge into it would silently drop everything after the truncation.
        assert!(matches!(read("<a><b></b>"), Err(XmpError::Unclosed)));
        assert!(matches!(
            write(&sample(), Some("<a><b></b>")),
            Err(XmpError::Unclosed)
        ));
    }
}
