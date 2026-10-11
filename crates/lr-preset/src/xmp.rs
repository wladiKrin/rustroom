//! A `.xmp` develop preset: the `crs:` settings of its description, its name and group, its curves.

use crate::{Error, Raw, Value};
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

const NS_CRS: &[u8] = b"http://ns.adobe.com/camera-raw-settings/1.0/";
const NS_RDF: &[u8] = b"http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// A `crs:` element under the preset's description, while it is read.
struct Child {
    name: String,
    depth: usize,
    /// The texts of its `rdf:li` items.
    items: Vec<String>,
    /// Its own text, for the simple form `<crs:Exposure2012>+0.30</crs:Exposure2012>`.
    text: String,
    /// It holds structure (a mask, a Look) rather than a value.
    nested: bool,
    /// A Look's name, from a `crs:Name` attribute inside it.
    look_name: Option<String>,
}

fn malformed(e: impl std::fmt::Display) -> Error {
    Error::Malformed(e.to_string())
}

/// A value as an `.xmp` writes it: a number (`"+0.30"` included), `True`/`False`, or text.
pub(crate) fn parse_value(v: &str) -> Value {
    let t = v.trim();
    if let Ok(n) = t.trim_start_matches('+').parse::<f64>() {
        if !t.is_empty() {
            return Value::Num(n);
        }
    }
    match t {
        "True" | "true" => Value::Bool(true),
        "False" | "false" => Value::Bool(false),
        _ => Value::Text(v.to_string()),
    }
}

