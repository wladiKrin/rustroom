//! Running the networks through `tract`.
//!
//! ⚠ Loading is the slow part (BiRefNet_lite: ~6 s to load and optimise), so a caller keeps a
//! [`Subject`] or a [`Sam`] alive across photos — the app's AI worker does.

use crate::map::ProbMap;
use crate::prep;
use crate::registry::SubjectModel;
use image::RgbImage;
use std::collections::HashMap;
use std::path::Path;
use tract_onnx::prelude::*;

type Plan = std::sync::Arc<TypedRunnableModel>;

#[derive(Debug, thiserror::Error)]
pub enum InferError {
    #[error("model: {0}")]
    Model(String),
}

fn err(e: impl std::fmt::Display) -> InferError {
    InferError::Model(e.to_string())
}

/// The side a subject network is run at.
const SUBJECT_SIDE: u32 = 1024;

/// A loaded subject network — IS-Net or BiRefNet_lite.
pub struct Subject {
    plan: Plan,
    kind: SubjectModel,
}

impl Subject {
    pub fn load(path: &Path, kind: SubjectModel) -> Result<Self, InferError> {
        let s = SUBJECT_SIDE as usize;
        let plan = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(err)?
            .with_input_fact(0, f32::fact([1, 3, s, s]).into())
            .map_err(err)?
            .into_optimized()
            .map_err(err)?
            .into_runnable()
            .map_err(err)?;
        Ok(Self { plan, kind })
    }

    /// The subject's probability over `img` (already upright), at `img`'s long side ≤ 1024.
    ///
    /// ⚠ Each network's own recipe, measured in the spike: IS-Net takes mean 0.5 / std 1.0 and its
    /// **first** of twelve outputs is already a probability (min–max normalised, as rembg does);
    /// BiRefNet takes ImageNet statistics and its **last** output is a logit.
    pub fn run(&self, img: &RgbImage) -> Result<ProbMap, InferError> {
        let (mean, std) = match self.kind {
            SubjectModel::IsNet => ([0.5; 3], [1.0; 3]),
            SubjectModel::BiRefNetLite => (prep::IMAGENET_MEAN, prep::IMAGENET_STD),
        };
        let s = SUBJECT_SIDE as usize;
        let input = Tensor::from_shape(&[1, 3, s, s], &prep::nchw(img, SUBJECT_SIDE, mean, std))
            .map_err(err)?;
        let outs = self.plan.run(tvec!(input.into())).map_err(err)?;
        let pick = match self.kind {
            SubjectModel::IsNet => 0,
            SubjectModel::BiRefNetLite => outs.len() - 1,
        };
        let plane: Vec<f32> = outs[pick]
            .to_plain_array_view::<f32>()
            .map_err(err)?
            .iter()
            .copied()
            .collect();
        if plane.len() != s * s {
            return Err(InferError::Model(format!(
                "expected a {s}² output, got {} values",
                plane.len()
            )));
        }
        let (w, h) = prep::long_side(img.width(), img.height(), SUBJECT_SIDE);
        Ok(match self.kind {
            SubjectModel::IsNet => {
                let (lo, hi) = plane
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
                let span = (hi - lo).max(1e-6);
                prep::square_to_map(&plane, SUBJECT_SIDE, w, h, |v| (v - lo) / span)
            }
            SubjectModel::BiRefNetLite => {
                prep::square_to_map(&plane, SUBJECT_SIDE, w, h, prep::sigmoid)
            }
        })
    }
}

/// A photo's MobileSAM embedding — computed once, then every click decodes against it.
pub struct Embedding {
    tensor: TValue,
    pub rw: u32,
    pub rh: u32,
}

/// MobileSAM's encoder and the SAM ViT-H decoder.
pub struct Sam {
    encoder_path: std::path::PathBuf,
    decoder_path: std::path::PathBuf,
    /// ⚠ One decoder per resized size and point count: `tract` needs `orig_im_size` as a **constant
    /// fact**, because the decoder's `Resize` takes its output size from it (M9.13 spike). Building one
    /// is ~0.2 s.
    decoders: HashMap<(u32, u32, usize), Plan>,
}

impl Sam {
    pub fn new(encoder_path: &Path, decoder_path: &Path) -> Self {
        Self {
            encoder_path: encoder_path.to_path_buf(),
            decoder_path: decoder_path.to_path_buf(),
            decoders: HashMap::new(),
        }
    }

