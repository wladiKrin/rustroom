//! HDR merge, end to end: decode a bracket, align it, measure it, deghost it, merge it, write it.
//!
//! The dialog's preview runs it on half-size decodes ([`decode_all`]`(…, half: true, …)`); the merge
//! itself on full ones, starting each alignment from the preview's (rescaled ×2).

use crate::align::{self, Alignment, COARSEST, MIN_OVERLAP};
pub use crate::deghost::Deghost;
use crate::deghost::{self, Mask};
use crate::frame::{exposure_ev, Frame};
use crate::homography::Homography;
use crate::pyramid::LumPyramid;
use crate::{exposure, merge as merging};
use core_image::ImageBuf;
use raw_decode::{CameraFrame, DecodeOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The finest pyramid level a full-size merge refines its alignment at: half size. The preview has
/// aligned the half-size decodes already; the full ones refine from there.
pub const FULL_FINEST: usize = 1;

/// What the dialog decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HdrOptions {
    pub auto_align: bool,
    pub deghost: Deghost,
}

impl Default for HdrOptions {
    /// Lightroom's.
    fn default() -> Self {
        Self {
            auto_align: true,
            deghost: Deghost::None,
        }
    }
}

/// Where a merge is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    /// About to decode frame `done + 1` of `total`.
    Decoding {
        done: usize,
        total: usize,
    },
    Aligning,
    Merging,
    Writing,
}

#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error("could not decode {0}: {1}")]
    Decode(PathBuf, raw_decode::DecodeError),
    #[error("cancelled")]
    Cancelled,
    #[error("the photos are different sizes")]
    Sizes,
    #[error("the photos are from different cameras")]
    Cameras,
    #[error("the photos were taken at different focal lengths")]
    Focal,
    #[error("some photos share nothing with the others")]
    Unconnected(Vec<usize>),
    #[error("the photos do not form one row")]
    NotARow,
    #[error("the panorama is too wide for this projection")]
    TooWide,
    #[error("could not fill the edges: {0}")]
    Fill(String),
    #[error("could not write the merged photo: {0}")]
    Write(#[from] export::ExportError),
}

/// A bracket, decoded.
pub struct Decoded {
    pub frames: Vec<Frame>,
    /// Each frame's libraw facts — matrices, metadata, capture — with the samples moved out into
    /// `frames`.
    pub metas: Vec<CameraFrame>,
    /// Half-size decodes (the preview's) or full.
    pub half: bool,
}

/// The merged photo.
pub struct Merged {
    /// Normalised raw camera RGB, the reference's geometry and exposure.
    pub image: ImageBuf,
    pub reference: usize,
    /// Reference pixels → each frame's, at this merge's size.
    pub transforms: Vec<Homography>,
    /// Each frame's `reference / frame` exposure.
    pub scales: Vec<f32>,
    /// What deghosting took from one frame alone; `None` at [`Deghost::None`].
    pub overlay: Option<Mask>,
}

fn cancelled(c: &AtomicBool) -> Result<(), MergeError> {
    if c.load(Ordering::Relaxed) {
        Err(MergeError::Cancelled)
    } else {
        Ok(())
    }
}

/// Which frame the merge is built on: the median exposure — EV from the files, or mean brightness
/// when any lacks one — ties going to the active photo, then the darker. With an even count both
/// middle frames are "the median".
pub fn reference_index(evs: &[Option<f32>], brightness: &[f32], active: Option<usize>) -> usize {
    let n = evs.len();
    let key: Vec<f32> = if evs.iter().all(Option::is_some) {
        evs.iter().map(|e| e.unwrap_or(0.0)).collect()
    } else {
        brightness.iter().map(|b| b.max(1e-12).log2()).collect()
    };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| key[a].total_cmp(&key[b]).then(a.cmp(&b)));
    let middle: Vec<usize> = if n % 2 == 1 {
        let m = key[order[n / 2]];
        order
            .iter()
            .copied()
            .filter(|&i| (key[i] - m).abs() < 1e-3)
            .collect()
    } else {
        let (lo, hi) = (key[order[n / 2 - 1]], key[order[n / 2]]);
        order
            .iter()
            .copied()
            .filter(|&i| (key[i] - lo).abs() < 1e-3 || (key[i] - hi).abs() < 1e-3)
            .collect()
    };
    if let Some(a) = active.filter(|a| middle.contains(a)) {
        return a;
    }
    middle.first().copied().unwrap_or(0)
}

