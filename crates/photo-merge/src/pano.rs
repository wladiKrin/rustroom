//! The panorama merge end to end (M10.17b): decode → each frame's lens → features → every pair → the chain
//! → the global fit → levelled → refined densely → re-fitted → the exposures → projected → seams → blended
//! → Fill edges or Auto crop → the floating-point DNG. HDR's `hdr.rs` is the model; its decode, error type
//! and DNG writer are shared.
//!
//! **Two sizes.** Alignment runs at the ALIGN size — the frames as decoded when their long edge is at most
//! [`ALIGN_LONG`], else half — so the dialog's preview (half decodes) and the merge (full ones) align at the
//! same size, and the merge starts from the preview's rig unchanged ([`align`]'s `init`): it never matches
//! features at full size. The stitch runs at the decode size.

use crate::crop::{self, Valid};
use crate::features::{self, Keypoint, FEATURE_LONG_EDGE};
use crate::fill::{self, Inpainter};
use crate::frame::Frame;
use crate::hdr::{Decoded, MergeError};
use crate::lens::{self, FrameLens};
use crate::projection::{self, Canvas, Projection};
use crate::pyramid::LumPyramid;
use crate::rig::{self, Rig};
use crate::rotation::{self, Corr};
use crate::seam::{self, Labels};
use crate::{blend, gain, pano_warp, refine};
use core_image::ImageBuf;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Frames up to this long edge align as decoded; longer ones at half.
pub const ALIGN_LONG: usize = 3200;
/// Two frames' focal lengths further apart than this (mm) are two focal lengths.
const FOCAL_TOLERANCE_MM: f32 = 0.5;

/// What the dialog decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanoOptions {
    /// `None` = Auto select.
    pub projection: Option<Projection>,
    pub auto_crop: bool,
    pub fill_edges: bool,
}

impl Default for PanoOptions {
    fn default() -> Self {
        Self {
            projection: None,
            auto_crop: true,
            fill_edges: false,
        }
    }
}

/// Where a merge is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    Decoding { done: usize, total: usize },
    Matching,
    Aligning,
    Stitching,
    Filling,
    Writing,
}

/// The frames aligned.
pub struct Aligned {
    /// At the ALIGN size — what a later [`align`] takes as `init`.
    pub align_rig: Rig,
    /// At the decode size — what the stitch uses.
    pub rig: Rig,
    /// Each frame's factor to the reference.
    pub gains: Vec<f32>,
    /// Each frame's lens, at the decode size.
    pub lens: Vec<FrameLens>,
    /// A lens LensFun does not know, for the dialog's line.
    pub no_profile: Option<String>,
}

/// The stitched panorama.
pub struct Stitched {
    /// Normalised raw camera RGB of the reference's camera, every pixel filled (edge-extended or Fill edges).
    pub image: ImageBuf,
    pub reference: usize,
    pub projection: Projection,
    /// Auto crop's rectangle, `None` when Fill edges ran or Auto crop is off.
    pub crop: Option<edit_model::CropRect>,
    pub seams: Labels,
}

fn cancelled(c: &AtomicBool) -> Result<(), MergeError> {
    if c.load(Ordering::Relaxed) {
        Err(MergeError::Cancelled)
    } else {
        Ok(())
    }
}