/// The numbers of a curve's `"x, y"` items, flattened.
fn curve(items: &[String]) -> Value {
    Value::List(
        items
            .iter()
            .flat_map(|i| {
                i.split(',')
                    .map(str::trim)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter_map(|n| n.parse::<f64>().ok().map(Value::Num))
            .collect(),
    )
}

/// Read a `.xmp` preset's text.
///
/// ⚠ Only the **top** description's `crs:` attributes are the preset's settings: a Look and a mask
/// carry descriptions of their own, whose `crs:Name` and amounts must not overwrite the preset's.
pub fn read(text: &str) -> Result<Raw, Error> {
    let mut reader = NsReader::from_str(text);
    let mut raw = Raw::default();
    let mut found = false;
    let mut depth = 0usize;
    // The depth of `rdf:RDF`, and of the description directly under it.
    let mut rdf_depth: Option<usize> = None;
    let mut top: Option<usize> = None;
    let mut child: Option<Child> = None;
    loop {
        let (ns, ev) = reader.read_resolved_event().map_err(malformed)?;
        let ns: Option<Vec<u8>> = match ns {
            ResolveResult::Bound(n) => Some(n.as_ref().to_vec()),
            _ => None,
        };
        let is = |want_ns: &[u8], local: &[u8], got: &[u8]| {
            ns.as_deref() == Some(want_ns) && got == local
        };
        let opened = matches!(ev, Event::Start(_));
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let local = e.local_name().as_ref().to_vec();
                // Attributes: the preset's settings on the top description; a Look's name inside it.
                let at_top = top.is_none()
                    && is(NS_RDF, b"Description", &local)
                    && rdf_depth.is_some_and(|d| depth == d + 1);
                if at_top || child.as_ref().is_some_and(|c| c.name == "Look") {
                    for a in e.attributes() {
                        let a = a.map_err(malformed)?;
                        let (ans, alocal) = reader.resolver().resolve_attribute(a.key);
                        if !matches!(ans, ResolveResult::Bound(n) if n.as_ref() == NS_CRS) {
                            continue;
                        }
                        let key = String::from_utf8_lossy(alocal.as_ref()).into_owned();
                        let v = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(malformed)?
                            .into_owned();
                        if at_top {
                            raw.settings.insert(key, parse_value(&v));
                            found = true;
                        } else if key == "Name" {
                            if let Some(c) = child.as_mut() {
                                c.look_name.get_or_insert(v);
                            }
                        }
                    }
                }
                if at_top && opened {
                    top = Some(depth);
                }
                if is(NS_RDF, b"RDF", &local) && opened && rdf_depth.is_none() {
                    rdf_depth = Some(depth);
                }
                if let Some(c) = child.as_mut() {
                    if is(NS_RDF, b"li", &local) {
                        if opened {
                            let inner = reader.read_text(e.name()).map_err(malformed)?;
                            let inner = inner.decode().map_err(malformed)?;
                            if inner.contains('<') {
                                c.nested = true;
                            } else {
                                let t = quick_xml::escape::unescape(&inner).map_err(malformed)?;
                                c.items.push(t.into_owned());
                            }
                            // ⚠ `read_text` consumed the end tag: the depth is unchanged.
                            continue;
                        }
                        c.items.push(String::new());
                    } else if !is(NS_RDF, b"Seq", &local)
                        && !is(NS_RDF, b"Alt", &local)
                        && !is(NS_RDF, b"Bag", &local)
                    {
                        c.nested = true;
                    }
                } else if top.is_some_and(|t| depth == t + 1) && ns.as_deref() == Some(NS_CRS) {
                    let name = String::from_utf8_lossy(&local).into_owned();
                    if opened {
                        child = Some(Child {
                            name,
                            depth,
                            items: Vec::new(),
                            text: String::new(),
                            nested: false,
                            look_name: None,
                        });
                    }
                }
                if opened {
                    depth += 1;
                }
            }
            Event::Text(t) => {
                if let Some(c) = child.as_mut() {
                    if depth == c.depth + 1 {
                        let d = t.decode().map_err(malformed)?;
                        c.text
                            .push_str(&quick_xml::escape::unescape(&d).map_err(malformed)?);
                    }
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| malformed("an end with no start"))?;
                if child.as_ref().is_some_and(|c| c.depth == depth) {
                    let c = child.take().expect("checked");
                    found = true;
                    finish(&mut raw, c);
                }
                if top == Some(depth) {
                    top = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(malformed("the file ends early"));
    }
    if !found {
        return Err(Error::NotAPreset("it holds no Camera Raw settings"));
    }
    if raw.settings.get("PresetType").and_then(Value::text) == Some("Look") {
        return Err(Error::NotAPreset("it is a profile, not a develop preset"));
    }
    Ok(raw)
}

/// Store a finished `crs:` child.
fn finish(raw: &mut Raw, c: Child) {
    let first = || {
        c.items
            .iter()
            .chain(std::iter::once(&c.text))
            .map(|t| t.trim())
            .find(|t| !t.is_empty())
            .map(str::to_string)
    };
    match c.name.as_str() {
        "Name" => raw.name = first(),
        "Group" => raw.group = first(),
        n if n.starts_with("ToneCurve") && !c.items.is_empty() => {
            raw.settings.insert(c.name.clone(), curve(&c.items));
        }
        "Look" => {
            let name = c.look_name.clone().or_else(first).unwrap_or_default();
            raw.settings.insert(c.name, Value::Text(name));
        }
        _ if c.nested || !c.items.is_empty() => {
            raw.settings.insert(c.name, Value::Bool(true));
        }
        _ => {
            let v = parse_value(&c.text);
            raw.settings.insert(c.name, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;

    fn fixture(name: &str) -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/questtion")
            .join(name);
        std::fs::read_to_string(p).unwrap()
    }

    /// A real preset: its name, its settings as numbers, booleans and text, its curve's points.
    #[test]
    fn a_real_preset_gives_its_name_settings_and_curve() {
        let r = read(&fixture("Preset 4.xmp")).unwrap();
        assert_eq!(r.name.as_deref(), Some("Preset 4"));
        assert_eq!(r.group, None, "its group is empty");
        assert_eq!(r.settings["ColorGradeGlobalHue"], Value::Num(297.0));
        assert_eq!(
            r.settings["ColorGradeMidtoneLum"],
            Value::Num(3.0),
            "\"+3\" is a number"
        );
        assert!(r.settings["HasSettings"].as_bool());
        assert_eq!(
            r.settings["ToneCurveName2012"],
            Value::Text("Custom".into())
        );
        assert_eq!(
            r.settings["ToneCurvePV2012"].numbers(),
            [0.0, 2.0, 53.0, 53.0, 149.0, 168.0, 255.0, 255.0]
        );
        assert!(r.settings.contains_key("ToneCurvePV2012Red"));
        assert!(r.settings.contains_key("Look"), "its Look is recorded");
    }

    #[test]
    fn white_balance_and_hsl_come_through() {
        let r = read(&fixture("Play Prom Try 2.xmp")).unwrap();
        assert_eq!(r.settings["Temperature"], Value::Num(4200.0));
        assert_eq!(r.settings["HueAdjustmentRed"], Value::Num(10.0));
        assert_eq!(r.settings["WhiteBalance"], Value::Text("Custom".into()));
        assert_eq!(
            r.settings["CameraProfile"],
            Value::Text("Camera Standard".into())
        );
    }

    /// A group the file names is read — a real file with its empty group filled in.
    #[test]
    fn a_named_group_is_read() {
        let text = fixture("Preset 4.xmp");
        let named = text.replacen(
            "<crs:Group>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\"/>",
            "<crs:Group>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">Film</rdf:li>",
            1,
        );
        assert_ne!(named, text, "the sample: the group was filled in");
        assert_eq!(read(&named).unwrap().group.as_deref(), Some("Film"));
    }

    /// A Lightroom profile and a photo's sidecar are not presets.
    #[test]
    fn a_profile_or_a_sidecar_is_not_a_preset() {
        let profile = fixture("Preset 4.xmp").replacen(
            "crs:PresetType=\"Normal\"",
            "crs:PresetType=\"Look\"",
            1,
        );
        assert!(matches!(read(&profile), Err(Error::NotAPreset(_))));
        let sidecar = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"3\"/></rdf:RDF></x:xmpmeta>";
        assert!(matches!(read(sidecar), Err(Error::NotAPreset(_))));
    }

    /// No name: `None`, for the caller to name it after the file; CRLF line ends read the same.
    #[test]
    fn a_nameless_preset_and_crlf_line_ends() {
        let text = fixture("BW 2 +1.2.xmp");
        let crlf = text.replace('\n', "\r\n");
        assert_eq!(read(&crlf).unwrap(), read(&text).unwrap());
        let start = text.find("<crs:Name>").unwrap();
        let end = text.find("</crs:Name>").unwrap() + "</crs:Name>".len();
        let nameless = format!("{}{}", &text[..start], &text[end..]);
        assert_eq!(read(&nameless).unwrap().name, None);
    }

    /// A truncated file is an error, never a panic.
    #[test]
    fn a_truncated_file_is_an_error() {
        let text = fixture("Open Mic Night 3.xmp");
        for cut in [10, text.len() / 3, text.len() - 20] {
            assert!(read(&text[..cut]).is_err(), "cut at {cut}");
        }
    }
}
