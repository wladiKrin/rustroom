//! The HDR merge's background thread — the dialog's preview and the merge itself.
//!
//! ⚠ `ingest::worker`'s shape: a `std::thread`, an mpsc channel of events, an `AtomicBool` cancel
//! checked between steps. A **preview** worker decodes the bracket at half size once and then waits
//! for new options, re-merging without re-reading (and re-aligning only when Auto align changed); it
//! ends when it is dropped. A **merge** worker reads the full decodes, starts each alignment from the
//! preview's, and writes the DNG.

use crate::hdr_merge::{progress_line, HdrPlan, HdrSettings};
use edit_model::Edits;
use photo_merge::hdr::{self, Aligned, Decoded, MergeError, Merged};
use photo_merge::homography::Homography;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// The dialog preview's long edge, in pixels.
pub const PREVIEW_LONG: usize = 1200;
/// The DNG's own picture's long edge.
pub const DNG_PREVIEW_LONG: usize = 1024;

/// What the worker reports.
#[derive(Debug)]
pub enum HdrEvent {
    /// The status row's line for the merge.
    Progress(String),
    /// The preview, and the same tinted where deghosting took one frame alone (`None` at None) —
    /// both, so the overlay toggles without a re-merge. `transforms` are the half-size alignment the
    /// merge starts from.
    Preview {
        image: image::RgbImage,
        tinted: Option<image::RgbImage>,
        transforms: Vec<Homography>,
    },
    /// The merged DNG is at `path`; `edits` are what it opens with.
    Done {
        path: PathBuf,
        edits: Edits,
    },
    Failed(String),
    Cancelled,
}

pub struct HdrWorker {
    events: Receiver<HdrEvent>,
    cancel: Arc<AtomicBool>,
    options: Option<Sender<HdrSettings>>,
}

impl Drop for HdrWorker {
    /// A worker nobody holds stops at its next step.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The merged photo developed: its linear ProPhoto through `base` (plus Auto when asked), rendered
/// at `long`, and tinted where an overlay exists. Returns the edits too — what the DNG opens with.
fn develop(
    d: &Decoded,
    m: &Merged,
    s: &HdrSettings,
    base: &Edits,
    long: usize,
) -> (image::RgbImage, Option<image::RgbImage>, Edits) {
    let pp = hdr::to_prophoto(m, d);
    let mut edits = base.clone();
    if s.auto_settings {
        crate::auto_tone_tool::apply(&mut edits, &pp, None);
    }
    let as_shot = d.metas[m.reference].metadata.as_shot.unwrap_or_default();
    let scale = if d.half { 0.5 } else { 1.0 };
    let image = photo_merge::preview::render(&pp, &edits, long, scale, as_shot);
    let tinted = m.overlay.as_ref().map(|o| {
        let mut t = image.clone();
        photo_merge::preview::tint(&mut t, o);
        t
    });
    (image, tinted, edits)
}

impl HdrWorker {
    pub fn events(&self) -> &Receiver<HdrEvent> {
        &self.events
    }

    /// Stop at the next step; a merge cancelled before it writes leaves no file.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// New options for a preview worker — the latest wins.
    pub fn set_options(&self, s: HdrSettings) {
        if let Some(o) = &self.options {
            let _ = o.send(s);
        }
    }

