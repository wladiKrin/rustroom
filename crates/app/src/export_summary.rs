//! The Export dialog's section summaries: one short phrase per section, so the collapsed
//! sections read as a description of the whole export.
//!
//! ⚠ Pure, in the lib: a trailing text on a header is painted, which the harness cannot read,
//! so each phrase is testable here and the dialog only places it.

use crate::batch_export::ExportOptions;
use crate::export_presets::ExportPreset;

/// The folder's last component, `(none chosen)`, or the name with ` (missing)`.
pub fn location(o: &ExportOptions) -> String {
    use crate::batch_export::Destination;
    // ⚠⚠ Delivery folded in here on 2026-09-24: "where does this go" is **one** question, and it used
    // to be asked half at the top of the dialog and half at the bottom. `delivery()` is gone, not
    // merely unused.
    match o.destination {
        Destination::Nowhere => "(nowhere chosen)".to_string(),
        Destination::Target(_) => format!("Immich, {}", o.upload_date.label().to_lowercase()),
        Destination::Files => folder(o),
        Destination::Both(_) => format!("{} + Immich", folder(o)),
    }
}

/// The destination folder's last component, `(none chosen)`, or the name with ` (missing)`.
fn folder(o: &ExportOptions) -> String {
    if o.dest_dir.as_os_str().is_empty() {
        return "(none chosen)".to_string();
    }
    let name = o
        .dest_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| o.dest_dir.display().to_string());
    if o.dest_dir.is_dir() {
        name
    } else {
        format!("{name} (missing)")
    }
}

/// `JPEG · sRGB · 92` — the quality only for a lossy format.
pub fn file(o: &ExportOptions) -> String {
    let mut s = o.format.label().to_string();
    if o.format.is_lossy() {
        s.push_str(&format!(" · {}", o.quality));
    }
    s
}

/// The colour space and, where it applies, the rendering intent — the Colour section's trailer.
pub fn colour(o: &ExportOptions) -> String {
    // ⚠ DNG names its own fixed space: the dialog's choice does not apply to it.
    if let Some(fixed) = o.format.fixed_space() {
        return format!("linear {}", fixed.label());
    }
    let mut s = o.space.label().to_string();
    // ⚠ Not for ProPhoto: it is the working space and maps nothing, so naming an intent there
    // would describe a choice that has no effect.
    if o.space != color::OutputSpace::ProPhoto {
        s.push_str(&format!(" · {}", o.intent.label()));
    }
    s
}

/// `2048 px long edge` or `original`.
pub fn size(o: &ExportOptions) -> String {
    match o.max_edge {
        Some(e) => format!("{e} px long edge"),
        None => "original".to_string(),
    }
}

/// The amount, or `off` at zero.
pub fn sharpening(o: &ExportOptions) -> String {
    match o.output_sharpening.amount {
        0 => "off".to_string(),
        n => n.to_string(),
    }
}

pub fn demosaic(o: &ExportOptions) -> String {
    o.demosaic.label().to_string()
}

/// The scope's label, lowercased: `all metadata`, `copyright only`.
pub fn metadata(o: &ExportOptions) -> String {
    o.metadata.scope.label().to_lowercase()
}

/// The watermark's text, elided past 24 characters, or `off` when empty.
pub fn watermark(o: &ExportOptions) -> String {
    let t = o.watermark.text.trim();
    if t.is_empty() {
        return "off".to_string();
    }
    let n = t.chars().count();
    if n > 24 {
        let head: String = t.chars().take(23).collect();
        format!("{head}…")
    } else {
        t.to_string()
    }
}

/// The preset whose options equal `o`, if any.
pub fn matching_preset<'a>(presets: &'a [ExportPreset], o: &ExportOptions) -> Option<&'a str> {
    presets
        .iter()
        .find(|p| p.options == *o)
        .map(|p| p.name.as_str())
}