    /// Encode `img` (upright) — ~1.6 s on an M2.
    pub fn embed(&self, img: &RgbImage) -> Result<Embedding, InferError> {
        let (hwc, rw, rh) = prep::hwc_long_side(img);
        let (w, h) = (rw as usize, rh as usize);
        let enc = tract_onnx::onnx()
            .model_for_path(&self.encoder_path)
            .map_err(err)?
            .with_input_fact(0, f32::fact([h, w, 3]).into())
            .map_err(err)?
            .into_optimized()
            .map_err(err)?
            .into_runnable()
            .map_err(err)?;
        let input = Tensor::from_shape(&[h, w, 3], &hwc).map_err(err)?;
        let tensor = enc.run(tvec!(input.into())).map_err(err)?.remove(0);
        Ok(Embedding { tensor, rw, rh })
    }

    /// The region at normalised point `p` of the embedded photo — ~0.1 s.
    pub fn click(&mut self, e: &Embedding, p: [f32; 2]) -> Result<ProbMap, InferError> {
        self.prompt(e, &[p], None)
    }

    /// The region asked for by `points` (each "in it", normalised) and an optional `bbox` around it
    /// (`[x0, y0, x1, y1]`, normalised) — M10.16c1's Detect objects; a click is one point and no box.
    ///
    /// ⚠ SAM's ONNX convention: points carry label 1, a box its two corners labels 2 and 3; without a
    /// box one padding point (0, 0) with label −1 closes the list. The decoder is built per point count.
    pub fn prompt(
        &mut self,
        e: &Embedding,
        points: &[[f32; 2]],
        bbox: Option<[f32; 4]>,
    ) -> Result<ProbMap, InferError> {
        if points.is_empty() && bbox.is_none() {
            return Err(InferError::Model(
                "a prompt with no point and no box".into(),
            ));
        }
        let (rw, rh) = (e.rw, e.rh);
        let (sx, sy) = (rw as f32, rh as f32);
        let mut coords: Vec<f32> = points.iter().flat_map(|p| [p[0] * sx, p[1] * sy]).collect();
        let mut labels = vec![1.0f32; points.len()];
        match bbox {
            Some([x0, y0, x1, y1]) => {
                coords.extend([x0 * sx, y0 * sy, x1 * sx, y1 * sy]);
                labels.extend([2.0, 3.0]);
            }
            None => {
                coords.extend([0.0, 0.0]);
                labels.push(-1.0);
            }
        }
        let n = labels.len();
        let key = (rw, rh, n);
        if !self.decoders.contains_key(&key) {
            let size = tract_ndarray::arr1(&[rh as f32, rw as f32]).into_tensor();
            let plan = tract_onnx::onnx()
                .model_for_path(&self.decoder_path)
                .map_err(err)?
                .with_input_fact(0, f32::fact([1, 256, 64, 64]).into())
                .map_err(err)?
                .with_input_fact(1, f32::fact([1, n, 2]).into())
                .map_err(err)?
                .with_input_fact(2, f32::fact([1, n]).into())
                .map_err(err)?
                .with_input_fact(3, f32::fact([1, 1, 256, 256]).into())
                .map_err(err)?
                .with_input_fact(4, f32::fact([1]).into())
                .map_err(err)?
                .with_input_fact(5, size.into())
                .map_err(err)?
                .into_optimized()
                .map_err(err)?
                .into_runnable()
                .map_err(err)?;
            self.decoders.insert(key, plan);
        }
        let plan = &self.decoders[&key];
        let coords = Tensor::from_shape(&[1, n, 2], &coords).map_err(err)?;
        let labels = Tensor::from_shape(&[1, n], &labels).map_err(err)?;
        let mask_in = tract_ndarray::Array4::<f32>::zeros((1, 1, 256, 256)).into_tensor();
        let has = tract_ndarray::arr1(&[0.0f32]).into_tensor();
        let size = tract_ndarray::arr1(&[rh as f32, rw as f32]).into_tensor();
        let outs = plan
            .run(tvec!(
                e.tensor.clone(),
                coords.into(),
                labels.into(),
                mask_in.into(),
                has.into(),
                size.into()
            ))
            .map_err(err)?;
        // ⚠ Output 2, `low_res_masks` — see `prep::sam_low_res_to_map` for why not output 0.
        let low: Vec<f32> = outs
            .get(2)
            .ok_or_else(|| InferError::Model("the decoder has no low_res_masks".into()))?
            .to_plain_array_view::<f32>()
            .map_err(err)?
            .iter()
            .copied()
            .collect();
        if low.len() != 256 * 256 {
            return Err(InferError::Model(format!(
                "low_res_masks has {} values",
                low.len()
            )));
        }
        Ok(prep::sam_low_res_to_map(&low, rw, rh))
    }
}

