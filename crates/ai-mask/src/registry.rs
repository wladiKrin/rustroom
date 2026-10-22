//! Every model file Rustroom may download, pinned by size and SHA-256.
//!
//! ⚠ Hashes were measured from the files downloaded for the M9.13 spike and copied verbatim — never
//! retype one. A file whose hash does not match is deleted, not used.

use serde::{Deserialize, Serialize};

/// One downloadable model file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// The name it is stored under in the models directory.
    pub file: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    /// Lower-case hex.
    pub sha256: &'static str,
    /// Shown in the download dialog before anything is fetched.
    pub licence: &'static str,
    pub licence_url: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelId {
    IsNet,
    BiRefNetLite,
    SamEncoder,
    SamDecoder,
    /// M10.16a — the Depth range mask's network.
    DepthAnythingV2Small,
    /// M10.16b — AI denoise's network.
    DrunetColor,
    /// M10.16b6 — AI denoise's default network, ten times lighter than DRUNet.
    FfdnetColor,
    /// M10.16c1 — the Remove brush's inpainting network.
    Lama,
    /// M10.16c3 — Find people's detector.
    YoloxM,
}

impl ModelId {
    pub const ALL: [ModelId; 9] = [
        Self::IsNet,
        Self::BiRefNetLite,
        Self::SamEncoder,
        Self::SamDecoder,
        Self::DepthAnythingV2Small,
        Self::DrunetColor,
        Self::FfdnetColor,
        Self::Lama,
        Self::YoloxM,
    ];

