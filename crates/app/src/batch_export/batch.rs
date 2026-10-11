//! What an export run reads from disk **once**, before any photo: the creative looks its photos
//! name and the image its watermark draws (M10.10a).
//!
//! ⚠⚠ **Read on the worker thread, never in `spawn`**, which runs on the UI thread that starts
//! the export — a 64³ cube is 786 KB and a logo can be a large PNG. And read **per run**, not per
//! photo: one batch shares one watermark, and a look is loaded once however many photos name it.

use super::*;
use export::Logo;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where a run finds the files its photos and its options only *name*.
///
/// ⚠ `None` for either means there is no such place, and a run then behaves as though every file
/// it names were missing — a look is left out, an image watermark is reported unreadable.
#[derive(Debug, Clone, Default)]
pub struct ExportDirs {
    /// The creative looks (`.cube` files).
    pub looks: Option<PathBuf>,
    /// The kept copies an image watermark draws ([`crate::watermark_store`]).
    pub watermarks: Option<PathBuf>,
}

/// What a run read once.
pub(super) struct Batch {
    pub(super) looks: std::collections::HashMap<String, Arc<pipeline::lut::Cube>>,
    /// The image watermark, decoded: `Ok(None)` when the options draw no image, `Err` the
    /// report's note when they do and its copy could not be read.
    pub(super) logo: Result<Option<Logo>, String>,
}

impl Batch {
    pub(super) fn read(jobs: &[ExportJob], opts: &ExportOptions, dirs: ExportDirs) -> Self {
        Self {
            looks: resolve_looks(jobs, dirs.looks),
            logo: resolve_logo(opts, dirs.watermarks.as_deref()),
        }
    }

    /// The logo every photo of the run draws, if there is one to draw.
    pub(super) fn logo(&self) -> Option<&Logo> {
        self.logo.as_ref().ok().and_then(Option::as_ref)
    }
}

/// Load every distinct `.cube` the jobs name, once.
///
/// ⚠⚠ **A map rather than one slot, because a selection is not one photograph.** `RenderEngine`'s
/// worker caches a single look and is right to — it renders one photo at a time. An export runs a
/// *selection*, whose jobs each carry their own `Edits`, so a one-slot cache would hand some
/// photographs another's look. The load is still paid once per distinct name.
///
/// ⚠ A name that will not load is simply absent from the map, and the photograph then exports
/// **without the look and with its rendering intact** — `build_pipeline_split_in_region_with_brush`
/// documents that as the rule for a `.cube` this machine does not have. Never an error that fails
/// the photo.
pub(super) fn resolve_looks(
    jobs: &[ExportJob],
    dir: Option<std::path::PathBuf>,
) -> std::collections::HashMap<String, Arc<pipeline::lut::Cube>> {
    let mut out = std::collections::HashMap::new();
    let Some(store) = dir.map(crate::looks::LookStore::new) else {
        return out;
    };
    for j in jobs {
        let Some(f) = j.edits.look.file.as_deref() else {
            continue;
        };
        if out.contains_key(f) {
            continue;
        }
        if let Ok(cube) = store.load(f) {
            out.insert(f.to_string(), Arc::new(cube));
        }
    }
    out
}

