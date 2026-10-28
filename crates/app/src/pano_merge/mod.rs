//! Photo → Photo merge → Panorama… (M10.17b): what the app decides around `photo_merge::pano` — which
//! files, the result's name, the remembered options — and the worker that runs it. No egui.
//!
//! The selection's rule is the HDR merge's (`hdr_merge::frames`): one raw per photo, a virtual copy is its
//! original, an Immich photo must be downloaded (its result then goes to `hdr_merge::out_folder`), a merged
//! DNG is refused.

pub mod lama;
pub mod worker;

use crate::hdr_merge::{self, Refusal};
use photo_merge::pano::{PanoOptions, Progress};
use photo_merge::projection::Projection;
use std::path::{Path, PathBuf};

/// The dialog's options, remembered when Merge is pressed (`start_import`'s rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PanoSettings {
    /// Auto select picks the projection; `projection` is then only what the control shows when it is off.
    pub auto_projection: bool,
    pub projection: Projection,
    /// Seed the result's crop with the largest rectangle inside the panorama.
    pub auto_crop: bool,
    /// Fill the empty edges with LaMa (M10.16c1's network).
    pub fill_edges: bool,
    /// The panorama on top of a stack of its sources.
    pub create_stack: bool,
}

impl Default for PanoSettings {
    fn default() -> Self {
        Self {
            auto_projection: true,
            projection: Projection::Cylindrical,
            auto_crop: true,
            fill_edges: false,
            create_stack: false,
        }
    }
}

impl PanoSettings {
    /// Auto crop means something only without Fill edges: a filled panorama has nothing to crop away, so
    /// the dialog greys it.
    pub fn auto_crop_applies(&self) -> bool {
        !self.fill_edges
    }

    pub fn options(&self) -> PanoOptions {
        PanoOptions {
            projection: (!self.auto_projection).then_some(self.projection),
            auto_crop: self.auto_crop,
            fill_edges: self.fill_edges,
        }
    }
}

/// A panorama to stitch.
#[derive(Debug, Clone, PartialEq)]
pub struct PanoPlan {
    /// One raw per photo, in grid order — the merge finds the row's own order.
    pub frames: Vec<PathBuf>,
    /// Where the panorama's DNG goes.
    pub output: PathBuf,
}

impl PanoPlan {
    /// Whether the panorama sits in its first frame's folder — the one place Create stack can show it.
    pub fn beside_frames(&self) -> bool {
        hdr_merge::beside(&self.output, &self.frames)
    }
}

/// The selection — each photo's files, in grid order — as a panorama, or why not.
pub fn plan(
    photos: &[Vec<PathBuf>],
    is_immich: impl Fn(&Path) -> bool,
    is_float: impl Fn(&Path) -> bool,
    taken: impl Fn(&Path) -> bool,
    exists: impl Fn(&Path) -> bool,
) -> Result<PanoPlan, Refusal> {
    let (frames, _) = hdr_merge::frames(photos, None, is_immich, is_float, exists)?;
    let output = hdr_merge::output_path_with(&frames[0], "Pano", taken);
    Ok(PanoPlan { frames, output })
}

/// The status row's line for a running merge of `name`.
pub fn progress_line(p: &Progress, name: &str) -> String {
    let step = match p {
        Progress::Decoding { total, .. } => format!("reading {total} photos"),
        Progress::Matching => "matching".into(),
        Progress::Aligning => "aligning".into(),
        Progress::Stitching => "stitching".into(),
        Progress::Filling => "filling edges".into(),
        Progress::Writing => "writing".into(),
    };
    format!("Merging {name} · {step}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdr_merge::MergeKind;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(format!("/photos/{s}"))
    }

    fn plan_of(photos: &[Vec<PathBuf>]) -> Result<PanoPlan, Refusal> {
        plan(photos, |_| false, |_| false, |_| false, |_| true)
    }

    /// Review Focus 4: a photo met twice is one frame, a RAW+JPEG pair its raw.
    #[test]
    fn a_virtual_copy_is_one_frame_in_a_panorama() {
        let a = vec![p("A.RAF"), p("A.JPG")];
        assert_eq!(plan_of(&[a.clone(), a.clone()]), Err(Refusal::TooFew));
        let ok = plan_of(&[a.clone(), a, vec![p("B.RAF")]]).unwrap();
        assert_eq!(ok.frames, vec![p("A.RAF"), p("B.RAF")]);
        assert_eq!(ok.output, p("A-Pano.dng"));
    }

    #[test]
    fn a_merged_dng_is_refused_with_the_panorama_sentence() {
        let r = plan(
            &[vec![p("A.RAF")], vec![p("A-HDR.dng")]],
            |_| false,
            |f| f.to_string_lossy().contains("HDR"),
            |_| false,
            |_| true,
        );
        assert_eq!(r, Err(Refusal::Merged));
        assert_eq!(
            Refusal::Merged.sentence_for(MergeKind::Panorama),
            "A merged photo cannot be merged again"
        );
        assert_eq!(
            Refusal::NotRaw.sentence_for(MergeKind::Panorama),
            "Panorama merge needs raw photos"
        );
    }

    #[test]
    fn the_hdr_sentences_are_unchanged() {
        assert_eq!(Refusal::NotRaw.sentence(), "HDR merge needs raw photos");
        assert_eq!(
            Refusal::Merged.sentence(),
            "A merged HDR cannot be merged again"
        );
        for r in [
            Refusal::TooFew,
            Refusal::NotRaw,
            Refusal::Immich,
            Refusal::Merged,
            Refusal::Missing,
        ] {
            let s = r.sentence_for(MergeKind::Panorama);
            assert!(!s.is_empty() && !s.ends_with('.'), "{s:?}");
        }
    }

    #[test]
    fn the_pano_name_steps_past_a_taken_one() {
        let taken = [p("A-Pano.dng"), p("A-Pano-2.dng")];
        let r = plan(
            &[vec![p("A.RAF")], vec![p("B.RAF")]],
            |_| false,
            |_| false,
            |q| taken.contains(&q.to_path_buf()),
            |_| true,
        )
        .unwrap();
        assert_eq!(r.output, p("A-Pano-3.dng"));
    }

    #[test]
    fn the_defaults_and_the_options() {
        let s = PanoSettings::default();
        assert!(s.auto_projection && s.auto_crop && !s.fill_edges && !s.create_stack);
        assert_eq!(s.options().projection, None, "Auto select");
        let manual = PanoSettings {
            auto_projection: false,
            projection: Projection::Spherical,
            ..s
        };
        assert_eq!(manual.options().projection, Some(Projection::Spherical));
    }

    #[test]
    fn auto_crop_is_greyed_while_fill_edges_is_on() {
        let s = PanoSettings::default();
        assert!(s.auto_crop_applies());
        assert!(!PanoSettings {
            fill_edges: true,
            ..s
        }
        .auto_crop_applies());
    }

    #[test]
    fn the_progress_line_names_the_file_and_the_step() {
        let n = "DSCF0120-Pano.dng";
        assert_eq!(
            progress_line(&Progress::Decoding { done: 0, total: 5 }, n),
            "Merging DSCF0120-Pano.dng · reading 5 photos"
        );
        assert_eq!(
            progress_line(&Progress::Filling, n),
            "Merging DSCF0120-Pano.dng · filling edges"
        );
    }
}