/// The footer's note: the preset the options equal, else `changed from “X”` when one was
/// picked, else nothing.
pub fn footer_note(presets: &[ExportPreset], selected: Option<&str>, o: &ExportOptions) -> String {
    // ⚠⚠ The reason wins over the preset name. Export is dark in this state, and a note reading
    // "Web" beside a dead button explains nothing — this project's rule is that an unusable control
    // goes dark, which only works when something says why.
    if o.destination.is_nowhere() {
        return "Choose at least one destination".to_string();
    }
    match (matching_preset(presets, o), selected) {
        (Some(m), _) => m.to_string(),
        (None, Some(s)) => format!("changed from “{s}”"),
        (None, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str, o: &ExportOptions) -> ExportPreset {
        ExportPreset {
            name: name.to_string(),
            options: o.clone(),
        }
    }

    #[test]
    fn each_section_has_a_phrase() {
        let mut o = ExportOptions {
            dest_dir: std::path::PathBuf::new(),
            ..Default::default()
        };
        assert_eq!(location(&o), "(none chosen)");
        o.dest_dir = test_dir::path("export-summary");
        assert!(!location(&o).ends_with("(missing)"), "{}", location(&o));
        o.dest_dir = std::path::PathBuf::from("/nonexistent/Export");
        assert_eq!(location(&o), "Export (missing)");

        o.max_edge = None;
        assert_eq!(size(&o), "original");
        o.max_edge = Some(2048);
        assert_eq!(size(&o), "2048 px long edge");

        o.output_sharpening.amount = 0;
        assert_eq!(sharpening(&o), "off");
        o.output_sharpening.amount = 30;
        assert_eq!(sharpening(&o), "30");

        o.watermark.text = "  ".into();
        assert_eq!(watermark(&o), "off");
        o.watermark.text = "© Wladi".into();
        assert_eq!(watermark(&o), "© Wladi");
        o.watermark.text = "a".repeat(40);
        assert_eq!(watermark(&o).chars().count(), 24);

        assert!(file(&o).contains(" · "));
        assert!(colour(&o).contains(" · "), "{}", colour(&o));
        o.format = export::ExportFormat::Dng;
        assert!(colour(&o).starts_with("linear "), "{}", colour(&o));
        assert!(!file(&o).contains(" · "), "{}", file(&o));
        assert_eq!(demosaic(&o), o.demosaic.label());
        assert_eq!(metadata(&o), o.metadata.scope.label().to_lowercase());
    }

    #[test]
    fn the_footer_names_the_matching_preset_or_the_drift() {
        let base = ExportOptions::default();
        let mut other = base.clone();
        other.quality = base.quality.wrapping_add(1);
        let presets = vec![preset("Web", &base)];
        assert_eq!(matching_preset(&presets, &base), Some("Web"));
        assert_eq!(footer_note(&presets, None, &base), "Web");
        assert_eq!(
            footer_note(&presets, Some("Web"), &other),
            "changed from “Web”"
        );
        assert_eq!(footer_note(&presets, None, &other), "");
    }

    /// ⚠ Every section header in this dialog says its own state, so a collapsed dialog reads as a
    /// description of the export — and since Delivery folded in, that includes where it is delivered.
    #[test]
    fn the_location_summary_covers_all_four_destinations() {
        use crate::batch_export::{Destination, UploadDate, UploadTarget};
        let mut o = ExportOptions {
            dest_dir: std::path::PathBuf::from("/nope/exports"),
            ..ExportOptions::default()
        };
        // ⚠ A folder that does not exist is marked, which is the pre-existing behaviour folding in
        // must not lose.
        assert_eq!(location(&o), "exports (missing)");

        o.destination = Destination::Target(UploadTarget::Immich);
        assert_eq!(location(&o), "Immich, original date");

        o.destination = Destination::Both(UploadTarget::Immich);
        o.upload_date = UploadDate::Uploaded;
        assert_eq!(location(&o), "exports (missing) + Immich");

        o.destination = Destination::Nowhere;
        assert_eq!(location(&o), "(nowhere chosen)");
    }

    /// ⚠⚠ The footer must say **why** Export is dark, or the dark button is unexplained.
    #[test]
    fn the_footer_note_explains_a_destination_of_nowhere() {
        use crate::batch_export::Destination;
        let o = ExportOptions {
            destination: Destination::Nowhere,
            ..ExportOptions::default()
        };
        assert_eq!(
            footer_note(&[], None, &o),
            "Choose at least one destination"
        );
        // ⚠ The reason outranks a matching preset name, which would otherwise be shown instead.
        let preset = ExportPreset {
            name: "Web".into(),
            options: o.clone(),
        };
        assert_eq!(
            footer_note(&[preset], Some("Web"), &o),
            "Choose at least one destination"
        );
        // ⚠ And with a real destination the note goes back to what it always said.
        let ok = ExportOptions {
            destination: Destination::Files,
            ..o
        };
        assert_eq!(footer_note(&[], None, &ok), "");
    }
}
