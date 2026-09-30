//! Background render worker: renders `Edits` against a cached linear editing buffer,
//! off the UI thread, coalescing superseded requests.

mod brush_cache;
mod engine;
#[cfg(test)]
pub(crate) mod fixtures;
mod frame;
mod pack;
mod prefix;
mod proof;
mod render;
mod scale;
mod scoped;
mod tail;

pub use engine::RenderEngine;
pub use frame::{FrameSpec, Proof, Ran, RenderedFrame};
pub use pack::Packed;
pub use render::render_once;
pub use scale::scale_from_full;

/// A requested crop of the rendered image, in post-geometry pixels.
///
/// ⚠⚠ **This was `RegionRect`, a struct identical to `pipeline::Rect` in everything but its name.**
/// Two types that cannot disagree because they are the same type twice is still two places to
/// change, and M8.7 would otherwise have added a third. The alias is kept so the ~40 call sites in
/// this crate keep reading as "a region", which is what they are about.
pub use pipeline::Rect as RegionRect;