/// Align `d`'s frames — from `init` (a rig at the ALIGN size, the preview's) when given, else from features.
pub fn align(
    d: &Decoded,
    init: Option<&Rig>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Aligned, MergeError> {
    use rayon::prelude::*;
    cancelled(cancel)?;
    let n = d.frames.len();
    let (fw, fh) = (d.frames[0].width, d.frames[0].height);
    let focals: Vec<f32> = d
        .metas
        .iter()
        .filter_map(|m| m.metadata.lens.as_ref().map(|l| l.focal))
        .collect();
    if focals
        .iter()
        .any(|f| (f - focals[0]).abs() > FOCAL_TOLERANCE_MM)
    {
        return Err(MergeError::Focal);
    }
    let lens: Vec<FrameLens> = d
        .metas
        .iter()
        .map(|m| FrameLens::for_frame(&m.metadata, fw, fh))
        .collect();
    let no_profile = lens.iter().find_map(|l| l.missing().map(str::to_owned));

    // The ALIGN size's frames, lens-corrected: what features, refinement and the gains read.
    let halve = fw.max(fh) > ALIGN_LONG;
    let small: Vec<Frame> = d
        .frames
        .par_iter()
        .zip(&d.metas)
        .map(|(f, m)| {
            let f = if halve { f.half() } else { f.clone() };
            let l = FrameLens::for_frame(&m.metadata, f.width, f.height);
            lens::corrected(&f, &l)
        })
        .collect();
    let size = (small[0].width, small[0].height);
    cancelled(cancel)?;

    let (coarse, feature_corrs) = match init {
        Some(r) => {
            let s = size.0 as f64 / r.size.0 as f64;
            (
                if (s - 1.0).abs() > 1e-9 {
                    r.rescaled(s)
                } else {
                    r.clone()
                },
                Vec::new(),
            )
        }
        None => {
            progress(Progress::Matching);
            let guess = rig::focal_guess(&d.metas[0].metadata, size.0.max(size.1))
                .unwrap_or(size.0.max(size.1) as f64);
            let keys: Vec<Vec<Keypoint>> = small
                .par_iter()
                .map(|f| features::detect(f, FEATURE_LONG_EDGE))
                .collect();
            cancelled(cancel)?;
            let pairs: Vec<(usize, usize)> = (0..n)
                .flat_map(|i| (i + 1..n).map(move |j| (i, j)))
                .collect();
            let fits: Vec<_> = pairs
                .par_iter()
                .filter_map(|&(i, j)| {
                    rotation::fit_pair(
                        (i, j),
                        &keys[i],
                        &keys[j],
                        &features::matches(&keys[i], &keys[j]),
                        guess,
                        size,
                    )
                })
                .collect();
            let links: Vec<(usize, usize, usize)> = fits
                .iter()
                .map(|f| (f.inliers[0].a, f.inliers[0].b, f.inliers.len()))
                .collect();
            let order = rotation::chain(n, &links).map_err(|outside| {
                if outside.is_empty() {
                    MergeError::NotARow
                } else {
                    MergeError::Unconnected(outside)
                }
            })?;
            let corrs: Vec<Corr> = fits.into_iter().flat_map(|f| f.inliers).collect();
            let mut r = rig::solve(n, guess, size, &corrs, order);
            rig::level(&mut r);
            (r, corrs)
        }
    };
    cancelled(cancel)?;
    progress(Progress::Aligning);
    let pyr: Vec<LumPyramid> = small
        .par_iter()
        .map(|f| LumPyramid::build(f, crate::align::COARSEST))
        .collect();
    let refined = refine::refine(&coarse, &pyr, 0);
    // ⚠⚠ From the preview's rig there are no features: its own correspondences stand in, so a pair the
    // refinement drops keeps the preview's rotation and the bias guard below still has something to judge by.
    let base = if feature_corrs.is_empty() {
        rig_corrs(&coarse)
    } else {
        feature_corrs
    };
    let fine = refit(&coarse, &base, refined);
    let gains = gain::solve(
        &fine,
        &small,
        &(0..n).map(|_| FrameLens::NoLens).collect::<Vec<_>>(),
    );
    let rig = if halve {
        fine.rescaled(fw as f64 / size.0 as f64)
    } else {
        fine.clone()
    };
    Ok(Aligned {
        align_rig: fine,
        rig: Rig {
            size: (fw, fh),
            ..rig
        },
        gains,
        lens,
        no_profile,
    })
}

/// Every neighbouring pair's correspondences under `rig` — a 16 × 12 grid over the first frame, where the
/// second sees it.
pub(crate) fn rig_corrs(rig: &Rig) -> Vec<Corr> {
    let (w, h) = (rig.size.0 as f64, rig.size.1 as f64);
    let mut out = Vec::new();
    for pair in rig.order.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let hom = refine::pair_homography(rig, a, b);
        for j in 0..12 {
            for i in 0..16 {
                let p = ((i as f64 + 0.5) / 16.0 * w, (j as f64 + 0.5) / 12.0 * h);
                let q = hom.apply(p.0, p.1);
                if (0.0..w).contains(&q.0) && (0.0..h).contains(&q.1) {
                    out.push(Corr {
                        a,
                        b,
                        pa: [p.0, p.1],
                        pb: [q.0, q.1],
                    });
                }
            }
        }
    }
    out
}

