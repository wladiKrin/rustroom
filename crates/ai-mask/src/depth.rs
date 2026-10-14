//! Depth Anything V2 Small (M10.16a): a relative depth estimate of the whole photo, for the Depth range
//! mask and, next, Lens Blur.
//!
//! ⚠ The network answers **disparity** — large when near — up to an unknown scale and shift, so only
//! the ORDER of its values means anything. [`normalise`] turns it into a position, 0 the nearest point
//! of this photo and 1 the farthest, between robust percentiles so a stray pixel cannot claim the scale.

use crate::infer::InferError;
use image::RgbImage;

/// A plane of `f32`, row-major, `w × h`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plane {
    pub w: u32,
    pub h: u32,
    pub data: Vec<f32>,
}

/// How a photo's disparities are laid out along the 0..1 position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthScale {
    /// Linear in disparity: equal steps of nearness. Distant things crowd into the far end.
    Linear,
    /// Each step an equal share of the photo's pixels — the far end spread out.
    Equalised,
}

/// The network's input size for a `w × h` photo: the aspect kept, the long side about `long`, both
/// sides multiples of 14 (the ViT's patch).
pub fn input_size(w: u32, h: u32, long: u32) -> (u32, u32) {
    let s = long as f32 / w.max(h).max(1) as f32;
    let side = |v: u32| (((v as f32 * s) / 14.0).round().max(1.0) as u32) * 14;
    (side(w), side(h))
}

/// The value at fraction `q` of `sorted`, which is non-empty.
fn percentile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q).round() as usize]
}

/// Disparity → position, 0 nearest … 1 farthest, every value finite and in `[0, 1]`.
///
/// ⚠⚠ **Between the 1st and 99th percentiles of the photo's own finite values** — a speck of sky
/// through leaves, or one pixel the network answered wildly, would otherwise claim an end of the scale
/// and squash everything else into the middle. A non-finite value, and a photo with no spread at all
/// (a wall, a clear sky), answer 0.5: the middle, rather than a NaN that would quantize to black.
pub fn normalise(disparity: &Plane, scale: DepthScale) -> Plane {
    let mut finite: Vec<f32> = disparity
        .data
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    finite.sort_by(f32::total_cmp);
    let middle = || Plane {
        w: disparity.w,
        h: disparity.h,
        data: vec![0.5; disparity.data.len()],
    };
    if finite.is_empty() {
        return middle();
    }
    let (lo, hi) = (percentile(&finite, 0.01), percentile(&finite, 0.99));
    let span = hi - lo;
    if span <= 1e-9 {
        return middle();
    }
    let data = disparity
        .data
        .iter()
        .map(|&v| {
            if !v.is_finite() {
                return 0.5;
            }
            let v = v.clamp(lo, hi);
            // How near, 0..1 — disparity is LARGE when near.
            let near = match scale {
                DepthScale::Linear => (v - lo) / span,
                // The share of the photo's pixels that are farther than this one.
                DepthScale::Equalised => {
                    finite.partition_point(|&x| x < v) as f32 / finite.len() as f32
                }
            };
            (1.0 - near).clamp(0.0, 1.0)
        })
        .collect();
    Plane {
        w: disparity.w,
        h: disparity.h,
        data,
    }
}

/// A loaded Depth Anything V2 Small.
///
/// ⚠ `tract` needs the input's shape as a concrete fact, so one runnable plan is built per input size
/// and kept — the SAM decoder's precedent. A photo's aspect decides the size, so a session sees a
/// handful (3:2, 2:3, 4:3…).
///
/// ⚠⚠ **Two traps in this export, each a refusal at load (M10.16a spike):** (1) the graph carries its
/// own symbolic dimensions (`batch_size`, `height`, `width`) all the way through, so a concrete input
/// fact alone fails to unify `Sym(batch_size)` with `1` at the first convolution — the symbols are
/// BOUND per size instead ([`TypedModel::set_symbols`]); (2) the exporter annotated intermediate shapes
/// with symbols of its own (`ConvTranspose_1022_o0__d1`) that contradict the real 48 channels — those
/// annotations are IGNORED (`with_ignore_value_info`, `with_ignore_output_shapes`) and `tract` infers
/// the shapes itself.
pub struct Depth {
    /// The typed graph, its symbols still open — bound into each size's plan.
    model: tract_onnx::prelude::TypedModel,
    plans: std::collections::HashMap<(u32, u32), Plan>,
}

