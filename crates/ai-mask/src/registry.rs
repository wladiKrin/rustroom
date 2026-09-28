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
}

impl ModelId {
    pub const ALL: [ModelId; 4] = [
        Self::IsNet,
        Self::BiRefNetLite,
        Self::SamEncoder,
        Self::SamDecoder,
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

    #[test]
    fn every_model_is_pinned_and_licensed() {
        for id in ModelId::ALL {
            let s = id.spec();
            assert!(s.url.starts_with("https://github.com/"), "{id:?}");
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
