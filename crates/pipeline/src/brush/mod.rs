//! Rasterizing a painted mask into a coverage map.

mod frame_map;
mod raster;

pub use frame_map::FrameMap;
pub use raster::{
    plan_dabs, rasterize, rasterize_in, BasePixels, CoverageMap, Dab, DabPlan, Stroke,
};