/// The rig solved again from the dense refinement's correspondences, a pair it dropped keeping `base`'s —
/// and kept only if it fits `base` no worse than `coarse` does (refinement on frames whose vignetting is not
/// corrected is biased, `refine`'s note; `base` — features, or the preview's rig — is not).
pub(crate) fn refit(coarse: &Rig, base: &[Corr], refined: Vec<Corr>) -> Rig {
    if refined.is_empty() {
        return coarse.clone();
    }
    let n = coarse.rotations.len();
    let done: Vec<(usize, usize)> = refined.iter().map(|c| (c.a, c.b)).collect();
    let mut corrs = refined;
    corrs.extend(
        base.iter()
            .filter(|c| !done.contains(&(c.a, c.b)) && !done.contains(&(c.b, c.a)))
            .copied(),
    );
    let mut r = rig::solve(
        n,
        coarse.focal_px,
        coarse.size,
        &corrs,
        coarse.order.clone(),
    );
    rig::level(&mut r);
    if base.is_empty() || residual_px(&r, base) <= 1.5 * residual_px(coarse, base) + 0.2 {
        r
    } else {
        coarse.clone()
    }
}

/// The mean reprojection error of `corrs` under `rig`, pixels.
fn residual_px(rig: &Rig, corrs: &[Corr]) -> f64 {
    let errs: Vec<f64> = corrs
        .iter()
        .filter_map(|c| {
            let q = rig.pixel(c.b, rig.ray(c.a, c.pa[0], c.pa[1]))?;
            Some((q.0 - c.pb[0]).hypot(q.1 - c.pb[1]))
        })
        .collect();
    errs.iter().sum::<f64>() / errs.len().max(1) as f64
}

