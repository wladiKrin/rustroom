//! Photo → Photo merge → HDR… (M10.17a): what the app decides around `photo_merge` — which files,
//! the result's name, the remembered options — and the worker that runs it. No egui.

pub mod worker;

use photo_merge::deghost::Deghost;
use photo_merge::hdr::{HdrOptions, Progress};
use std::path::{Path, PathBuf};

/// The dialog's options, remembered when Merge is pressed (`start_import`'s rule). Lightroom's
/// defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HdrSettings {
    pub auto_align: bool,
    /// Rustroom's Auto (Exposure, Whites, Blacks) on the merged photo.
    pub auto_settings: bool,
    pub deghost: Deghost,
    /// Red over what deghosting took from one frame alone, in the preview.
    pub overlay: bool,
    /// The merged photo on top of a stack of its sources.
    pub create_stack: bool,
}

impl Default for HdrSettings {
    fn default() -> Self {
        Self {
            auto_align: true,
            auto_settings: true,
            deghost: Deghost::None,
            overlay: false,
            create_stack: false,
        }
    }
}

impl HdrSettings {
    pub fn options(&self) -> HdrOptions {
        HdrOptions {
            auto_align: self.auto_align,
            deghost: self.deghost,
        }
    }
}

/// Why a selection cannot be merged — said in the status row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TooFew,
    NotRaw,
    Immich,
    Merged,
    Missing,
}

/// Which merge refuses — the sentence names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeKind {
    Hdr,
    Panorama,
}

impl Refusal {
    /// The HDR merge's sentence.
    pub fn sentence(&self) -> &'static str {
        self.sentence_for(MergeKind::Hdr)
    }

    pub fn sentence_for(&self, kind: MergeKind) -> &'static str {
        match (self, kind) {
            (Refusal::TooFew, _) => "Select two or more raw photos to merge",
            (Refusal::NotRaw, MergeKind::Hdr) => "HDR merge needs raw photos",
            (Refusal::NotRaw, MergeKind::Panorama) => "Panorama merge needs raw photos",
            (Refusal::Immich, _) => "Download Immich photos to a folder before merging them",
            (Refusal::Merged, MergeKind::Hdr) => "A merged HDR cannot be merged again",
            (Refusal::Merged, MergeKind::Panorama) => "A merged photo cannot be merged again",
            (Refusal::Missing, _) => "A photo's file is missing",
        }
    }
}

/// A merge to run.
#[derive(Debug, Clone, PartialEq)]
pub struct HdrPlan {
    /// One raw per photo, in grid order.
    pub frames: Vec<PathBuf>,
    /// The active photo's place in `frames` — it wins a tie for the reference.
    pub active: Option<usize>,
    /// Where the merged DNG goes.
    pub output: PathBuf,
}

/// The selection — each photo's files, in grid order — as a merge, or why not. A photo contributes
/// its raw (a RAW+JPEG pair its RAW); a photo met twice (a virtual copy and its original) is one
/// frame. The checks are handed in, so the rule is tested without a disk or a catalog.
pub fn plan(
    photos: &[Vec<PathBuf>],
    active: Option<usize>,
    is_immich: impl Fn(&Path) -> bool,
    is_float: impl Fn(&Path) -> bool,
    taken: impl Fn(&Path) -> bool,
    exists: impl Fn(&Path) -> bool,
) -> Result<HdrPlan, Refusal> {
    let (frames, active) = frames(photos, active, is_immich, is_float, exists)?;
    let output = output_path(&frames[0], taken);
    Ok(HdrPlan {
        frames,
        active,
        output,
    })
}

/// The selection's frames — every merge's rule (M10.17a; M10.17b shares it) — and the active photo's
/// place among them.
pub fn frames(
    photos: &[Vec<PathBuf>],
    active: Option<usize>,
    is_immich: impl Fn(&Path) -> bool,
    is_float: impl Fn(&Path) -> bool,
    exists: impl Fn(&Path) -> bool,
) -> Result<(Vec<PathBuf>, Option<usize>), Refusal> {
    let mut frames: Vec<PathBuf> = Vec::new();
    let mut active_frame = None;
    for (i, files) in photos.iter().enumerate() {
        let raw = files
            .iter()
            .find(|f| catalog::is_raw(f))
            .ok_or(Refusal::NotRaw)?;
        if !exists(raw) {
            return Err(Refusal::Missing);
        }
        if is_immich(raw) {
            return Err(Refusal::Immich);
        }
        if is_float(raw) {
            return Err(Refusal::Merged);
        }
        let at = match frames.iter().position(|f| f == raw) {
            Some(at) => at,
            None => {
                frames.push(raw.clone());
                frames.len() - 1
            }
        };
        if active == Some(i) {
            active_frame = Some(at);
        }
    }
    if frames.len() < 2 {
        return Err(Refusal::TooFew);
    }
    Ok((frames, active_frame))
}

/// `<stem>-HDR.dng` beside `first` — Lightroom's name — then `-HDR-2`, `-HDR-3`… past any taken.
pub fn output_path(first: &Path, taken: impl Fn(&Path) -> bool) -> PathBuf {
    output_path_with(first, "HDR", taken)
}

/// `<stem>-<suffix>.dng` beside `first`, then `-<suffix>-2`… past any taken.
pub fn output_path_with(first: &Path, suffix: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = first.parent().unwrap_or(Path::new(""));
    (1..)
        .map(|n| {
            let name = if n == 1 {
                format!("{stem}-{suffix}.dng")
            } else {
                format!("{stem}-{suffix}-{n}.dng")
            };
            dir.join(name)
        })
        .find(|p| !taken(p))
        .expect("an unbounded search")
}

