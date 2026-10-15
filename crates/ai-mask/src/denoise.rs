//! DRUNet colour (M10.16b): one tile of an image, denoised — the network and nothing around it. The
//! encoding, the noise estimate and the tiling are the app's (`rustroom_app::ai_denoise`).

use crate::infer::InferError;

/// The network's tiles must be a multiple of this on both sides — its three 2 × 2 down-convolutions.
pub const MULTIPLE: usize = 8;

/// Whether a `w × h` tile can be run.
pub fn check_size(w: usize, h: usize) -> Result<(), InferError> {
    if w == 0 || h == 0 || !w.is_multiple_of(MULTIPLE) || !h.is_multiple_of(MULTIPLE) {
        return Err(InferError::Model(format!(
            "a {w} × {h} tile: both sides must be positive multiples of {MULTIPLE}"
        )));
    }
    Ok(())
}

type Plan = std::sync::Arc<tract_onnx::prelude::TypedRunnableModel>;

fn err(e: impl std::fmt::Display) -> InferError {
    InferError::Model(e.to_string())
}

/// DRUNet colour, parsed once, a plan per tile size.
pub struct Drunet {
    model: tract_onnx::prelude::TypedModel,
    plans: std::collections::HashMap<(usize, usize), Plan>,
}

impl Drunet {
    pub fn load(path: &std::path::Path) -> Result<Self, InferError> {
        use tract_onnx::prelude::*;
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(err)?
            .into_typed()
            .map_err(err)?;
        Ok(Self {
            model,
            plans: std::collections::HashMap::new(),
        })
    }

    /// `rgb` (`w × h`, interleaved, display-referred sRGB in 0…1) denoised at noise level `sigma`
    /// (0…255 units) — the same layout back.
    ///
    /// ⚠ The export's own recipe: input `[1, 4, h, w]`, the fourth plane constant `sigma / 255`; one
    /// output `[1, 3, h, w]`. Its symbols are `b`, `h` and `w`, bound per size (`depth.rs`'s pattern).
    pub fn run(
        &mut self,
        rgb: &[f32],
        w: usize,
        h: usize,
        sigma: f32,
    ) -> Result<Vec<f32>, InferError> {
        use tract_onnx::prelude::*;
        check_size(w, h)?;
        if rgb.len() != w * h * 3 {
            return Err(InferError::Model(format!(
                "{} values for a {w} × {h} tile",
                rgb.len()
            )));
        }
        if !self.plans.contains_key(&(w, h)) {
            let subs: std::collections::HashMap<Symbol, TDim> = [
                (self.model.sym("b"), 1.to_dim()),
                (self.model.sym("h"), h.to_dim()),
                (self.model.sym("w"), w.to_dim()),
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
        let n = w * h;
        let mut data = vec![0f32; 4 * n];
        for i in 0..n {
            for c in 0..3 {
                data[c * n + i] = rgb[i * 3 + c];
            }
            data[3 * n + i] = sigma / 255.0;
        }
        let input = Tensor::from_shape(&[1, 4, h, w], &data).map_err(err)?;
        let outs = self.plans[&(w, h)].run(tvec!(input.into())).map_err(err)?;
        let out: Vec<f32> = outs
            .first()
            .ok_or_else(|| InferError::Model("DRUNet gave no output".into()))?
            .to_plain_array_view::<f32>()
            .map_err(err)?
            .iter()
            .copied()
            .collect();
        if out.len() != 3 * n {
            return Err(InferError::Model(format!(
                "expected {} values, got {}",
                3 * n,
                out.len()
            )));
        }
        Ok((0..n)
            .flat_map(|i| [out[i], out[n + i], out[2 * n + i]])
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_must_be_a_multiple_of_eight() {
        assert!(check_size(512, 512).is_ok());
        assert!(check_size(8, 16).is_ok());
        assert!(check_size(510, 512).is_err());
        assert!(check_size(512, 0).is_err());
        assert!(check_size(0, 0).is_err());
    }

    /// The real network on a flat grey with noise: the output's spread falls. Runs only with the model
    /// at `RUSTROOM_DRUNET` (`cargo test --workspace -- --ignored real_drunet`).
    #[test]
    #[ignore]
    fn real_drunet_denoises_a_noisy_flat() {
        let Some(path) = std::env::var_os("RUSTROOM_DRUNET") else {
            eprintln!("SKIPPED: RUSTROOM_DRUNET not set");
            return;
        };
        let mut net = Drunet::load(std::path::Path::new(&path)).expect("model");
        let (w, h) = (64, 48);
        let mut seed = 12345u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let rgb: Vec<f32> = (0..w * h * 3).map(|_| 0.5 + 0.1 * noise()).collect();
        let out = net.run(&rgb, w, h, 25.0).expect("run");
        assert_eq!(out.len(), rgb.len());
        let spread = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
        };
        assert!(
            spread(&out) < spread(&rgb) * 0.5,
            "{} vs {}",
            spread(&out),
            spread(&rgb)
        );
    }
}