/// Read and decode the image `opts`' watermark draws, from the store at `dir`.
///
/// ⚠⚠ **Decoded into the space the photo is RENDERED in** — `fixed_space` before the dialog's
/// choice, the rule `export_one` follows — because the logo lands on the rendered, encoded buffer;
/// a DNG is rendered in ProPhoto whatever the dialog says.
///
/// ⚠ Any miss — no store, a name that is not one plain file name, a copy gone, a file that will
/// not decode — is one note naming the image. The run still exports every photo: a missing logo
/// is worth saying, not worth failing a batch over.
pub(super) fn resolve_logo(
    opts: &ExportOptions,
    dir: Option<&Path>,
) -> Result<Option<Logo>, String> {
    let mark = &opts.watermark;
    if mark.kind != export::WatermarkKind::Image || !mark.is_active() {
        return Ok(None);
    }
    let name = mark.image.trim();
    let space = opts.format.fixed_space().unwrap_or(opts.space);
    dir.map(|d| crate::watermark_store::WatermarkStore::new(d.to_path_buf()))
        .and_then(|store| store.path_of(name))
        .and_then(|at| std::fs::read(at).ok())
        .and_then(|bytes| Logo::decode(&bytes, space).ok())
        .map(Some)
        .ok_or_else(|| format!("the watermark image {name} could not be read"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_export::fixtures::*;
    use export::{Logo, Watermark, WatermarkKind};

    fn image_mark(name: &str) -> ExportOptions {
        ExportOptions {
            watermark: Watermark {
                kind: WatermarkKind::Image,
                image: name.into(),
                ..Watermark::default()
            },
            ..ExportOptions::default()
        }
    }

    /// Write `n` `.cube` files into a fresh directory and return it.
    fn looks_dir(tag: &str, names: &[&str]) -> TempDir {
        let d = TempDir::new(tag);
        for n in names {
            std::fs::write(
                d.0.join(n),
                "LUT_3D_SIZE 2\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n1 1 1\n",
            )
            .unwrap();
        }
        d
    }

    fn job_wanting(dir: &TempDir, name: &str, look: Option<&str>) -> ExportJob {
        let mut j = jobs(dir, &[name]).pop().unwrap();
        j.edits.look.file = look.map(|s| s.to_string());
        j
    }

    /// ⚠⚠ **Each job's OWN look, and a one-slot cache would get this wrong.** `RenderEngine`'s
    /// worker caches a single look and is right to — it renders one photograph at a time. An
    /// export runs a *selection*, whose jobs each carry their own `Edits`, so a batch-wide look
    /// would render some photographs with another's.
    #[test]
    fn every_distinct_look_a_selection_names_is_loaded() {
        let looks = looks_dir("looks_many", &["a.cube", "b.cube"]);
        let files = TempDir::new("looks_many_src");
        let js = vec![
            job_wanting(&files, "p0.RAF", Some("a.cube")),
            job_wanting(&files, "p1.RAF", Some("b.cube")),
            // ⚠ A repeat: it must not be loaded twice, and it must still be present.
            job_wanting(&files, "p2.RAF", Some("a.cube")),
            job_wanting(&files, "p3.RAF", None),
        ];
        let got = resolve_looks(&js, Some(looks.0.clone()));
        let mut names: Vec<&str> = got.keys().map(|k| k.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a.cube", "b.cube"]);
    }

    /// ⚠⚠ **A `.cube` this machine does not have exports WITHOUT THE LOOK, never without the
    /// rendering and never as an error.** That is
    /// `build_pipeline_split_in_region_with_brush`'s documented rule — *"a preset naming a file
    /// this machine does not have must render without the look, not without a rendering"* — and
    /// the export path has to agree with it.
    ///
    /// ⚠ Sabotage: make `resolve_looks` insert a default cube on a load failure, or propagate the
    /// error. Either turns a missing file into a wrong photograph or a failed export.
    #[test]
    fn a_look_this_machine_does_not_have_is_simply_absent() {
        let looks = looks_dir("looks_missing", &["present.cube"]);
        let files = TempDir::new("looks_missing_src");
        let js = vec![
            job_wanting(&files, "p0.RAF", Some("present.cube")),
            job_wanting(&files, "p1.RAF", Some("gone.cube")),
        ];
        let got = resolve_looks(&js, Some(looks.0.clone()));
        assert!(
            got.contains_key("present.cube"),
            "the one that exists loads"
        );
        assert!(
            !got.contains_key("gone.cube"),
            "the one that does not must be ABSENT — the photo then renders without the look"
        );
    }

    /// ⚠ No looks directory at all is the same answer as an empty one: every photo exports as
    /// though its `.cube` were missing.
    #[test]
    fn no_looks_directory_resolves_nothing() {
        let files = TempDir::new("looks_nodir");
        let js = vec![job_wanting(&files, "p0.RAF", Some("a.cube"))];
        assert!(resolve_looks(&js, None).is_empty());
    }

    #[test]
    fn a_text_mark_reads_no_image() {
        let mut o = ExportOptions::default();
        o.watermark.text = "© me".into();
        assert!(matches!(resolve_logo(&o, None), Ok(None)));
        // ⚠ Nor is an image mark with no file chosen a miss: it draws nothing and says nothing.
        assert!(matches!(resolve_logo(&image_mark(""), None), Ok(None)));
    }

    #[test]
    fn a_copy_that_cannot_be_read_is_a_note() {
        let d = TempDir::new("wm_notes");
        let store = d.0.join("store");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("bad.png"), b"junk").unwrap();
        // ⚠⚠ A real image just OUTSIDE the store, so a name that reached it would succeed — the
        // guard is the only thing that can make `../outside.png` fail here.
        write_png(&d.0.join("outside.png"), 4, 4, [255, 0, 0, 255]);
        for name in ["gone.png", "bad.png", "../outside.png"] {
            assert_eq!(
                resolve_logo(&image_mark(name), Some(&store)).err(),
                Some(format!("the watermark image {name} could not be read")),
                "{name}"
            );
        }
        assert!(
            resolve_logo(&image_mark("gone.png"), None).is_err(),
            "no store at all"
        );
        // The positive control: a good copy in the store resolves.
        write_png(&store.join("good.png"), 4, 4, [255, 0, 0, 255]);
        assert!(matches!(
            resolve_logo(&image_mark("good.png"), Some(&store)),
            Ok(Some(_))
        ));
    }

    #[test]
    fn a_dng_export_converts_the_logo_into_prophoto() {
        let d = TempDir::new("wm_dng");
        write_png(&d.0.join("red.png"), 1, 1, [255, 0, 0, 255]);
        let bytes = std::fs::read(d.0.join("red.png")).unwrap();
        let mut o = image_mark("red.png");
        o.format = export::ExportFormat::Dng;
        o.space = color::OutputSpace::Srgb;
        let got = resolve_logo(&o, Some(&d.0))
            .expect("it reads")
            .expect("an image mark")
            .pixel(0, 0);
        let srgb = Logo::decode(&bytes, color::OutputSpace::Srgb)
            .unwrap()
            .pixel(0, 0);
        let pro = Logo::decode(&bytes, color::OutputSpace::ProPhoto)
            .unwrap()
            .pixel(0, 0);
        assert_ne!(
            srgb, pro,
            "the two spaces must differ for this to mean anything"
        );
        assert_eq!(
            got, pro,
            "a DNG is rendered in ProPhoto whatever the dialog says"
        );
    }
}
