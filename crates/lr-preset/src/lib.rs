//! Lightroom develop presets read into Rustroom edits (M10.9c): `.xmp` (Lightroom Classic 7.3+,
//! Lightroom, Camera Raw), the older `.lrtemplate`, and `.zip` packs of either.
//!
//! ⚠ No GUI. Two readers produce one [`Settings`] map; the mapping onto `edit_model::Edits` is one
//! table. The look will not match Lightroom exactly — the processing differs — and what has no
//! counterpart here is named, never silently dropped.

pub mod lrtemplate;
pub mod map;
mod value;
pub mod xmp;
pub mod zip;

pub use value::{Error, Raw, Settings, Value};
