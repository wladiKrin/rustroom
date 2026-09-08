//! Rasterizing a painted mask into a coverage map.

mod frame_map;
mod raster;

pub use frame_map::FrameMap;
pub use raster::{rasterize, rasterize_in, BasePixels, CoverageMap};