    pub const fn spec(self) -> ModelSpec {
        match self {
            // ⚠ The weights carry no explicit licence: the DIS repo licenses its CODE Apache-2.0 and
            // points to separate DIS5K dataset terms. Offered by the author's choice, having been told.
            Self::IsNet => ModelSpec {
                file: "isnet-general-use.onnx",
                url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx",
                bytes: 178_648_008,
                sha256: "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
                licence: "Apache-2.0 (code); weights: see the DIS repository",
                licence_url: "https://github.com/xuebinqin/DIS",
            },
            Self::BiRefNetLite => ModelSpec {
                file: "BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx",
                url: "https://github.com/ZhengPeng7/BiRefNet/releases/download/v1/BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx",
                bytes: 224_005_088,
                sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
                licence: "MIT (code and weights)",
                licence_url: "https://github.com/ZhengPeng7/BiRefNet",
            },
            Self::SamEncoder => ModelSpec {
                file: "mobile_sam.encoder.onnx",
                url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/mobile_sam.encoder.onnx",
                bytes: 28_157_098,
                sha256: "413d0cde1c77c02405cd82a8e12c15f22fbf8e844c0fb3ef927cf04df9f586e5",
                licence: "Apache-2.0 (MobileSAM)",
                licence_url: "https://github.com/ChaoningZhang/MobileSAM",
            },
            Self::SamDecoder => ModelSpec {
                file: "sam_vit_h_4b8939.decoder.onnx",
                url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/sam_vit_h_4b8939.decoder.onnx",
                bytes: 16_500_630,
                sha256: "43b5e1bc840d97ff4a08db991a9cd6cc33831c0bf2c70c0a48f5eba891ec9a46",
                licence: "Apache-2.0 (Segment Anything)",
                licence_url: "https://github.com/facebookresearch/segment-anything",
            },
            // ⚠ M10.16a: the hash and size measured from the file downloaded 2026-10-14 (`shasum -a 256`,
            // `stat`), copied verbatim. The licence verified at the source: the official README gives
            // Small Apache-2.0 (Base/Large/Giant are CC-BY-NC-4.0 — never offer them); the ONNX export's
            // repository is Apache-2.0. The DYNAMIC export, so any input size multiple of 14 runs.
            Self::DepthAnythingV2Small => ModelSpec {
                file: "depth_anything_v2_vits_dynamic.onnx",
                url: "https://github.com/fabio-sim/Depth-Anything-ONNX/releases/download/v2.0.0/depth_anything_v2_vits_dynamic.onnx",
                bytes: 99_092_268,
                sha256: "46c4e8eeda3a27f34701831b6a2ec7753d7b38779b215acb5633424703deed8f",
                licence: "Apache-2.0 (Depth Anything V2 Small, code and weights)",
                licence_url: "https://github.com/DepthAnything/Depth-Anything-V2",
            },
            // ⚠ M10.16b: the hash and size measured from the file downloaded 2026-10-15. The weights
            // are KAIR v1.0's unmodified `drunet_color.pth`, MIT; ⚠ its training data included DIV2K,
            // whose own terms say research only, and whether that reaches the weights is unsettled —
            // said in the dialog, as IS-Net's missing licence is.
            Self::DrunetColor => ModelSpec {
                file: "drunet_color.onnx",
                url: "https://huggingface.co/synthscript/drunet-color-onnx/resolve/main/drunet_color.onnx",
                bytes: 130_589_232,
                sha256: "2ae3ab5eb15daac2ee79be984d584b908ce7f0f60b27be87d005f728c2aa0087",
                licence: "MIT (DRUNet weights, KAIR) — trained partly on DIV2K, whose terms say research use only",
                licence_url: "https://github.com/cszn/DPIR",
            },
            // ⚠ PyTorch's legacy format, read by `pth` — a closed pickle subset that runs nothing from it.
            // Trained partly on ImageNet images, whose terms say research only — DRUNet's DIV2K note again.
            Self::FfdnetColor => ModelSpec {
                file: "ffdnet_color.pth",
                url: "https://github.com/cszn/KAIR/releases/download/v1.0/ffdnet_color.pth",
                bytes: 3_411_551,
                sha256: "286f04f7e3a20254c067ba95efb5d3339f4fc8a09a365a93cd469184d4cdb279",
                licence: "MIT (FFDNet weights, KAIR) — trained partly on ImageNet images, whose terms say research use only",
                licence_url: "https://github.com/cszn/KAIR",
            },
            // ⚠ M10.16c1: the hash and size measured from the file downloaded 2026-10-19 for the spike,
            // copied verbatim. Big LaMa exported to ONNX at a FIXED 512 × 512 (its Fourier layers
            // rewritten for the export); the export and the original are Apache-2.0. Picked by the
            // author over MI-GAN from the spike's sheets (better on 9 of 11 objects, ~1.5 s slower).
            Self::Lama => ModelSpec {
                file: "lama_fp32.onnx",
                url: "https://huggingface.co/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx",
                bytes: 208_044_816,
                sha256: "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6",
                licence: "Apache-2.0 (LaMa, code and weights; the ONNX export by Carve)",
                licence_url: "https://github.com/advimman/lama",
            },
            // ⚠ M10.16c3: the size and hash measured from the file downloaded 2026-10-21 for the spike, copied
            // verbatim. Megvii's own ONNX export (release 0.1.1rc0); code and weights Apache-2.0. Picked by the
            // author over YOLOX-s from the spike's sheets, with the tiled pass.
            Self::YoloxM => ModelSpec {
                file: "yolox_m.onnx",
                url: "https://github.com/Megvii-BaseDetection/YOLOX/releases/download/0.1.1rc0/yolox_m.onnx",
                bytes: 101_259_744,
                sha256: "21ff6cfdeb53b013bac2249599e55f00bff3cfdfdab37ed7a4620818c1d15b3f",
                licence: "Apache-2.0 (YOLOX, code and weights)",
                licence_url: "https://github.com/Megvii-BaseDetection/YOLOX",
            },
        }
    }
}

/// Which network *Select Subject* uses — the author's choice to offer both (M9.13 spike).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SubjectModel {
    /// ~1.9 s a photo on an M2: the fast default.
    #[default]
    IsNet,
    /// ~24 s a photo on an M2, the crisper mask and the cleaner licence.
    BiRefNetLite,
}

impl SubjectModel {
    pub const ALL: [SubjectModel; 2] = [Self::IsNet, Self::BiRefNetLite];

