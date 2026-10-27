//! The panorama merge's background thread — the dialog's preview and the merge itself (M10.17b).
//!
//! HDR's shape (`hdr_merge::worker`): a `std::thread`, an mpsc channel of events, an `AtomicBool` cancel.
//! A **preview** worker decodes the row at half size and aligns it once, then stitches a small panorama
//! (≤ [`PREVIEW_LONG`]) — again on every new option, from the kept decodes and rig, so a projection or crop
//! change costs about a second. A **merge** worker reads the full decodes, starts from the preview's rig
//! (never matching features at full size), stitches at the 16,000-px cap and writes the DNG.

use crate::pano_merge::{progress_line, PanoPlan, PanoSettings};
use edit_model::Edits;
use photo_merge::fill::Inpainter;
use photo_merge::hdr::{self, Decoded, MergeError};
use photo_merge::pano::{self, Aligned, Progress, Stitched};
use photo_merge::projection::Projection;
use photo_merge::projection::{Canvas, MAX_LONG_EDGE};
use photo_merge::rig::Rig;
use raw_decode::CameraFrame;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// The dialog preview's long edge — small, so a change re-stitches in about a second.
pub const PREVIEW_LONG: usize = 2000;
/// The DNG's own picture's long edge.
pub const DNG_PREVIEW_LONG: usize = 1024;

/// Makes the edge filler on the worker's thread, for the row's reference camera — LaMa in the app.
pub type FillFactory =
    Box<dyn FnOnce(&CameraFrame) -> Result<Box<dyn Inpainter + Send>, String> + Send>;

/// What the worker reports.
#[derive(Debug)]
pub enum PanoEvent {
    /// The status row's line for the merge.
    Progress(String),
    /// The preview — the whole panorama, uncropped — with Auto crop's rectangle to draw on it, the
    /// projection used (Auto's pick when Auto select is on), the lens LensFun did not know, and the rig the
    /// merge starts from.
    Preview {
        image: image::RgbImage,
        crop: Option<edit_model::CropRect>,
        projection: Projection,
        no_profile: Option<String>,
        rig: Box<Rig>,
    },
    /// These frames share nothing with the others (file names).
    Unconnected(Vec<String>),
    /// The projection asked for cannot hold this sweep (Perspective past its limit).
    TooWide,
    /// The DNG is at `path`; `edits` are what it opens with.
    Done {
        path: PathBuf,
        edits: Box<Edits>,
    },
    Failed(String),
    Cancelled,
}

pub struct PanoWorker {
    events: Receiver<PanoEvent>,
    cancel: Arc<AtomicBool>,
    options: Option<Sender<PanoSettings>>,
}

impl Drop for PanoWorker {
    /// A worker nobody holds stops at its next step.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The file names of `frames` at `at`.
fn names(frames: &[PathBuf], at: &[usize]) -> Vec<String> {
    at.iter()
        .filter_map(|&i| frames.get(i))
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect()
}

/// The panorama developed: its linear ProPhoto through `base` with the lens corrections off (they are in
/// the pixels already) and, with `crop`, Auto crop's rectangle — rendered at `long`. Returns the edits too:
/// what the DNG opens with.
fn develop(
    d: &Decoded,
    a: &Aligned,
    st: &Stitched,
    base: &Edits,
    crop: bool,
    long: usize,
) -> (image::RgbImage, Edits) {
    let mut edits = base.clone();
    edits.lens_correction = Some(edit_model::LensCorrection::default());
    if let (true, Some(c)) = (crop, st.crop) {
        edits.geometry.crop = c;
    }
    // The pipeline's spatial radii scale by this picture's size against the full-size merge's.
    let full = if d.half {
        a.rig.rescaled(2.0)
    } else {
        a.rig.clone()
    };
    let full_long =
        Canvas::plan(&full, st.projection, MAX_LONG_EDGE).map_or(1, |c| c.width.max(c.height));
    let scale = st.image.width().max(st.image.height()) as f32 / full_long.max(1) as f32;
    let as_shot = d.metas[st.reference].metadata.as_shot.unwrap_or_default();
    let image =
        photo_merge::preview::render(&pano::to_prophoto(st, d), &edits, long, scale, as_shot);
    (image, edits)
}

/// The edge filler, made on first need (LaMa loads in ~7 s) and kept for the preview's re-stitches.
struct Filler {
    factory: Option<FillFactory>,
    net: Option<Box<dyn Inpainter + Send>>,
}

impl Filler {
    fn new(factory: Option<FillFactory>) -> Self {
        Self { factory, net: None }
    }

