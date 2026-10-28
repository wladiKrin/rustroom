//! LaMa as the panorama's edge filler (M10.17b): `photo_merge::fill`'s [`Inpainter`] over M10.16c1's network.
//!
//! The merge's pixels are the camera's normalised raw signal; the network wants display-encoded RGB. So a
//! window goes raw → the reference's white balance and camera matrix (linear ProPhoto) → the Remove fill's
//! peak-channel encoding (`remove_tool::peak_encoding` — a sky's blue clipped under luminance, M10.16c1) →
//! the network → back the same way.

use crate::removal;
use core_image::ImageBuf;
use photo_merge::fill::Inpainter;
use raw_decode::CameraFrame;
use std::path::Path;

/// LaMa's file is downloaded into `models`.
pub fn present(models: &Path) -> bool {
    models.join(removal::MODEL.spec().file).is_file()
}

/// LaMa with the camera's conversions.
pub struct LamaFill {
    net: removal::Net,
    pre_mul: [f32; 3],
    to_prophoto: [[f32; 3]; 3],
    from_prophoto: [[f32; 3]; 3],
}

impl LamaFill {
    /// `net` for frames of `meta`'s camera.
    pub fn new(net: removal::Net, meta: &CameraFrame) -> Self {
        let to_prophoto = color::camera::camera_to_prophoto(meta.rgb_cam);
        Self {
            net,
            pre_mul: meta.pre_mul,
            to_prophoto,
            from_prophoto: color::camera::invert(&to_prophoto).unwrap_or(to_prophoto),
        }
    }
}

impl Inpainter for LamaFill {
    fn side(&self) -> usize {
        crate::remove_tool::SIDE
    }

    fn fill(&mut self, window: &ImageBuf, hole: &[f32]) -> Result<ImageBuf, String> {
        let k = self.pre_mul;
        let pp = window.map_pixels(|p| {
            color::spaces::apply(&self.to_prophoto, [p[0] * k[0], p[1] * k[1], p[2] * k[2]])
        });
        let enc = crate::remove_tool::peak_encoding(&pp);
        let rgb: Vec<f32> = pp
            .data()
            .chunks(3)
            .flat_map(|p| enc.encode([p[0], p[1], p[2]]))
            .collect();
        let out = (self.net)(&rgb, hole)?;
        if out.len() != rgb.len() || out.iter().any(|v| !v.is_finite()) {
            return Err(format!("the network answered {} values", out.len()));
        }
        let data: Vec<f32> = out
            .chunks(3)
            .flat_map(|e| {
                let q = color::spaces::apply(&self.from_prophoto, enc.decode([e[0], e[1], e[2]]));
                [q[0] / k[0], q[1] / k[1], q[2] / k[2]]
            })
            .collect();
        Ok(ImageBuf::from_vec(window.width(), window.height(), data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> CameraFrame {
        let dir = test_dir::path("pano-lama");
        let p = dir.join("one.dng");
        export::dng::save(&ImageBuf::from_vec(300, 200, vec![0.2; 300 * 200 * 3]), &p).unwrap();
        raw_decode::decode_camera(&p, &raw_decode::DecodeOptions::linear_scene()).unwrap()
    }

    /// The conversions round-trip: a network that returns its input gives the window back.
    #[test]
    fn an_identity_network_returns_the_window() {
        let side = crate::remove_tool::SIDE;
        let mut f = LamaFill::new(Box::new(|rgb: &[f32], _: &[f32]| Ok(rgb.to_vec())), &meta());
        // Near-neutral pixels: a saturated one falls outside sRGB, and the encoding clamps it (as Remove's does).
        let window = ImageBuf::from_vec(
            side,
            side,
            (0..side * side)
                .flat_map(|i| {
                    let v = 0.05 + 0.3 * ((i % 7) as f32 / 7.0);
                    [v * 1.05, v, v * 0.95]
                })
                .collect(),
        );
        let out = f.fill(&window, &vec![0.0; side * side]).unwrap();
        let worst = window
            .data()
            .iter()
            .zip(out.data())
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(worst < 1e-3, "{worst}");
    }

    #[test]
    fn a_network_error_is_passed_on() {
        let side = crate::remove_tool::SIDE;
        let mut f = LamaFill::new(
            Box::new(|_: &[f32], _: &[f32]| Err("no LaMa here".to_string())),
            &meta(),
        );
        let r = f.fill(&ImageBuf::new(side, side), &vec![1.0; side * side]);
        assert_eq!(r.err().as_deref(), Some("no LaMa here"));
    }

    #[test]
    fn an_empty_models_folder_has_no_lama() {
        let dir = test_dir::path("pano-lama-empty");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!present(&dir));
    }

    /// The real network on a panorama's edge — `RUSTROOM_AI_MODELS` (the folder LaMa is downloaded into).
    #[test]
    fn lama_fills_a_panorama_edge() {
        let Some(models) = std::env::var_os("RUSTROOM_AI_MODELS")
            .map(std::path::PathBuf::from)
            .filter(|m| present(m))
        else {
            eprintln!("skipped: RUSTROOM_AI_MODELS has no LaMa");
            return;
        };
        let side = crate::remove_tool::SIDE;
        let mut f = LamaFill::new(removal::lama(&models).expect("LaMa loads"), &meta());
        let window = ImageBuf::from_vec(
            side,
            side,
            (0..side * side * 3)
                .map(|i| 0.1 + 0.05 * (((i / 3) % side) as f32 * 0.1).sin())
                .collect(),
        );
        let hole: Vec<f32> = (0..side * side)
            .map(|i| ((i / side) < side / 4) as u8 as f32)
            .collect();
        let out = f.fill(&window, &hole).unwrap();
        let p99 = 0.15;
        assert!(
            out.data().iter().all(|v| v.is_finite() && *v <= 4.0 * p99),
            "a fill out of range"
        );
    }
}