    pub fn model(self) -> ModelId {
        match self {
            Self::IsNet => ModelId::IsNet,
            Self::BiRefNetLite => ModelId::BiRefNetLite,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::IsNet => "IS-Net (fast)",
            Self::BiRefNetLite => "BiRefNet lite (high quality)",
        }
    }
}

/// The files *Select by click* needs.
pub const CLICK_MODELS: [ModelId; 2] = [ModelId::SamEncoder, ModelId::SamDecoder];

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ `ALL` is a hand-written list — the shape *"a guard that enumerates a list stops guarding the
    /// moment the list grows"*: `every_model_is_pinned_and_licensed` sweeps `ALL`, so a model missing
    /// from it would ship unpinned and unswept. The exhaustive `match` makes a new variant a compile
    /// error here, and the loop makes leaving it out of `ALL` a failure.
    #[test]
    fn every_variant_is_in_all() {
        let index = |id: ModelId| match id {
            ModelId::IsNet => 0,
            ModelId::BiRefNetLite => 1,
            ModelId::SamEncoder => 2,
            ModelId::SamDecoder => 3,
            ModelId::DepthAnythingV2Small => 4,
            ModelId::DrunetColor => 5,
            ModelId::FfdnetColor => 6,
            ModelId::Lama => 7,
            ModelId::YoloxM => 8,
        };
        let every = [
            ModelId::IsNet,
            ModelId::BiRefNetLite,
            ModelId::SamEncoder,
            ModelId::SamDecoder,
            ModelId::DepthAnythingV2Small,
            ModelId::DrunetColor,
            ModelId::FfdnetColor,
            ModelId::Lama,
            ModelId::YoloxM,
        ];
        for id in every {
            assert_eq!(
                ModelId::ALL.get(index(id)),
                Some(&id),
                "{id:?} is not in ALL"
            );
        }
    }

    /// M10.16b: DRUNet's weights are MIT, but its training data included DIV2K, whose own terms say
    /// research only — the download dialog must say so before anything is fetched.
    #[test]
    fn the_denoise_model_carries_its_licence_caveat() {
        let s = ModelId::DrunetColor.spec();
        assert_eq!(s.bytes, 130_589_232);
        assert_eq!(
            s.sha256,
            "2ae3ab5eb15daac2ee79be984d584b908ce7f0f60b27be87d005f728c2aa0087"
        );
        assert!(
            s.licence.contains("MIT") && s.licence.contains("DIV2K"),
            "{}",
            s.licence
        );
    }

    /// M10.16c1: LaMa as downloaded for the spike, 2026-10-19 — size and hash measured, never retyped.
    #[test]
    fn the_registry_pins_the_inpainting_model() {
        let s = ModelId::Lama.spec();
        assert_eq!(s.bytes, 208_044_816);
        assert_eq!(
            s.sha256,
            "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6"
        );
        assert!(s.licence.contains("Apache-2.0"), "{}", s.licence);
    }

    #[test]
    fn every_model_is_pinned_and_licensed() {
        for id in ModelId::ALL {
            let s = id.spec();
            // ⚠ M10.16b: Hugging Face beside GitHub — DRUNet's only ONNX export lives there. Both are
            // HTTPS and both redirect to a CDN, which the downloader follows; the integrity is the
            // pinned SHA-256, never the host.
            assert!(
                ["https://github.com/", "https://huggingface.co/"]
                    .iter()
                    .any(|h| s.url.starts_with(h)),
                "{id:?}: {}",
                s.url
            );
            assert!(
                s.url.ends_with(s.file),
                "{id:?}: the URL names another file"
            );
            assert_eq!(s.sha256.len(), 64, "{id:?}");
            assert!(s
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
            assert!(s.bytes > 1_000_000 && !s.licence.is_empty() && !s.licence_url.is_empty());
        }
        let files: std::collections::HashSet<_> =
            ModelId::ALL.iter().map(|m| m.spec().file).collect();
        assert_eq!(
            files.len(),
            ModelId::ALL.len(),
            "two models share a file name"
        );
    }
}
