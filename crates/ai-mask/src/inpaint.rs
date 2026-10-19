//! LaMa (M10.16c1): a 512 × 512 square with a hole, the hole filled from its surroundings — the network
//! and nothing around it. The window, the mask and the paste are the app's (`rustroom_app::remove_tool`).

use crate::infer::InferError;
use tract_onnx::prelude::*;

/// The export's fixed side — its Fourier layers were rewritten for one size.
pub const SIDE: usize = 512;

type Plan = std::sync::Arc<TypedRunnableModel>;

fn err(e: impl std::fmt::Display) -> InferError {
    InferError::Model(e.to_string())
}

/// LaMa, loaded and optimised once (~7 s) — a caller keeps it across removals.
pub struct Inpaint {
    plan: Plan,
}

impl Inpaint {
    pub fn load(path: &std::path::Path) -> Result<Self, InferError> {
        let plan = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(err)?
            .with_input_fact(0, f32::fact([1, 3, SIDE, SIDE]).into())
            .map_err(err)?
            .with_input_fact(1, f32::fact([1, 1, SIDE, SIDE]).into())
            .map_err(err)?
            .into_optimized()
            .map_err(err)?
            .into_runnable()
            .map_err(err)?;
        Ok(Self { plan })
    }

    /// `rgb` (`SIDE²`, interleaved, display-referred 0…1) with the pixels where `mask` ≥ 0.5 removed and
    /// filled — the same layout back, 0…1, the whole square (the caller pastes only the hole).
    ///
    /// ⚠ The export's recipe, measured in the spike: `image [1, 3, 512, 512]` in 0…1 and `mask
    /// [1, 1, 512, 512]` with **1 = remove**; its one output is in **0…255**. The hole is blanked here
    /// too, so nothing of the object reaches the network whatever the export does inside.
    pub fn run(&mut self, rgb: &[f32], mask: &[f32]) -> Result<Vec<f32>, InferError> {
        let n = SIDE * SIDE;
        if rgb.len() != n * 3 || mask.len() != n {
            return Err(InferError::Model(format!(
                "expected {SIDE}² pixels, got {} rgb values and {} mask values",
                rgb.len(),
                mask.len()
            )));
        }
        let hole: Vec<f32> = mask.iter().map(|&m| (m >= 0.5) as u8 as f32).collect();
        let mut planar = vec![0f32; n * 3];
        for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
            let keep = 1.0 - hole[i];
            for c in 0..3 {
                planar[c * n + i] = px[c] * keep;
            }
        }
        let image = Tensor::from_shape(&[1, 3, SIDE, SIDE], &planar).map_err(err)?;
        let mask = Tensor::from_shape(&[1, 1, SIDE, SIDE], &hole).map_err(err)?;
        let out = self
            .plan
            .run(tvec!(image.into(), mask.into()))
            .map_err(err)?;
        let o = out[0].to_plain_array_view::<f32>().map_err(err)?;
        let o = o.as_slice().ok_or_else(|| err("a non-contiguous output"))?;
        if o.len() != n * 3 {
            return Err(InferError::Model(format!(
                "expected a 3 × {SIDE}² output, got {} values",
                o.len()
            )));
        }
        if let Some(bad) = o.iter().find(|v| !v.is_finite()) {
            return Err(InferError::Model(format!("a non-finite output ({bad})")));
        }
        Ok((0..n)
            .flat_map(|i| (0..3).map(move |c| o[c * n + i] / 255.0))
            .collect())
    }
}

/// Real-model check — `#[ignore]`, run with `RUSTROOM_AI_MODELS=<dir> cargo test --workspace --release
/// -- --ignored real_inpaint`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ModelId;

    /// Two smooth ramps crossing a 100 × 140 hole: the fill must land far closer to the hidden picture
    /// than the hole's flat mean would — the network reads its surroundings.
    #[test]
    #[ignore]
    fn real_inpaint_fills_a_hole_from_its_surroundings() {
        let Some(dir) = std::env::var("RUSTROOM_AI_MODELS").ok() else {
            return;
        };
        let path = std::path::Path::new(&dir).join(ModelId::Lama.spec().file);
        let mut net = Inpaint::load(&path).expect("LaMa loads");
        let truth: Vec<f32> = (0..SIDE * SIDE)
            .flat_map(|i| {
                let (x, y) = ((i % SIDE) as f32, (i / SIDE) as f32);
                let (u, v) = (((x - 250.0) / 60.0).tanh(), ((y - 250.0) / 80.0).tanh());
                [0.5 + 0.4 * u, 0.5 + 0.4 * v, 0.5 + 0.2 * (u - v)]
            })
            .collect();
        let in_hole =
            |i: usize| (200..300).contains(&(i % SIDE)) && (180..320).contains(&(i / SIDE));
        let mask: Vec<f32> = (0..SIDE * SIDE).map(|i| in_hole(i) as u8 as f32).collect();
        // The object: a saturated block the network must not reproduce.
        let mut rgb = truth.clone();
        for i in (0..SIDE * SIDE).filter(|&i| in_hole(i)) {
            rgb[i * 3..i * 3 + 3].copy_from_slice(&[1.0, 0.0, 1.0]);
        }
        let out = net.run(&rgb, &mask).expect("runs");
        let hole: Vec<usize> = (0..SIDE * SIDE).filter(|&i| in_hole(i)).collect();
        assert!(hole.len() > 10_000, "the hole holds {} pixels", hole.len());
        let mean: Vec<f32> = (0..3)
            .map(|c| hole.iter().map(|&i| truth[i * 3 + c]).sum::<f32>() / hole.len() as f32)
            .collect();
        let rms = |f: &dyn Fn(usize, usize) -> f32| {
            (hole
                .iter()
                .flat_map(|&i| (0..3).map(move |c| (i, c)))
                .map(|(i, c)| (f(i, c) - truth[i * 3 + c]).powi(2))
                .sum::<f32>()
                / (hole.len() * 3) as f32)
                .sqrt()
        };
        let fill = rms(&|i, c| out[i * 3 + c]);
        let flat = rms(&|_, c| mean[c]);
        assert!(
            flat > 0.05,
            "the flat mean's error {flat} — the gradient is too gentle to judge"
        );
        let outside: Vec<usize> = (0..SIDE * SIDE).filter(|&i| !in_hole(i)).collect();
        let kept = (outside
            .iter()
            .flat_map(|&i| (0..3).map(move |c| (i, c)))
            .map(|(i, c)| (out[i * 3 + c] - truth[i * 3 + c]).powi(2))
            .sum::<f32>()
            / (outside.len() * 3) as f32)
            .sqrt();
        // ⚠ Measured 2026-10-19: the fill 0.037 against the flat mean's 0.158 (4.3×) — a network trained
        // on photos, asked for a synthetic ramp. The plan's 5× was invented; 3× is the bar below the
        // measurement, and swapping the mask's polarity lands at the magenta block (≫ the mean).
        assert!(
            fill * 3.0 < flat,
            "the fill's error {fill} vs the flat mean's {flat}"
        );
        // The export composites the known pixels back itself — measured 3.6e-9; the 0…255 scale with it.
        assert!(kept < 1e-4, "outside the hole the output moved by {kept}");
    }
}
