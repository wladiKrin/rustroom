//! Rasterizing a painted mask into a coverage map.

mod frame_map;
pub mod incremental;
mod raster;
mod spot_map;

pub use frame_map::FrameMap;
pub use raster::{
    plan_dabs, rasterize, rasterize_in, BasePixels, BaseRef, CoverageMap, Dab, DabPlan, Stroke,
};
pub use spot_map::SpotMap;