/// Real-model checks — `#[ignore]`, run with `RUSTROOM_AI_MODELS=<dir> cargo test --workspace
/// --release -- --ignored real_` and a photo in `RUSTROOM_AI_PHOTO`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ModelId;

    fn models() -> Option<(std::path::PathBuf, RgbImage)> {
        let dir = std::path::PathBuf::from(std::env::var("RUSTROOM_AI_MODELS").ok()?);
        let photo = std::env::var("RUSTROOM_AI_PHOTO").ok()?;
        Some((dir, image::open(photo).ok()?.to_rgb8()))
    }

    #[test]
    #[ignore]
    fn real_isnet_finds_a_subject() {
        let Some((dir, img)) = models() else { return };
        let s = Subject::load(&dir.join(ModelId::IsNet.spec().file), SubjectModel::IsNet).unwrap();
        let m = s.run(&img).unwrap();
        let covered = m.data.iter().filter(|&&v| v > 128).count() as f32 / m.data.len() as f32;
        assert!(covered > 0.01 && covered < 0.9, "subject covers {covered}");
    }

    #[test]
    #[ignore]
    fn real_sam_click_selects_a_region() {
        let Some((dir, img)) = models() else { return };
        let mut sam = Sam::new(
            &dir.join(ModelId::SamEncoder.spec().file),
            &dir.join(ModelId::SamDecoder.spec().file),
        );
        let e = sam.embed(&img).unwrap();
        let m = sam.click(&e, [0.5, 0.5]).unwrap();
        assert!(
            m.at(0.5, 0.5) > 0.5,
            "the clicked point is not in its own region"
        );
    }

    /// M10.16c1 (Detect objects): a bright square on a dark ground, asked for by a box around it and
    /// two points inside — the square's coverage high, the ground's low.
    #[test]
    #[ignore]
    fn real_sam_box_prompt_returns_the_object() {
        let Some(dir) = std::env::var("RUSTROOM_AI_MODELS").ok() else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let inside = |x: u32, y: u32| (300..500).contains(&x) && (200..380).contains(&y);
        let img = RgbImage::from_fn(800, 600, |x, y| {
            if inside(x, y) {
                image::Rgb([230, 200, 60])
            } else {
                image::Rgb([30, 40, 50])
            }
        });
        let mut sam = Sam::new(
            &dir.join(ModelId::SamEncoder.spec().file),
            &dir.join(ModelId::SamDecoder.spec().file),
        );
        let e = sam.embed(&img).unwrap();
        let (w, h) = (800.0, 600.0);
        let m = sam
            .prompt(
                &e,
                &[[350.0 / w, 290.0 / h], [450.0 / w, 290.0 / h]],
                Some([280.0 / w, 180.0 / h, 520.0 / w, 400.0 / h]),
            )
            .unwrap();
        let (mut cin, mut nin, mut cout, mut nout) = (0.0, 0, 0.0, 0);
        for y in (0..600).step_by(4) {
            for x in (0..800).step_by(4) {
                let p = m.at((x as f32 + 0.5) / w, (y as f32 + 0.5) / h);
                // A 12-px band around the edge is left out: the map is SAM's 256² low-res answer.
                let near = (288..512).contains(&x) && (188..392).contains(&y);
                let deep = (312..488).contains(&x) && (212..368).contains(&y);
                if inside(x, y) && deep {
                    cin += p;
                    nin += 1;
                } else if !near {
                    cout += p;
                    nout += 1;
                }
            }
        }
        let (cin, cout) = (cin / nin as f32, cout / nout as f32);
        assert!(nin > 1000 && nout > 10_000, "{nin} inside, {nout} outside");
        assert!(cin > 0.8 && cout < 0.1, "inside {cin}, outside {cout}");
    }
}