    /// The filler when Fill edges is on and one can be made; a failure to make it is said once.
    fn get(
        &mut self,
        want: bool,
        d: &Decoded,
        a: &Aligned,
        tx: &Sender<PanoEvent>,
    ) -> Option<&mut dyn Inpainter> {
        if !want {
            return None;
        }
        if self.net.is_none() {
            if let Some(f) = self.factory.take() {
                match f(&d.metas[a.rig.reference]) {
                    Ok(n) => self.net = Some(n),
                    Err(e) => {
                        let _ = tx.send(PanoEvent::Failed(e));
                    }
                }
            }
        }
        self.net.as_deref_mut().map(|n| n as &mut dyn Inpainter)
    }
}

impl PanoWorker {
    pub fn events(&self) -> &Receiver<PanoEvent> {
        &self.events
    }

    /// Stop at the next step; a merge cancelled before it writes leaves no file.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// A worker whose thread is gone without a last word — what a panic leaves. For the queue's tests.
    #[doc(hidden)]
    pub fn ended() -> Self {
        let (_, events) = channel();
        Self {
            events,
            cancel: Arc::new(AtomicBool::new(false)),
            options: None,
        }
    }

    /// New options for a preview worker — the latest wins.
    pub fn set_options(&self, s: PanoSettings) {
        if let Some(o) = &self.options {
            let _ = o.send(s);
        }
    }

    /// The dialog's preview of `plan`.
    pub fn preview(
        plan: &PanoPlan,
        settings: PanoSettings,
        base: Edits,
        fill: Option<FillFactory>,
    ) -> Self {
        let (tx, events) = channel();
        let (otx, orx) = channel::<PanoSettings>();
        let cancel = Arc::new(AtomicBool::new(false));
        let (frames, c) = (plan.frames.clone(), cancel.clone());
        std::thread::spawn(move || {
            let run = || -> Result<(), MergeError> {
                let d = hdr::decode_all(&frames, true, &c, &mut |_| {})?;
                let a = match pano::align(&d, None, &c, &mut |_| {}) {
                    Err(MergeError::Unconnected(v)) => {
                        let _ = tx.send(PanoEvent::Unconnected(names(&frames, &v)));
                        return Ok(());
                    }
                    other => other?,
                };
                let mut s = settings;
                let mut filler = Filler::new(fill);
                loop {
                    let net = filler.get(s.fill_edges, &d, &a, &tx);
                    let ev = match pano::stitch(
                        &d,
                        &a,
                        &s.options(),
                        PREVIEW_LONG,
                        net,
                        &c,
                        &mut |_| {},
                    ) {
                        Ok(st) => PanoEvent::Preview {
                            image: develop(&d, &a, &st, &base, false, PREVIEW_LONG).0,
                            crop: st.crop,
                            projection: st.projection,
                            no_profile: a.no_profile.clone(),
                            rig: Box::new(a.align_rig.clone()),
                        },
                        Err(MergeError::TooWide) => PanoEvent::TooWide,
                        Err(e) => return Err(e),
                    };
                    if tx.send(ev).is_err() {
                        return Ok(());
                    }
                    let Ok(mut next) = orx.recv() else {
                        return Ok(());
                    };
                    while let Ok(n) = orx.try_recv() {
                        next = n;
                    }
                    if c.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    s = next;
                }
            };
            match run() {
                Ok(()) | Err(MergeError::Cancelled) => {}
                Err(e) => {
                    let _ = tx.send(PanoEvent::Failed(e.to_string()));
                }
            }
        });
        Self {
            events,
            cancel,
            options: Some(otx),
        }
    }

