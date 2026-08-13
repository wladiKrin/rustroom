//! Rustroom GUI logic (no egui here — unit-testable units live in this lib).

pub mod action_log;
pub mod batch_export;
pub mod buffer_cache;
pub mod crop_tool;
pub mod decode_worker;
pub mod edit_clipboard;
pub mod export_presets;
pub mod format_filter;
pub mod full_res;
pub mod histogram;
pub mod history;
pub mod presets;
pub mod render_engine;
pub mod safe_name;
pub mod selection;
pub mod settings;
pub mod sidecar;
pub mod sync;
pub mod thumb_loader;
pub mod view_region;