type Plan = std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>;

fn err(e: impl std::fmt::Display) -> InferError {
    InferError::Model(e.to_string())
}

impl Depth {
    /// Parses the model; the plans are built on first use per size.
    pub fn load(path: &std::path::Path) -> Result<Self, InferError> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .with_ignore_value_info(true)
            .with_ignore_output_shapes(true)
            .model_for_path(path)
            .map_err(err)?
            .into_typed()
            .map_err(err)?;
        Ok(Self {
            model,
            plans: std::collections::HashMap::new(),
        })
    }

    /// The raw **disparity** over `img` (upright), at [`input_size`]`(img, long)`.
    ///
    /// ⚠ The recipe Depth Anything V2's own inference uses: RGB/255, ImageNet mean and std, NCHW, both
    /// sides multiples of 14; one output, `(1, h, w)`, at the input's size.
    pub fn run(&mut self, img: &RgbImage, long: u32) -> Result<Plane, InferError> {
        use tract_onnx::prelude::*;
        let (w, h) = input_size(img.width(), img.height(), long);
        if !self.plans.contains_key(&(w, h)) {
            let subs: std::collections::HashMap<Symbol, TDim> = [
                (self.model.sym("batch_size"), 1.to_dim()),
                (self.model.sym("height"), (h as usize).to_dim()),
                (self.model.sym("width"), (w as usize).to_dim()),
            ]
            .into_iter()
            .collect();
            let plan = self
                .model
                .set_symbols(&subs)
                .map_err(err)?
                .into_optimized()
                .map_err(err)?
                .into_runnable()
                .map_err(err)?;
            self.plans.insert((w, h), plan);
        }
        let input = Tensor::from_shape(
            &[1, 3, h as usize, w as usize],
            &crate::prep::nchw_wh(
                img,
                w,
                h,
                crate::prep::IMAGENET_MEAN,
                crate::prep::IMAGENET_STD,
            ),
        )
        .map_err(err)?;
        let outs = self.plans[&(w, h)].run(tvec!(input.into())).map_err(err)?;
        let data: Vec<f32> = outs
            .first()
            .ok_or_else(|| InferError::Model("the depth model gave no output".into()))?
            .to_plain_array_view::<f32>()
            .map_err(err)?
            .iter()
            .copied()
            .collect();
        if data.len() != (w * h) as usize {
            return Err(InferError::Model(format!(
                "expected a {w}×{h} depth, got {} values",
                data.len()
            )));
        }
        Ok(Plane { w, h, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(values: &[f32]) -> Plane {
        Plane {
            w: values.len() as u32,
            h: 1,
            data: values.to_vec(),
        }
    }

    #[test]
    fn input_size_keeps_the_aspect_in_multiples_of_14() {
        let (w, h) = input_size(6240, 4160, 770);
        assert!(w % 14 == 0 && h % 14 == 0, "{w}×{h}");
        assert!(w.abs_diff(770) <= 14, "{w}");
        let aspect = w as f32 / h as f32;
        assert!((aspect / 1.5 - 1.0).abs() < 0.03, "{aspect}");
        assert_eq!(
            input_size(4160, 6240, 770),
            (h, w),
            "a portrait is the transpose"
        );
        assert_eq!(input_size(10, 10, 518), (518, 518));
        assert_eq!(input_size(5000, 1, 518).1, 14, "never below one patch");
    }

    #[test]
    fn normalise_puts_the_nearest_at_zero_and_is_monotone() {
        // Disparity grows left to right: the right end is the nearest.
        let d = row(&(0..100).map(|x| x as f32).collect::<Vec<_>>());
        for scale in [DepthScale::Linear, DepthScale::Equalised] {
            let p = normalise(&d, scale);
            assert!(p.data[0] > 0.95, "{scale:?}: the far end is {}", p.data[0]);
            assert!(
                p.data[99] < 0.05,
                "{scale:?}: the near end is {}",
                p.data[99]
            );
            assert!(
                p.data.windows(2).all(|w| w[1] <= w[0]),
                "{scale:?}: not monotone"
            );
        }
    }

    #[test]
    fn percentiles_hold_out_an_outlier() {
        let mut v: Vec<f32> = (0..99).map(|i| i as f32 / 98.0).collect();
        v.push(1e6);
        let p = normalise(&row(&v), DepthScale::Linear);
        let (lo, hi) = p.data[..99]
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(hi - lo >= 0.9, "the 99 span only {}", hi - lo);
    }

    #[test]
    fn equalised_spreads_a_crowded_far_end() {
        // 90 distant pixels crowded at small disparities, 10 near ones spread out.
        let mut v: Vec<f32> = (0..90).map(|i| i as f32 * 0.001).collect();
        v.extend((0..10).map(|i| 0.5 + i as f32 * 0.05));
        let span = |s: DepthScale| {
            let p = normalise(&row(&v), s);
            let far = &p.data[..90];
            far.iter().cloned().fold(f32::MIN, f32::max)
                - far.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(
            span(DepthScale::Equalised) >= 0.8,
            "{}",
            span(DepthScale::Equalised)
        );
        assert!(
            span(DepthScale::Linear) <= 0.2,
            "{}",
            span(DepthScale::Linear)
        );
    }

    /// The real network — `#[ignore]`, run with `RUSTROOM_AI_MODELS=<dir>` and a landscape photo in
    /// `RUSTROOM_AI_PHOTO`: the foreground (the bottom of the frame) reads nearer than the sky.
    #[test]
    #[ignore]
    fn real_depth_orders_near_before_far() {
        let (Some(dir), Some(photo)) = (
            std::env::var_os("RUSTROOM_AI_MODELS"),
            std::env::var_os("RUSTROOM_AI_PHOTO"),
        ) else {
            return;
        };
        // ⚠ Upright, as the app shows the network every photo: a camera JPEG is often stored sideways
        // with an EXIF rotation, and "the bottom rows" of the stored pixels are then a side of the scene
        // — which is how this test's first run failed against a network that was right.
        use image::ImageDecoder;
        let mut decoder = image::ImageReader::open(photo)
            .unwrap()
            .into_decoder()
            .unwrap();
        let orientation = decoder.orientation().unwrap();
        let mut upright = image::DynamicImage::from_decoder(decoder).unwrap();
        upright.apply_orientation(orientation);
        let img = upright.to_rgb8();
        let file =
            std::path::Path::new(&dir).join(crate::ModelId::DepthAnythingV2Small.spec().file);
        let mut d = Depth::load(&file).unwrap();
        let p = normalise(&d.run(&img, 518).unwrap(), DepthScale::Linear);
        let (w, h) = (p.w as usize, p.h as usize);
        let band = |rows: std::ops::Range<usize>| {
            let n = rows.len() * w;
            rows.flat_map(|y| p.data[y * w..(y + 1) * w].to_vec())
                .sum::<f32>()
                / n as f32
        };
        let (top, bottom) = (band(0..h / 10), band(h - h / 10..h));
        assert!(bottom < top, "bottom {bottom} is not nearer than top {top}");
    }

    /// Review Focus 1: a wall or a sky — one disparity everywhere — and a network that answers NaN.
    #[test]
    fn a_flat_or_broken_plane_normalises_to_finite_values() {
        for scale in [DepthScale::Linear, DepthScale::Equalised] {
            let flat = normalise(&row(&[0.3; 16]), scale);
            assert!(
                flat.data.iter().all(|&v| v == 0.5),
                "{scale:?}: {:?}",
                flat.data
            );
            let broken = normalise(
                &row(&[0.1, f32::NAN, 0.4, f32::INFINITY, 0.9, f32::NEG_INFINITY]),
                scale,
            );
            assert!(
                broken
                    .data
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "{scale:?}: {:?}",
                broken.data
            );
            let empty = normalise(&row(&[f32::NAN; 4]), scale);
            assert!(empty.data.iter().all(|&v| v == 0.5));
        }
    }
}