/// How many photos are decoded at once. libraw demosaics a frame on one thread, so three at a time
/// is nearly three times as fast; more would cost ~0.5 GB of libraw's buffers each for little more.
const DECODE_AT_ONCE: usize = 3;

/// Decode every photo of the bracket as libraw's camera-space output, [`DECODE_AT_ONCE`] at a time;
/// refuse two sizes or two cameras. `Progress::Decoding` reports how many are done.
pub fn decode_all(
    paths: &[PathBuf],
    half: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Decoded, MergeError> {
    use rayon::prelude::*;
    let opts = DecodeOptions {
        half_size: half,
        ..DecodeOptions::linear_scene()
    };
    let mut cams: Vec<CameraFrame> = Vec::with_capacity(paths.len());
    for chunk in paths.chunks(DECODE_AT_ONCE) {
        cancelled(cancel)?;
        progress(Progress::Decoding {
            done: cams.len(),
            total: paths.len(),
        });
        let decoded: Vec<Result<CameraFrame, MergeError>> = chunk
            .par_iter()
            .map(|p| {
                raw_decode::decode_camera(p, &opts).map_err(|e| MergeError::Decode(p.clone(), e))
            })
            .collect();
        for c in decoded {
            cams.push(c?);
        }
    }
    cancelled(cancel)?;
    progress(Progress::Decoding {
        done: cams.len(),
        total: paths.len(),
    });
    let first = &cams[0];
    let (size, camera) = (
        (first.width, first.height),
        (first.metadata.make.clone(), first.metadata.model.clone()),
    );
    if cams.iter().any(|c| (c.width, c.height) != size) {
        return Err(MergeError::Sizes);
    }
    if cams
        .iter()
        .any(|c| (&c.metadata.make, &c.metadata.model) != (&camera.0, &camera.1))
    {
        return Err(MergeError::Cameras);
    }
    let mut frames = Vec::with_capacity(cams.len());
    for c in &mut cams {
        let samples = std::mem::take(&mut c.samples);
        frames.push(Frame::new(
            c.width,
            c.height,
            samples,
            c.pre_mul,
            exposure_ev(&c.capture),
        ));
    }
    Ok(Decoded {
        frames,
        metas: cams,
        half,
    })
}

/// A frame's mean raw luminance on a sparse grid — the reference's fallback key.
fn brightness(f: &Frame) -> f32 {
    let step = (f.width.max(f.height) / 256).max(1);
    let (mut s, mut n) = (0f64, 0usize);
    for y in (0..f.height).step_by(step) {
        for x in (0..f.width).step_by(step) {
            let r = f.raw(x, y);
            s += (0.25 * r[0] + 0.5 * r[1] + 0.25 * r[2]) as f64;
            n += 1;
        }
    }
    (s / n.max(1) as f64) as f32
}

/// Align, measure, deghost and merge `d` — [`align_all`] then [`merge_aligned`].
pub fn merge(
    d: &Decoded,
    active: Option<usize>,
    opts: &HdrOptions,
    init: Option<&[Homography]>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Merged, MergeError> {
    let a = align_all(d, active, opts, init, cancel, progress)?;
    merge_aligned(d, &a, opts.deghost, cancel, progress)
}

/// How a bracket sits on its reference — what a change of Deghost keeps (the dialog re-merges in
/// well under a second without re-aligning).
#[derive(Clone, Debug, PartialEq)]
pub struct Aligned {
    pub reference: usize,
    /// Reference pixels → each frame's, at the decode's size.
    pub transforms: Vec<Homography>,
    /// Each frame's `reference / frame` exposure.
    pub scales: Vec<f32>,
}

/// Choose the reference and align every frame to it, measuring each one's exposure. `active` breaks
/// a tie for the reference; `init` starts each frame's alignment (reference → frame, at `d`'s size)
/// — the preview's, rescaled.
pub fn align_all(
    d: &Decoded,
    active: Option<usize>,
    opts: &HdrOptions,
    init: Option<&[Homography]>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Aligned, MergeError> {
    let n = d.frames.len();
    let evs: Vec<Option<f32>> = d.frames.iter().map(|f| f.ev).collect();
    let bright: Vec<f32> = d.frames.iter().map(brightness).collect();
    let reference = reference_index(&evs, &bright, active);
    // Each frame's exposure relative to the reference, in stops, from the files when they say.
    let ev_gain = |i: usize| match (evs[i], evs[reference]) {
        (Some(a), Some(b)) => a - b,
        _ => (bright[i].max(1e-12) / bright[reference].max(1e-12)).log2(),
    };

    progress(Progress::Aligning);
    cancelled(cancel)?;
    let mut aligned: Vec<Option<Alignment>> = vec![None; n];
    aligned[reference] = Some(Alignment {
        h: Homography::identity(),
        log_gain: 0.0,
        overlap: 1.0,
        residual: 0.0,
    });
    if opts.auto_align {
        let pyr: Vec<LumPyramid> = d
            .frames
            .iter()
            .map(|f| LumPyramid::build(f, COARSEST))
            .collect();
        let finest = if d.half { 0 } else { FULL_FINEST };
        // Nearest the reference in exposure first, so a frame that must go through its neighbour
        // finds it aligned.
        let mut order: Vec<usize> = (0..n).filter(|&i| i != reference).collect();
        order.sort_by(|&a, &b| ev_gain(a).abs().total_cmp(&ev_gain(b).abs()));
        for &i in &order {
            cancelled(cancel)?;
            let start = init.map(|h| Alignment {
                h: h[i],
                log_gain: ev_gain(i),
                overlap: 1.0,
                residual: 0.0,
            });
            let mut a = align::align(&pyr[reference], &pyr[i], start.as_ref(), finest);
            if a.overlap < MIN_OVERLAP {
                // Through the aligned frame nearest it in exposure.
                let via = (0..n)
                    .filter(|&j| j != i && aligned[j].is_some())
                    .min_by(|&p, &q| {
                        (ev_gain(p) - ev_gain(i))
                            .abs()
                            .total_cmp(&(ev_gain(q) - ev_gain(i)).abs())
                    });
                if let Some(j) = via.filter(|&j| j != reference) {
                    let aj = aligned[j].expect("aligned");
                    let ij = align::align(&pyr[j], &pyr[i], None, finest);
                    if ij.overlap >= MIN_OVERLAP {
                        a = Alignment {
                            h: ij.h.compose(&aj.h),
                            log_gain: aj.log_gain + ij.log_gain,
                            overlap: ij.overlap,
                            residual: ij.residual,
                        };
                    }
                }
            }
            if a.overlap < MIN_OVERLAP {
                a = Alignment {
                    h: Homography::identity(),
                    log_gain: ev_gain(i),
                    ..a
                };
            }
            aligned[i] = Some(a);
        }
    } else {
        for (i, slot) in aligned.iter_mut().enumerate() {
            if i != reference {
                *slot = Some(Alignment {
                    h: Homography::identity(),
                    log_gain: ev_gain(i),
                    overlap: 1.0,
                    residual: 0.0,
                });
            }
        }
    }
    let aligned: Vec<Alignment> = aligned
        .into_iter()
        .map(|a| a.expect("every frame"))
        .collect();
    let transforms: Vec<Homography> = aligned.iter().map(|a| a.h).collect();
    let scales: Vec<f32> = (0..n)
        .map(|i| {
            if i == reference {
                1.0
            } else {
                exposure::measure(&d.frames[reference], &d.frames[i], &aligned[i])
            }
        })
        .collect();

    Ok(Aligned {
        reference,
        transforms,
        scales,
    })
}

/// Deghost and merge an aligned bracket.
pub fn merge_aligned(
    d: &Decoded,
    a: &Aligned,
    level: Deghost,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Merged, MergeError> {
    progress(Progress::Merging);
    cancelled(cancel)?;
    let (ghost, overlay) = deghost::masks(&d.frames, a.reference, &a.transforms, &a.scales, level);
    cancelled(cancel)?;
    let image = merging::merge(&d.frames, a.reference, &a.transforms, &a.scales, &ghost);
    Ok(Merged {
        image,
        reference: a.reference,
        transforms: a.transforms.clone(),
        scales: a.scales.clone(),
        overlay,
    })
}

/// The merge in linear ProPhoto, as Develop would decode its DNG: the reference's white balance,
/// then its camera matrix.
pub fn to_prophoto(m: &Merged, d: &Decoded) -> ImageBuf {
    raw_to_prophoto(&m.image, &d.metas[m.reference])
}

/// Normalised raw camera RGB of `meta`'s camera in linear ProPhoto: its white balance, then its matrix.
pub fn raw_to_prophoto(image: &ImageBuf, meta: &CameraFrame) -> ImageBuf {
    let mat = color::camera::camera_to_prophoto(meta.rgb_cam);
    let k = meta.pre_mul;
    image.map_pixels(|p| color::spaces::apply(&mat, [p[0] * k[0], p[1] * k[1], p[2] * k[2]]))
}

/// Write the merge as the floating-point DNG at `path`, with `preview` as its picture.
pub fn write_dng(
    m: &Merged,
    d: &Decoded,
    path: &Path,
    preview: &image::RgbImage,
) -> Result<(), MergeError> {
    write_raw_dng(&m.image, &d.metas[m.reference], path, preview, true)
}

/// `image` — normalised raw camera RGB of `meta`'s camera — as a floating-point DNG at `path`, its
/// capture facts `meta`'s. `lens_tags` off leaves out the lens make and model (the focal length stays):
/// a panorama's lens is already corrected, and a named lens would be corrected again (M10.17b).
pub fn write_raw_dng(
    image: &ImageBuf,
    meta: &CameraFrame,
    path: &Path,
    preview: &image::RgbImage,
    lens_tags: bool,
) -> Result<(), MergeError> {
    let lens_make = meta
        .metadata
        .lens
        .as_ref()
        .map(|l| l.make.as_str())
        .filter(|s| lens_tags && !s.is_empty());
    let unnamed;
    let capture = if lens_tags {
        &meta.capture
    } else {
        unnamed = core_image::Capture {
            lens: None,
            ..meta.capture.clone()
        };
        &unnamed
    };
    export::dng_hdr::save(
        &export::dng_hdr::HdrDng {
            width: image.width(),
            height: image.height(),
            data: image.data(),
            make: &meta.metadata.make,
            model: &meta.metadata.model,
            cam_xyz: meta.cam_xyz,
            as_shot_neutral: meta.pre_mul.map(|v| 1.0 / v),
            capture,
            lens_make,
            preview,
        },
        path,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn the_reference_is_the_median_exposure() {
        assert_eq!(
            reference_index(&[Some(0.0), Some(-2.0), Some(2.0)], &[0.5, 0.1, 0.9], None),
            0
        );
        assert_eq!(
            reference_index(
                &[Some(1.0), Some(-1.0), Some(3.0), Some(-3.0), Some(0.0)],
                &[0.0; 5],
                None
            ),
            4
        );
    }

    #[test]
    fn without_exposures_the_reference_is_the_median_brightness() {
        assert_eq!(
            reference_index(&[None, Some(1.0), None], &[0.4, 0.1, 0.2], None),
            2
        );
    }

    #[test]
    fn a_tie_goes_to_the_active_photo() {
        assert_eq!(reference_index(&[Some(0.0); 3], &[0.3; 3], Some(2)), 2);
        assert_eq!(reference_index(&[Some(0.0); 3], &[0.3; 3], None), 0);
        // Two frames: both are the middle — the active one, else the darker.
        assert_eq!(
            reference_index(&[Some(0.0), Some(-2.0)], &[0.5, 0.1], Some(0)),
            0
        );
        assert_eq!(
            reference_index(&[Some(0.0), Some(-2.0)], &[0.5, 0.1], None),
            1
        );
    }

    /// A bracket of M9.10 DNGs — 16-bit linear ProPhoto, which libraw reads as a raw — rendered from
    /// the synthetic scene at `evs`. ⚠ Constructed: libraw decoding real files of our own making.
    fn dng_bracket(tag: &str, w: usize, h: usize, evs: &[f32]) -> Vec<std::path::PathBuf> {
        let dir = test_dir::path(&format!("hdr-{tag}"));
        evs.iter()
            .enumerate()
            .map(|(i, ev)| {
                let mut img = ImageBuf::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let s = crate::synth::scene(x as f64, y as f64, w, h);
                        img.set_pixel(x, y, s.map(|c| c * 2f32.powf(*ev)));
                    }
                }
                let p = dir.join(format!("frame{i}.dng"));
                export::dng::save(&img, &p).unwrap();
                p
            })
            .collect()
    }

    /// ⚠⚠ The whole chain on files: decode → align → measure → merge → the float DNG → Rustroom's
    /// reader. A highlight the reference clipped reads back as the scene, above 1.
    ///
    /// ⚠ 300 px wide, not less: an M9.10 DNG narrower than one 256-px tile reads back scrambled in
    /// libraw (measured 2026-10-26: worst 0.99 at 160 and 255 px, exact from 256) — a defect of the
    /// rendered-DNG export, recorded in the journal, which a 160-px fixture here first ran into.
    #[test]
    fn a_dng_bracket_merges_into_a_float_dng_that_reads_back() {
        let (w, h) = (300, 200);
        let paths = dng_bracket("chain", w, h, &[-2.0, 0.0, 2.0]);
        let cancel = AtomicBool::new(false);
        let d = decode_all(&paths, false, &cancel, &mut |_| {}).expect("decodes");
        let opts = HdrOptions {
            auto_align: true,
            deghost: Deghost::None,
        };
        let m = merge(&d, None, &opts, None, &cancel, &mut |_| {}).expect("merges");
        assert_eq!(m.reference, 1, "the middle brightness");
        let out = paths[0].with_file_name("merged.dng");
        let preview = image::RgbImage::from_pixel(16, 12, image::Rgb([100, 100, 100]));
        write_dng(&m, &d, &out, &preview).expect("writes");
        let back = raw_decode::decode_raw(&out, &raw_decode::DecodeOptions::linear_scene())
            .expect("reads");
        assert_eq!((back.image.width(), back.image.height()), (w, h));
        let (mut err, mut n) = (0f64, 0usize);
        for y in 4..h - 4 {
            for x in 4..w - 4 {
                let t = crate::synth::scene(x as f64, y as f64, w, h)[1];
                if !(1.5..3.5).contains(&t) {
                    continue;
                }
                err += ((back.image.pixel(x, y)[1] - t) / t).abs() as f64;
                n += 1;
            }
        }
        let mean = err / n.max(1) as f64;
        eprintln!("highlights above the reference's white: {n} px, mean error {mean:.4}");
        assert!(
            n > 200,
            "the scene must have highlights the reference clipped ({n})"
        );
        assert!(mean < 0.02, "{mean}");
    }

    /// A panorama's lens is corrected before it is stitched, so its DNG must not name the lens — or
    /// Develop's automatic lens correction would correct it twice. The focal length stays.
    #[test]
    fn a_dng_without_lens_tags_reads_back_without_a_lens() {
        let paths = dng_bracket("lens-tags", 300, 200, &[0.0]);
        let cancel = AtomicBool::new(false);
        let mut d = decode_all(&paths, false, &cancel, &mut |_| {}).expect("decodes");
        let meta = &mut d.metas[0];
        meta.capture.lens = Some("XF23mmF2 R WR".into());
        meta.capture.focal_len = Some(23.0);
        meta.metadata.lens = Some(raw_decode::LensMetadata {
            make: "FUJIFILM".into(),
            name: "XF23mmF2 R WR".into(),
            focal: 23.0,
            aperture: 2.0,
        });
        let img = ImageBuf::new(300, 200);
        let preview = image::RgbImage::from_pixel(16, 12, image::Rgb([100, 100, 100]));
        let (with, without) = (
            paths[0].with_file_name("with.dng"),
            paths[0].with_file_name("without.dng"),
        );
        write_raw_dng(&img, &d.metas[0], &with, &preview, true).expect("writes");
        write_raw_dng(&img, &d.metas[0], &without, &preview, false).expect("writes");
        let c = raw_decode::read_capture(&with).expect("reads");
        assert_eq!(
            c.lens.as_deref(),
            Some("XF23mmF2 R WR"),
            "the control: the tag is written when asked"
        );
        let c = raw_decode::read_capture(&without).expect("reads");
        assert_eq!(c.lens, None);
        assert_eq!(c.focal_len, Some(23.0));
    }

    #[test]
    fn a_cancel_between_frames_stops_with_cancelled() {
        let paths = dng_bracket("cancel", 64, 48, &[-1.0, 0.0]);
        let cancel = AtomicBool::new(false);
        let r = decode_all(&paths, false, &cancel, &mut |p| {
            // ⚠ Frames decode three at a time, so progress comes per chunk: cancel at the first.
            if let Progress::Decoding { done: 0, .. } = p {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        assert!(matches!(r, Err(MergeError::Cancelled)), "{:?}", r.err());
    }

    #[test]
    fn two_sizes_are_refused() {
        let mut paths = dng_bracket("sizes-a", 64, 48, &[0.0]);
        paths.extend(dng_bracket("sizes-b", 48, 64, &[-1.0]));
        let r = decode_all(&paths, false, &AtomicBool::new(false), &mut |_| {});
        assert!(matches!(r, Err(MergeError::Sizes)), "{:?}", r.err());
    }

    /// ⚠⚠ One frame "merged", written and read back through Develop's decode is the raw itself —
    /// white balance, matrix and orientation all survive. ⚠ Gated on `RAW_FIXTURE`; it skips without.
    #[test]
    fn a_one_frame_merge_reproduces_the_raw() {
        let Some(raf) = std::env::var("RAW_FIXTURE")
            .ok()
            .map(std::path::PathBuf::from)
        else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let cancel = AtomicBool::new(false);
        let d = decode_all(std::slice::from_ref(&raf), true, &cancel, &mut |_| {}).unwrap();
        let m = merge(&d, None, &HdrOptions::default(), None, &cancel, &mut |_| {}).unwrap();
        let out = test_dir::path("hdr-one-frame").join("one.dng");
        write_dng(&m, &d, &out, &image::RgbImage::new(8, 8)).unwrap();
        let ours =
            raw_decode::decode_raw(&out, &raw_decode::DecodeOptions::linear_scene()).unwrap();
        let theirs = raw_decode::decode_raw(&raf, &raw_decode::DecodeOptions::preview()).unwrap();
        assert_eq!(
            (ours.image.width(), ours.image.height()),
            (theirs.image.width(), theirs.image.height()),
            "size and orientation"
        );
        let (a, b) = (
            ours.metadata.as_shot.unwrap(),
            theirs.metadata.as_shot.unwrap(),
        );
        eprintln!("as shot: {:.1} K / {:.1} K", a.kelvin, b.kelvin);
        assert!((a.kelvin - b.kelvin).abs() < 1.0 && (a.tint - b.tint).abs() < 0.1);
        let (mut worst, mut n) = (0f32, 0usize);
        for (p, q) in ours
            .image
            .data()
            .chunks(3)
            .zip(theirs.image.data().chunks(3))
        {
            let top = q.iter().cloned().fold(0f32, f32::max);
            if top > 0.95 || top < 1e-3 {
                continue;
            }
            for c in 0..3 {
                worst = worst.max((p[c] - q[c]).abs() / top);
            }
            n += 1;
        }
        eprintln!("one-frame merge vs the raw: worst {worst:.5} over {n} px");
        assert!(worst < 2e-3, "{worst}");
    }

    /// ⚠⚠ The author's own brackets: every frame aligns over most of the reference, and its measured
    /// exposure agrees with an independent one — the plain median ratio of the frames' raw green,
    /// pixel for pixel, no alignment (they barely moved). ⚠ Not with EXIF: on DSCF7753–55 the file
    /// says 1/80 against 1/60 and the pixels say the same exposure (+0.002 EV), measured three ways.
    /// ⚠ Gated on `HDR_FIXTURE` — the bracket's files joined by `:` — and skips without it.
    #[test]
    fn a_real_bracket_aligns_and_measures_like_its_pixels() {
        let Ok(list) = std::env::var("HDR_FIXTURE") else {
            eprintln!("skipping: set HDR_FIXTURE=a.raw:b.raw:c.raw to run this test");
            return;
        };
        let paths: Vec<std::path::PathBuf> =
            list.split(':').map(std::path::PathBuf::from).collect();
        let cancel = AtomicBool::new(false);
        let d = decode_all(&paths, true, &cancel, &mut |_| {}).unwrap();
        let evs: Vec<Option<f32>> = d.frames.iter().map(|f| f.ev).collect();
        let r = reference_index(&evs, &vec![0.0; evs.len()], None);
        let pyr: Vec<LumPyramid> = d
            .frames
            .iter()
            .map(|f| LumPyramid::build(f, COARSEST))
            .collect();
        for i in (0..paths.len()).filter(|&i| i != r) {
            let a = align::align(&pyr[r], &pyr[i], None, 0);
            let scale = exposure::measure(&d.frames[r], &d.frames[i], &a);
            let (fr, ff) = (&d.frames[r], &d.frames[i]);
            let mut ratios: Vec<f32> = Vec::new();
            for y in (0..fr.height).step_by(7) {
                for x in (0..fr.width).step_by(7) {
                    let (p, q) = (fr.raw(x, y)[1], ff.raw(x, y)[1]);
                    if !fr.clipped(x, y)
                        && !ff.clipped(x, y)
                        && (0.05..0.7).contains(&p)
                        && (0.05..0.7).contains(&q)
                    {
                        ratios.push(p / q);
                    }
                }
            }
            ratios.sort_by(|a, b| a.total_cmp(b));
            let naive = ratios[ratios.len() / 2];
            eprintln!(
                "{}: overlap {:.3}, scale {scale:.4}, unaligned {naive:.4}",
                paths[i].display(),
                a.overlap
            );
            assert!(a.overlap > 0.5, "overlap {}", a.overlap);
            assert!((scale / naive - 1.0).abs() < 0.03, "{scale} vs {naive}");
        }
    }
}
