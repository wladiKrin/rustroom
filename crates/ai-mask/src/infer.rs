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
    /// ⚠ One decoder per resized size: `tract` needs `orig_im_size` as a **constant fact**, because
    /// the decoder's `Resize` takes its output size from it (M9.13 spike). Building one is ~0.2 s.
    decoders: HashMap<(u32, u32), Plan>,
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
        let (rw, rh) = (e.rw, e.rh);
        if !self.decoders.contains_key(&(rw, rh)) {
            let size = tract_ndarray::arr1(&[rh as f32, rw as f32]).into_tensor();
            let plan = tract_onnx::onnx()
                .model_for_path(&self.decoder_path)
                .map_err(err)?
                .with_input_fact(0, f32::fact([1, 256, 64, 64]).into())
                .map_err(err)?
                .with_input_fact(1, f32::fact([1, 2, 2]).into())
                .map_err(err)?
                .with_input_fact(2, f32::fact([1, 2]).into())
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
            self.decoders.insert((rw, rh), plan);
        }
        let plan = &self.decoders[&(rw, rh)];
        // The point in the resized image's pixels, plus SAM's padding point (0, 0) with label −1.
        let (px, py) = (p[0] * rw as f32, p[1] * rh as f32);
        let coords = tract_ndarray::arr3(&[[[px, py], [0.0, 0.0]]]).into_tensor();
        let labels = tract_ndarray::arr2(&[[1.0f32, -1.0]]).into_tensor();
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
}