/// The status row's line for a running merge of `name`.
pub fn progress_line(p: &Progress, name: &str) -> String {
    let step = match p {
        Progress::Decoding { total, .. } => format!("reading {total} photos"),
        Progress::Aligning => "aligning".into(),
        Progress::Merging => "merging".into(),
        Progress::Writing => "writing".into(),
    };
    format!("Merging {name} · {step}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(format!("/photos/{s}"))
    }

    fn plan_of(photos: &[Vec<PathBuf>]) -> Result<HdrPlan, Refusal> {
        plan(photos, None, |_| false, |_| false, |_| false, |_| true)
    }

    #[test]
    fn the_name_steps_past_a_taken_one() {
        let first = p("DSCF7618.RAF");
        assert_eq!(output_path(&first, |_| false), p("DSCF7618-HDR.dng"));
        let taken = [p("DSCF7618-HDR.dng"), p("DSCF7618-HDR-2.dng")];
        assert_eq!(
            output_path(&first, |q| taken.contains(&q.to_path_buf())),
            p("DSCF7618-HDR-3.dng")
        );
    }

    /// A photo and its virtual copy are one file — one frame.
    #[test]
    fn a_virtual_copy_and_its_original_are_one_frame() {
        let a = vec![p("A.RAF")];
        assert_eq!(plan_of(&[a.clone(), a.clone()]), Err(Refusal::TooFew));
        let ok = plan_of(&[a.clone(), a, vec![p("B.RAF")]]).unwrap();
        assert_eq!(ok.frames, vec![p("A.RAF"), p("B.RAF")]);
        assert_eq!(ok.output, p("A-HDR.dng"));
    }

    #[test]
    fn a_raw_and_jpeg_pair_contributes_its_raw() {
        let ok = plan_of(&[vec![p("A.RAF"), p("A.JPG")], vec![p("B.JPG"), p("B.RAF")]]).unwrap();
        assert_eq!(ok.frames, vec![p("A.RAF"), p("B.RAF")]);
    }

    #[test]
    fn a_photo_without_a_raw_is_refused() {
        assert_eq!(
            plan_of(&[vec![p("A.RAF")], vec![p("B.JPG")]]),
            Err(Refusal::NotRaw)
        );
    }

    #[test]
    fn a_merged_hdr_is_refused() {
        let photos = [vec![p("A.RAF")], vec![p("A-HDR.dng")]];
        let r = plan(
            &photos,
            None,
            |_| false,
            |f| f.to_string_lossy().contains("HDR"),
            |_| false,
            |_| true,
        );
        assert_eq!(r, Err(Refusal::Merged));
    }

    #[test]
    fn one_photo_is_too_few() {
        assert_eq!(plan_of(&[vec![p("A.RAF")]]), Err(Refusal::TooFew));
        assert_eq!(plan_of(&[]), Err(Refusal::TooFew));
    }

    #[test]
    fn an_immich_photo_is_refused() {
        let r = plan(
            &[vec![p("A.RAF")], vec![p("B.RAF")]],
            None,
            |f| f.ends_with("B.RAF"),
            |_| false,
            |_| false,
            |_| true,
        );
        assert_eq!(r, Err(Refusal::Immich));
    }

    #[test]
    fn a_missing_file_is_refused() {
        let r = plan(
            &[vec![p("A.RAF")], vec![p("B.RAF")]],
            None,
            |_| false,
            |_| false,
            |_| false,
            |f: &Path| !f.ends_with("B.RAF"),
        );
        assert_eq!(r, Err(Refusal::Missing));
    }

    /// The active photo's place survives the de-duplication.
    #[test]
    fn the_active_photo_is_carried_into_the_plan() {
        let photos = [
            vec![p("A.RAF")],
            vec![p("A.RAF")],
            vec![p("B.RAF")],
            vec![p("C.RAF")],
        ];
        let ok = plan(&photos, Some(3), |_| false, |_| false, |_| false, |_| true).unwrap();
        assert_eq!(ok.active, Some(2));
    }

    #[test]
    fn every_refusal_is_one_short_sentence() {
        for r in [
            Refusal::TooFew,
            Refusal::NotRaw,
            Refusal::Immich,
            Refusal::Merged,
            Refusal::Missing,
        ] {
            let s = r.sentence();
            assert!(!s.is_empty() && !s.ends_with('.'), "{s:?}");
        }
    }

    #[test]
    fn the_defaults_are_lightrooms() {
        let s = HdrSettings::default();
        assert!(s.auto_align && s.auto_settings && !s.overlay && !s.create_stack);
        assert_eq!(s.deghost, photo_merge::deghost::Deghost::None);
    }

    #[test]
    fn the_progress_line_names_the_file_and_the_step() {
        use photo_merge::hdr::Progress;
        let n = "DSCF7618-HDR.dng";
        assert_eq!(
            progress_line(&Progress::Decoding { done: 0, total: 3 }, n),
            "Merging DSCF7618-HDR.dng · reading 3 photos"
        );
        assert_eq!(
            progress_line(&Progress::Aligning, n),
            "Merging DSCF7618-HDR.dng · aligning"
        );
        assert_eq!(
            progress_line(&Progress::Writing, n),
            "Merging DSCF7618-HDR.dng · writing"
        );
    }
}
