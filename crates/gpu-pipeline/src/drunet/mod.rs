//! DRUNet on the GPU (M10.16b2): the denoise network's 64 convolutions as one tiled product in three
//! modes, run tile by tile for the app's denoise job. The weights come in as plain tensors — this crate
//! never parses ONNX.

mod arch;
mod pack;

pub use arch::{Tensor, Weights};