    /// The dialog's preview of `plan`: half-size decodes, merged, then again on each new option.
    pub fn preview(plan: &HdrPlan, settings: HdrSettings, base: Edits) -> Self {
        let (tx, events) = channel();
        let (otx, orx) = channel::<HdrSettings>();
        let cancel = Arc::new(AtomicBool::new(false));
        let (frames, active, c) = (plan.frames.clone(), plan.active, cancel.clone());
        std::thread::spawn(move || {
            let fail = |e: MergeError| match e {
                MergeError::Cancelled => None,
                e => Some(HdrEvent::Failed(e.to_string())),
            };
            let run = || -> Result<(), MergeError> {
                let d = hdr::decode_all(&frames, true, &c, &mut |_| {})?;
                let mut s = settings;
                let mut aligned: Aligned =
                    hdr::align_all(&d, active, &s.options(), None, &c, &mut |_| {})?;
                loop {
                    let m = hdr::merge_aligned(&d, &aligned, s.deghost, &c, &mut |_| {})?;
                    let (image, tinted, _) = develop(&d, &m, &s, &base, PREVIEW_LONG);
                    let transforms = aligned.transforms.clone();
                    if tx
                        .send(HdrEvent::Preview {
                            image,
                            tinted,
                            transforms,
                        })
                        .is_err()
                    {
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
                    if next.auto_align != s.auto_align {
                        aligned =
                            hdr::align_all(&d, active, &next.options(), None, &c, &mut |_| {})?;
                    }
                    s = next;
                }
            };
            if let Err(e) = run() {
                if let Some(ev) = fail(e) {
                    let _ = tx.send(ev);
                }
            }
        });
        Self {
            events,
            cancel,
            options: Some(otx),
        }
    }

    /// Merge `plan` at full size and write its DNG. `init` is the preview's alignment, at half size.
    pub fn merge(
        plan: HdrPlan,
        settings: HdrSettings,
        base: Edits,
        init: Option<Vec<Homography>>,
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
            let progress = |p| {
                let _ = tx.send(HdrEvent::Progress(progress_line(&p, &name)));
            };
            let run = || -> Result<Edits, MergeError> {
                let d = hdr::decode_all(&plan.frames, false, &c, &mut |p| progress(p))?;
                let init: Option<Vec<Homography>> =
                    init.map(|v| v.iter().map(|h| h.rescaled(2.0)).collect());
                let a = hdr::align_all(
                    &d,
                    plan.active,
                    &settings.options(),
                    init.as_deref(),
                    &c,
                    &mut |p| progress(p),
                )?;
                let m = hdr::merge_aligned(&d, &a, settings.deghost, &c, &mut |p| progress(p))?;
                let (picture, _, edits) = develop(&d, &m, &settings, &base, DNG_PREVIEW_LONG);
                if c.load(Ordering::Relaxed) {
                    return Err(MergeError::Cancelled);
                }
                progress(hdr::Progress::Writing);
                hdr::write_dng(&m, &d, &plan.output, &picture)?;
                Ok(edits)
            };
            let ev = match run() {
                Ok(edits) => HdrEvent::Done {
                    path: plan.output.clone(),
                    edits,
                },
                Err(MergeError::Cancelled) => HdrEvent::Cancelled,
                Err(e) => HdrEvent::Failed(e.to_string()),
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
    use crate::hdr_merge::{HdrPlan, HdrSettings};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// ⚠ A ceiling against a hang, never a budget — CLAUDE.md's rule for a test that waits on a job.
    const LONG: Duration = Duration::from_secs(120);

    fn next(w: &HdrWorker, want: impl Fn(&HdrEvent) -> bool) -> HdrEvent {
        let start = Instant::now();
        loop {
            let e = w.events().recv_timeout(LONG).expect("the worker answered");
            if want(&e) {
                return e;
            }
            assert!(start.elapsed() < LONG, "never came");
        }
    }

    /// A bracket of M9.10 DNGs — libraw reads them as raws — at −1 / 0 / +1. ⚠ Constructed. 300 px
    /// wide: M9.10's DNG narrower than one tile reads back scrambled (journal, 2026-10-26).
    fn bracket(tag: &str) -> Vec<PathBuf> {
        let dir = test_dir::path(&format!("hdr-worker-{tag}"));
        let (w, h) = (300, 200);
        [-1.0f32, 0.0, 1.0]
            .iter()
            .enumerate()
            .map(|(i, ev)| {
                let mut img = core_image::ImageBuf::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let (u, v) = (x as f32, y as f32);
                        let l = 0.02
                            * 2f32.powf(7.0 * u / w as f32)
                            * (1.0 + 0.4 * (u * 0.3).sin() * (v * 0.23).sin());
                        img.set_pixel(x, y, [l * 0.9, l, l * 1.1].map(|c| c * 2f32.powf(*ev)));
                    }
                }
                let p = dir.join(format!("F{i}.dng"));
                export::dng::save(&img, &p).unwrap();
                p
            })
            .collect()
    }

    fn plan_for(frames: Vec<PathBuf>) -> HdrPlan {
        let output = frames[0].with_file_name("F0-HDR.dng");
        HdrPlan {
            frames,
            active: None,
            output,
        }
    }

    #[test]
    fn the_worker_merges_a_dng_bracket_and_reports_done() {
        let plan = plan_for(bracket("merge"));
        let out = plan.output.clone();
        let w = HdrWorker::merge(
            plan,
            HdrSettings::default(),
            edit_model::Edits::default(),
            None,
        );
        match next(&w, |e| !matches!(e, HdrEvent::Progress(_))) {
            HdrEvent::Done { path, edits } => {
                assert_eq!(path, out);
                assert!(raw_decode::is_float_dng(&out), "a float DNG");
                assert_ne!(edits.exposure, 0.0, "Auto settings set the exposure");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_cancelled_merge_leaves_no_file() {
        let plan = plan_for(bracket("cancel"));
        let out = plan.output.clone();
        let w = HdrWorker::merge(
            plan,
            HdrSettings::default(),
            edit_model::Edits::default(),
            None,
        );
        w.cancel();
        let e = next(&w, |e| !matches!(e, HdrEvent::Progress(_)));
        assert!(matches!(e, HdrEvent::Cancelled), "{e:?}");
        assert!(!out.exists());
        let dir = out.parent().unwrap();
        let partial = std::fs::read_dir(dir).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".partial")
        });
        assert!(!partial, "a partial file was left");
    }

    /// The dialog's preview arrives, and a change of Deghost brings a new one without re-reading.
    #[test]
    fn the_preview_re_merges_on_new_options() {
        let plan = plan_for(bracket("preview"));
        let w = HdrWorker::preview(&plan, HdrSettings::default(), edit_model::Edits::default());
        let first = next(&w, |e| {
            matches!(e, HdrEvent::Preview { .. } | HdrEvent::Failed(_))
        });
        let HdrEvent::Preview {
            image,
            tinted,
            transforms,
        } = first
        else {
            panic!("{first:?}")
        };
        assert!(image.width() > 0 && tinted.is_none() && transforms.len() == 3);
        w.set_options(HdrSettings {
            deghost: photo_merge::deghost::Deghost::High,
            ..HdrSettings::default()
        });
        let second = next(&w, |e| {
            matches!(e, HdrEvent::Preview { .. } | HdrEvent::Failed(_))
        });
        let HdrEvent::Preview { tinted, .. } = second else {
            panic!("{second:?}")
        };
        assert!(tinted.is_some(), "a deghost level brings the overlay");
    }
}
