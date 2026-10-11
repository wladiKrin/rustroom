//! Lightroom develop presets read into Rustroom edits (M10.9c): `.xmp` (Lightroom Classic 7.3+,
//! Lightroom, Camera Raw), the older `.lrtemplate`, and `.zip` packs of either.
//!
//! ⚠ No GUI. Two readers produce one [`Settings`] map; the mapping onto `edit_model::Edits` is one
//! table. The look will not match Lightroom exactly — the processing differs — and what has no
//! counterpart here is named, never silently dropped.

pub mod lrtemplate;
pub mod map;
mod value;
pub mod xmp;
pub mod zip;

pub use map::Mapped;
pub use value::{Error, Raw, Settings, Value};

/// One preset ready to save: its name (the file's when it names none), its Lightroom group, its edits.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub name: String,
    pub group: Option<String>,
    pub mapped: Mapped,
}

/// What one file held.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub presets: Vec<Imported>,
    /// Presets that set only what Rustroom cannot apply (a mask alone) — with what that was.
    pub carried_nothing: Vec<(String, Vec<&'static str>)>,
    /// Files that read but are not develop presets — a profile, a sidecar, another file type.
    pub not_presets: usize,
    /// Files that could not be read at all.
    pub unreadable: usize,
}

/// Read one file — `.xmp`, `.lrtemplate` or `.zip` — by its name's extension.
pub fn read(file_name: &str, bytes: &[u8]) -> Report {
    let mut report = Report::default();
    read_into(&mut report, file_name, bytes, None);
    report
}

/// The part of a `/`-separated path after its last `/`.
fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn extension(path: &str) -> String {
    let base = base_name(path);
    base.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Read `bytes` into `report`; `folder` is the folder a zip holds it in — its group when it names none.
fn read_into(report: &mut Report, path: &str, bytes: &[u8], folder: Option<String>) {
    let raw = match extension(path).as_str() {
        "xmp" => xmp::read(&String::from_utf8_lossy(bytes)),
        "lrtemplate" => lrtemplate::read_bytes(bytes),
        "zip" => {
            match zip::entries(bytes) {
                Ok((files, unreadable)) => {
                    report.unreadable += unreadable;
                    for (inner, data) in files {
                        // ⚠ Only presets: a pack's README, licence and sample photos are not failures.
                        if !matches!(extension(&inner).as_str(), "xmp" | "lrtemplate") {
                            continue;
                        }
                        let parent = inner
                            .rsplit_once('/')
                            .map(|(dir, _)| base_name(dir).to_string())
                            .filter(|d| !d.is_empty());
                        read_into(report, &inner, &data, parent);
                    }
                }
                Err(_) => report.unreadable += 1,
            }
            return;
        }
        _ => {
            report.not_presets += 1;
            return;
        }
    };
    match raw {
        Ok(raw) => {
            let base = base_name(path);
            let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
            let name = raw.name.clone().unwrap_or_else(|| stem.to_string());
            let mapped = map::to_edits(&raw.settings);
            if mapped.groups == edit_model::EditGroups::NONE {
                report.carried_nothing.push((name, mapped.left_out));
                return;
            }
            report.presets.push(Imported {
                name,
                group: raw.group.or(folder),
                mapped,
            });
        }
        Err(Error::NotAPreset(_)) => report.not_presets += 1,
        Err(Error::Malformed(_)) => report.unreadable += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(rel),
        )
        .unwrap()
    }

    #[test]
    fn a_real_xmp_is_one_preset() {
        let r = read("Preset 4.xmp", &fixture("questtion/Preset 4.xmp"));
        assert_eq!(r.presets.len(), 1);
        assert_eq!(r.presets[0].name, "Preset 4");
        assert_eq!(r.presets[0].group, None);
        assert!(r.presets[0].mapped.groups.tone);
    }

    /// ⚠⚠ The real pack: every preset read, each in the folder it sits in inside the zip — the presets
    /// name no group of their own. Three are masks alone, which Rustroom cannot apply.
    #[test]
    fn a_real_pack_files_each_preset_under_its_folder() {
        let r = read("questtion.zip", &fixture("questtion.zip"));
        assert_eq!((r.not_presets, r.unreadable), (0, 0));
        assert_eq!(
            r.presets.len() + r.carried_nothing.len(),
            60,
            "the pack's 60"
        );
        assert!(!r.presets.is_empty(), "the sample");
        assert!(
            r.presets
                .iter()
                .all(|p| p.group.as_deref() == Some("Presets")),
            "{:?}",
            r.presets.iter().map(|p| &p.group).collect::<Vec<_>>()
        );
        let mut nothing: Vec<&str> = r.carried_nothing.iter().map(|(n, _)| n.as_str()).collect();
        nothing.sort_unstable();
        assert_eq!(
            nothing,
            [
                "Darken Background Mask",
                "Skin Smoothing Mask TX:+100",
                "Teeth Whitening Mask"
            ],
            "measured on the real pack: the three mask-only presets"
        );
        assert!(r.carried_nothing.iter().all(|(_, why)| why == &["masks"]));
    }

    /// A preset that names nothing is named after its file; something else is not a preset.
    #[test]
    fn a_nameless_preset_takes_its_file_name_and_other_files_are_counted() {
        let text = String::from_utf8(fixture("questtion/BW 2 +1.2.xmp")).unwrap();
        let start = text.find("<crs:Name>").unwrap();
        let end = text.find("</crs:Name>").unwrap() + "</crs:Name>".len();
        let nameless = format!("{}{}", &text[..start], &text[end..]);
        let r = read("Mono.xmp", nameless.as_bytes());
        assert_eq!(r.presets[0].name, "Mono");
        assert_eq!(read("notes.txt", b"hello").not_presets, 1);
        assert_eq!(read("broken.xmp", b"<x:xmpmeta").unreadable, 1);
    }

    /// The template reads through the same door.
    #[test]
    fn a_template_reads_through_read() {
        let r = read(
            "minimal.lrtemplate",
            &fixture("yuzukumo/minimal.lrtemplate"),
        );
        assert_eq!(r.presets[0].name, "Minimal Demo");
    }
}