    /// Stitch `plan` at full size and write its DNG; `rig` is the preview's.
    pub fn merge(
        plan: PanoPlan,
        settings: PanoSettings,
        base: Edits,
        rig: Option<Rig>,
        fill: Option<FillFactory>,
    ) -> Self {
        let (tx, events) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            let name = plan
                .output
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let progress = |p: Progress| {
                let _ = tx.send(PanoEvent::Progress(progress_line(&p, &name)));
            };
            let run = || -> Result<Edits, MergeError> {
                let d = hdr::decode_all(&plan.frames, false, &c, &mut |p| {
                    if let hdr::Progress::Decoding { done, total } = p {
                        progress(Progress::Decoding { done, total });
                    }
                })?;
                let a = pano::align(&d, rig.as_ref(), &c, &mut |p| progress(p))?;
                let mut net = match fill.filter(|_| settings.fill_edges) {
                    Some(f) => Some(f(&d.metas[a.rig.reference]).map_err(MergeError::Fill)?),
                    None => None,
                };
                let filler = net.as_deref_mut().map(|n| n as &mut dyn Inpainter);
                let st = pano::stitch(
                    &d,
                    &a,
                    &settings.options(),
                    MAX_LONG_EDGE,
                    filler,
                    &c,
                    &mut |p| progress(p),
                )?;
                let (picture, edits) = develop(&d, &a, &st, &base, true, DNG_PREVIEW_LONG);
                if c.load(Ordering::Relaxed) {
                    return Err(MergeError::Cancelled);
                }
                progress(Progress::Writing);
                pano::write_dng(&st, &d, &plan.output, &picture)?;
                Ok(edits)
            };
            let ev = match run() {
                Ok(edits) => PanoEvent::Done {
                    path: plan.output.clone(),
                    edits: Box::new(edits),
                },
                Err(MergeError::Cancelled) => PanoEvent::Cancelled,
                Err(MergeError::Unconnected(v)) => PanoEvent::Failed(format!(
                    "{} shares nothing with the others",
                    names(&plan.frames, &v).join(", ")
                )),
                Err(e) => PanoEvent::Failed(e.to_string()),
            };
            let _ = tx.send(ev);
        });
        Self {
            events,
            cancel,
            options: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use photo_merge::synth_pano;
    use std::time::{Duration, Instant};

    /// ⚠ A ceiling against a hang, never a budget — CLAUDE.md's rule for a test that waits on a job.
    const LONG: Duration = Duration::from_secs(120);

    fn next(w: &PanoWorker, want: impl Fn(&PanoEvent) -> bool) -> PanoEvent {
        let start = Instant::now();
        loop {
            let e = w.events().recv_timeout(LONG).expect("the worker answered");
            if want(&e) {
                return e;
            }
            assert!(start.elapsed() < LONG, "never came");
        }
    }

    fn is_preview(e: &PanoEvent) -> bool {
        matches!(
            e,
            PanoEvent::Preview { .. }
                | PanoEvent::Failed(_)
                | PanoEvent::Unconnected(_)
                | PanoEvent::TooWide
        )
    }

    /// A constructed row of M9.10 DNGs (`synth_pano`): four frames, a third shared.
    fn row(tag: &str, scenes: &[u64]) -> PanoPlan {
        let mut r = synth_pano::row(scenes.len(), 0.35, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        let dir = test_dir::path(&format!("pano-worker-{tag}"));
        let frames = synth_pano::write_dng_row(&dir, &r, scenes);
        let output = dir.join("frame0-Pano.dng");
        PanoPlan { frames, output }
    }

    #[test]
    fn the_worker_previews_then_restitches_within_a_second() {
        let w = PanoWorker::preview(
            &row("restitch", &[1, 1, 1, 1]),
            PanoSettings::default(),
            Edits::default(),
            None,
        );
        let first = next(&w, is_preview);
        let PanoEvent::Preview {
            image,
            crop,
            projection,
            rig,
            ..
        } = first
        else {
            panic!("{first:?}")
        };
        assert!(
            image.width() > image.height() * 2,
            "{} × {}",
            image.width(),
            image.height()
        );
        assert!(crop.is_some(), "Auto crop is on");
        assert_eq!(rig.order.len(), 4);
        let other = if projection == Projection::Spherical {
            Projection::Cylindrical
        } else {
            Projection::Spherical
        };
        let t = Instant::now();
        w.set_options(PanoSettings {
            auto_projection: false,
            projection: other,
            ..PanoSettings::default()
        });
        let second = next(&w, is_preview);
        let took = t.elapsed();
        let PanoEvent::Preview { projection, .. } = second else {
            panic!("{second:?}")
        };
        assert_eq!(projection, other);
        eprintln!("re-stitch {took:?}");
        // ⚠ The spec's bar is about a second in release; a debug build is several times slower.
        assert!(
            took < Duration::from_secs(if cfg!(debug_assertions) { 15 } else { 1 }),
            "{took:?}"
        );
    }

    #[test]
    fn an_unconnected_frame_is_named() {
        let w = PanoWorker::preview(
            &row("unconnected", &[1, 1, 1, 2]),
            PanoSettings::default(),
            Edits::default(),
            None,
        );
        let e = next(&w, is_preview);
        assert!(
            matches!(&e, PanoEvent::Unconnected(v) if v == &vec!["frame3.dng".to_string()]),
            "{e:?}"
        );
    }

    #[test]
    fn perspective_too_wide_says_so_and_waits() {
        let w = PanoWorker::preview(
            &row("toowide", &[1, 1, 1, 1]),
            PanoSettings {
                auto_projection: false,
                projection: Projection::Perspective,
                ..PanoSettings::default()
            },
            Edits::default(),
            None,
        );
        let e = next(&w, is_preview);
        assert!(matches!(e, PanoEvent::TooWide), "{e:?}");
        w.set_options(PanoSettings::default());
        let e = next(&w, is_preview);
        assert!(matches!(e, PanoEvent::Preview { .. }), "{e:?}");
    }

    #[test]
    fn the_merge_seeds_the_crop_and_lens_off() {
        let plan = row("merge", &[1, 1, 1]);
        let out = plan.output.clone();
        let w = PanoWorker::merge(plan, PanoSettings::default(), Edits::default(), None, None);
        match next(&w, |e| !matches!(e, PanoEvent::Progress(_))) {
            PanoEvent::Done { path, edits } => {
                assert_eq!(path, out);
                assert!(raw_decode::is_float_dng(&out), "a float DNG");
                assert_ne!(
                    edits.geometry.crop,
                    edit_model::CropRect::default(),
                    "Auto crop's rectangle"
                );
                assert_eq!(
                    edits.lens_correction,
                    Some(edit_model::LensCorrection::default()),
                    "lens off"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    /// Review Focus 3, the lib's half: Fill edges with no filler (LaMa not downloaded) still stitches, cropped.
    #[test]
    fn fill_edges_without_a_filler_falls_back_to_the_crop() {
        let w = PanoWorker::preview(
            &row("nofill", &[1, 1, 1]),
            PanoSettings {
                fill_edges: true,
                ..PanoSettings::default()
            },
            Edits::default(),
            None,
        );
        let e = next(&w, is_preview);
        assert!(
            matches!(e, PanoEvent::Preview { crop: Some(_), .. }),
            "{e:?}"
        );
    }

    /// With a filler, the edges are filled and nothing is cropped.
    #[test]
    fn fill_edges_with_a_filler_fills_and_does_not_crop() {
        struct Mean;
        impl Inpainter for Mean {
            fn side(&self) -> usize {
                64
            }
            fn fill(
                &mut self,
                window: &core_image::ImageBuf,
                _: &[f32],
            ) -> Result<core_image::ImageBuf, String> {
                Ok(window.clone())
            }
        }
        let used = Arc::new(AtomicBool::new(false));
        let flag = used.clone();
        let factory: FillFactory = Box::new(move |_| {
            flag.store(true, Ordering::Relaxed);
            Ok(Box::new(Mean))
        });
        let w = PanoWorker::preview(
            &row("fill", &[1, 1, 1]),
            PanoSettings {
                fill_edges: true,
                ..PanoSettings::default()
            },
            Edits::default(),
            Some(factory),
        );
        let e = next(&w, is_preview);
        assert!(matches!(e, PanoEvent::Preview { crop: None, .. }), "{e:?}");
        assert!(used.load(Ordering::Relaxed), "the filler was made");
    }

    #[test]
    fn a_cancelled_merge_leaves_no_file() {
        let plan = row("cancel", &[1, 1]);
        let out = plan.output.clone();
        let w = PanoWorker::merge(plan, PanoSettings::default(), Edits::default(), None, None);
        w.cancel();
        let e = next(&w, |e| !matches!(e, PanoEvent::Progress(_)));
        assert!(matches!(e, PanoEvent::Cancelled), "{e:?}");
        assert!(!out.exists());
    }
}