/// Stitch `d` under `a`'s rig onto a canvas whose long edge is at most `max_long_edge`.
pub fn stitch(
    d: &Decoded,
    a: &Aligned,
    opts: &PanoOptions,
    max_long_edge: usize,
    fill: Option<&mut dyn Inpainter>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Stitched, MergeError> {
    cancelled(cancel)?;
    progress(Progress::Stitching);
    let p = opts.projection.unwrap_or_else(|| projection::auto(&a.rig));
    let canvas = Canvas::plan(&a.rig, p, max_long_edge).map_err(|_| MergeError::TooWide)?;
    let mut warped = Vec::with_capacity(d.frames.len());
    for (i, f) in d.frames.iter().enumerate() {
        cancelled(cancel)?;
        warped.push(pano_warp::warp(
            &canvas, &a.rig, i, f, &a.lens[i], a.gains[i],
        ));
    }
    let seams = seam::cut(&canvas, &warped, &a.rig.order);
    cancelled(cancel)?;
    let (mut image, mut valid): (ImageBuf, Valid) =
        blend::multiband(&canvas, &warped, &seams, blend::LEVELS);
    drop(warped);
    let crop = match fill {
        Some(net) if opts.fill_edges => {
            progress(Progress::Filling);
            fill::fill_edges(&mut image, &mut valid, net, cancel)?;
            None
        }
        _ => {
            let rect = opts
                .auto_crop
                .then(|| crop::largest_rectangle(&valid))
                .flatten();
            crop::extend_edges(&mut image, &valid);
            rect.map(|r| crop::normalised(r, canvas.width, canvas.height))
        }
    };
    Ok(Stitched {
        image,
        reference: a.rig.reference,
        projection: p,
        crop,
        seams,
    })
}

/// The panorama in linear ProPhoto, as Develop would decode its DNG.
pub fn to_prophoto(s: &Stitched, d: &Decoded) -> ImageBuf {
    crate::hdr::raw_to_prophoto(&s.image, &d.metas[s.reference])
}

/// The panorama as a floating-point DNG at `path` — without lens tags: its lens is corrected already.
pub fn write_dng(
    s: &Stitched,
    d: &Decoded,
    path: &Path,
    preview: &image::RgbImage,
) -> Result<(), MergeError> {
    crate::hdr::write_raw_dng(&s.image, &d.metas[s.reference], path, preview, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdr::decode_all;
    use crate::synth_pano::{self, Row};

    /// A synthetic row written as M9.10 DNGs — 16-bit linear, which libraw reads as a raw. ⚠ Constructed:
    /// libraw decoding files of our own making. Frames of `scenes[i]` (most of them seed 1).
    pub(crate) fn dng_row(tag: &str, r: &Row, scenes: &[u64]) -> Vec<std::path::PathBuf> {
        synth_pano::write_dng_row(&test_dir::path(&format!("pano-{tag}")), r, scenes)
    }

    fn flat_row(n: usize) -> Row {
        let mut r = synth_pano::row(n, 0.35, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        r
    }

    fn decode(paths: &[std::path::PathBuf]) -> Decoded {
        decode_all(paths, false, &AtomicBool::new(false), &mut |_| {}).expect("decodes")
    }

    #[test]
    fn a_dng_row_merges_into_a_float_dng_that_reads_back() {
        let r = flat_row(3);
        let paths = dng_row("chain", &r, &[1, 1, 1]);
        let d = decode(&paths);
        let cancel = AtomicBool::new(false);
        let a = align(&d, None, &cancel, &mut |_| {}).expect("aligns");
        assert_eq!(a.rig.order.len(), 3);
        let s = stitch(
            &d,
            &a,
            &PanoOptions::default(),
            projection::MAX_LONG_EDGE,
            None,
            &cancel,
            &mut |_| {},
        )
        .expect("stitches");
        let (w, h) = (s.image.width(), s.image.height());
        assert!(w > 2 * r.w && h >= r.h - 20, "{w} × {h}");
        assert!(s.image.data().iter().all(|v| v.is_finite()));
        let c = s.crop.expect("Auto crop is on");
        assert!(
            c.left >= 0.0
                && c.right <= 1.0
                && c.top >= 0.0
                && c.bottom <= 1.0
                && c.right - c.left > 0.5
        );
        let out = paths[0].with_file_name("pano.dng");
        let preview = image::RgbImage::from_pixel(16, 12, image::Rgb([100, 100, 100]));
        write_dng(&s, &d, &out, &preview).expect("writes");
        let back = raw_decode::decode_raw(&out, &raw_decode::DecodeOptions::linear_scene())
            .expect("reads");
        assert_eq!((back.image.width(), back.image.height()), (w, h));
        assert_eq!(
            raw_decode::read_capture(&out).unwrap().lens,
            None,
            "no lens tags"
        );
    }

    #[test]
    fn the_merge_from_the_previews_rig_matches_a_fresh_one() {
        let r = flat_row(4);
        let d = decode(&dng_row("init", &r, &[1, 1, 1, 1]));
        let cancel = AtomicBool::new(false);
        let fresh = align(&d, None, &cancel, &mut |_| {}).unwrap();
        let mut events = Vec::new();
        let again = align(&d, Some(&fresh.align_rig), &cancel, &mut |p| events.push(p)).unwrap();
        assert!(
            !events.contains(&Progress::Matching),
            "features must not run: {events:?}"
        );
        let mut worst = 0f64;
        for w in fresh.rig.order.windows(2) {
            let (a, b) = (w[0], w[1]);
            for p in [(10.0, 10.0), (470.0, 300.0), (240.0, 160.0)] {
                let q1 = fresh.rig.pixel(b, fresh.rig.ray(a, p.0, p.1)).unwrap();
                let q2 = again.rig.pixel(b, again.rig.ray(a, p.0, p.1)).unwrap();
                worst = worst.max((q1.0 - q2.0).hypot(q1.1 - q2.1));
            }
        }
        assert!(worst < 0.1, "{worst} px");
    }

    /// ⚠⚠ The review's Critical: a merge started from the preview's rig matches no features, so a pair the
    /// dense refinement drops (it wandered, or found nothing) must keep the rotation the preview gave it —
    /// never collapse onto its neighbour. Here every pair's refined correspondences but 0→1's are kept.
    #[test]
    fn a_pair_the_refinement_drops_keeps_the_inits_rotation() {
        let r = flat_row(3);
        let d = decode(&dng_row("dropped", &r, &[1, 1, 1]));
        let a = align(&d, None, &AtomicBool::new(false), &mut |_| {}).unwrap();
        let init = &a.align_rig;
        let (p, q) = (init.order[0], init.order[1]);
        let refined: Vec<Corr> = rig_corrs(init)
            .into_iter()
            .filter(|c| (c.a, c.b) != (p, q))
            .collect();
        let fit = refit(init, &rig_corrs(init), refined);
        let angle = |rig: &Rig| {
            rig.rotations[q]
                .compose(&rig.rotations[p].transpose())
                .angle()
        };
        let (want, got) = (angle(init), angle(&fit));
        assert!(want > 0.3, "the fixture's frames are apart ({want} rad)");
        assert!(
            (got - want).abs() < 1e-3,
            "the dropped pair collapsed: {got} rad, the preview's {want}"
        );
    }

    /// The merge at the cap, its peak memory reported — six portrait X-T4-sized frames (4160 × 6240 at 23 mm,
    /// constructed), stitched onto the 16,000-px canvas and written. Release only:
    /// `cargo test --release --workspace a_merge_at_the_cap -- --ignored --nocapture`.
    #[test]
    #[ignore = "~26 Mpx frames; release only"]
    fn a_merge_at_the_cap_reports_its_peak_memory() {
        let mut r = synth_pano::row(6, 0.3, true);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        let r = Row {
            w: 4160,
            h: 6240,
            focal: 6107.0,
            ..r
        };
        let t = std::time::Instant::now();
        let paths = dng_row("cap", &r, &[1; 6]);
        eprintln!("fixture {:.1} s", t.elapsed().as_secs_f32());
        let peak = || {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with("VmHWM"))
                        .map(|l| l.trim_start_matches("VmHWM:").trim().to_owned())
                })
                .unwrap_or_default()
        };
        eprintln!("peak after the fixture: {}", peak());
        let cancel = AtomicBool::new(false);
        let t = std::time::Instant::now();
        let d = decode(&paths);
        let a = align(&d, None, &cancel, &mut |_| {}).unwrap();
        let aligned = t.elapsed();
        let s = stitch(
            &d,
            &a,
            &PanoOptions::default(),
            projection::MAX_LONG_EDGE,
            None,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        let stitched = t.elapsed();
        let out = paths[0].with_file_name("cap-Pano.dng");
        let preview = image::RgbImage::from_pixel(16, 12, image::Rgb([100, 100, 100]));
        write_dng(&s, &d, &out, &preview).unwrap();
        eprintln!(
            "{} × {}: decode+align {:.1} s, stitch {:.1} s, written {:.1} s; peak {}",
            s.image.width(),
            s.image.height(),
            aligned.as_secs_f32(),
            (stitched - aligned).as_secs_f32(),
            t.elapsed().as_secs_f32(),
            peak()
        );
        assert!(s.image.width().max(s.image.height()) <= projection::MAX_LONG_EDGE);
    }

    #[test]
    fn two_focal_lengths_are_refused() {
        let r = flat_row(2);
        let mut d = decode(&dng_row("focal", &r, &[1, 1]));
        for (i, m) in d.metas.iter_mut().enumerate() {
            m.metadata.lens = Some(raw_decode::LensMetadata {
                make: "FUJIFILM".into(),
                name: "XF16-80mmF4 R OIS WR".into(),
                focal: [16.0, 35.0][i],
                aperture: 4.0,
            });
        }
        let r = align(&d, None, &AtomicBool::new(false), &mut |_| {});
        assert!(matches!(r, Err(MergeError::Focal)), "{:?}", r.err());
    }

    #[test]
    fn an_unconnected_frame_is_named() {
        let r = flat_row(4);
        let d = decode(&dng_row("unconnected", &r, &[1, 1, 1, 2]));
        let r = align(&d, None, &AtomicBool::new(false), &mut |_| {});
        assert!(
            matches!(&r, Err(MergeError::Unconnected(v)) if v == &vec![3]),
            "{:?}",
            r.err()
        );
    }

    #[test]
    fn a_cancel_stops_the_merge() {
        let r = flat_row(2);
        let d = decode(&dng_row("cancel", &r, &[1, 1]));
        let r = align(&d, None, &AtomicBool::new(true), &mut |_| {});
        assert!(matches!(r, Err(MergeError::Cancelled)));
    }

    #[test]
    fn auto_crop_off_leaves_no_crop() {
        let r = flat_row(2);
        let d = decode(&dng_row("nocrop", &r, &[1, 1]));
        let cancel = AtomicBool::new(false);
        let a = align(&d, None, &cancel, &mut |_| {}).unwrap();
        let opts = PanoOptions {
            auto_crop: false,
            ..PanoOptions::default()
        };
        let s = stitch(
            &d,
            &a,
            &opts,
            projection::MAX_LONG_EDGE,
            None,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        assert!(s.crop.is_none());
        assert!(s.image.data().iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    /// Release only: `cargo test --release --workspace the_stitch_takes -- --ignored --nocapture`.
    ///
    /// ⚠ The spec's bar is the stitch against the DECODES, but a constructed linear DNG decodes in a fraction of
    /// a second where an X-T4 raw takes 8–9 s (M10.17a's probe, three at once). So the parts are measured here
    /// and extrapolated to six X-T4 frames at the cap — alignment by the align size's pixels, the stitch by the
    /// canvas's — against six real decodes; the real measurement is the hand-off's (`PANO_FIXTURE`).
    #[test]
    #[ignore = "a timing, release only"]
    fn the_stitch_takes_no_longer_than_the_decodes() {
        let mut r = synth_pano::row(6, 0.33, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        let r = Row {
            w: 2400,
            h: 1600,
            focal: r.focal * 5.0,
            ..r
        };
        let paths = dng_row("timing", &r, &[1; 6]);
        let cancel = AtomicBool::new(false);
        let d = decode(&paths);
        let t = std::time::Instant::now();
        let a = align(&d, None, &cancel, &mut |_| {}).unwrap();
        let aligning = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let s = stitch(
            &d,
            &a,
            &PanoOptions::default(),
            projection::MAX_LONG_EDGE,
            None,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
        let stitching = t.elapsed().as_secs_f64();
        let canvas = (s.image.width() * s.image.height()) as f64;
        // Six portrait X-T4 frames: aligned at 3120 × 2080, stitched onto ~16,000 × 5,800.
        let at_cap = aligning * (3120.0 * 2080.0) / (r.w * r.h) as f64
            + stitching * (16_000.0 * 5_800.0) / canvas;
        let decodes = 6.0 * 8.5 / 3.0;
        eprintln!(
            "align {aligning:.2} s, stitch {stitching:.2} s on {} × {}; at the cap ≈ {at_cap:.1} s against ≈ {decodes:.0} s of decodes",
            s.image.width(),
            s.image.height()
        );
        assert!(at_cap <= decodes, "{at_cap:.1} s");
    }
}
