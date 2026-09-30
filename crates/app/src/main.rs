//! The application's state and its bootstrap — and **nothing else**.
//!
//! ⚠⚠ `main.rs` reached 4,375 lines one reasonable-seeming addition at a time. The rule that
//! replaced it: **this file holds the type definitions, `RustroomApp::new` and `fn main`.**
//! Behaviour lives in a topic module beside it. An inherent impl only has to be in the same
//! *crate*, not the same module, so a method moves with `self` staying `self` and no call site
//! changes — which is why there is no excuse for adding one here.
//!
//! | Where | What |
//! |---|---|
//! | [`session_flow`] | opening a photo, the decode handover, the full-resolution upgrade, the per-frame pumps |
//! | [`sources`] | folders, All Photos, collections, keywords |
//! | [`smart_sources`] | the two query-driven sources: dates and smart collections |
//! | [`library_state`] | the loaded library's derived state, sorting and rebuilds |
//! | [`catalog_ops`] | keywords, collection membership, marks, rules, restoring a deletion |
//! | [`copies`] / [`snapshots`] | virtual copies and naming / named saved states |
//! | [`transfer`] | copy/paste, presets and sync |
//! | [`export_flow`] | driving the export worker |
//! | [`undo`] | walking the action log |
//! | [`settings_apply`] | applying a settings change to the running app |
//! | [`interaction`] | tool, rotate, arrow and guide gestures |
//! | [`decode`] | decoding a file, and the workers that do it off the UI thread |
//!
//! egui lives in the bin modules only — `theme`, `modals`, `develop`, `library`, `filmstrip`,
//! `thumb`, `shell`. Anything testable without a window belongs in the lib (`lib.rs`).

use eframe::egui;
use rustroom_app::photo_id::PhotoId;
use std::time::{Duration, Instant};

mod ai_flow;
mod app;
mod bootstrap;
mod catalog_ops;
mod collections_panel;
mod commands;
mod copies;
mod date_rows;
mod decode;
mod develop;
#[cfg(target_os = "macos")]
mod display_colorspace;
mod export_flow;
mod filmstrip;
mod folder_rows;
#[cfg(test)]
mod hover_coverage;
mod immich_flow;
mod immich_panel;
mod import_flow;
mod interaction;
mod keyword_ops;
mod keyword_rows;
mod library;
mod library_state;
mod list_row;
mod menu_bar;
mod modals;
mod multi;
mod name_edit;
#[cfg(target_os = "macos")]
mod native_menu;
mod preview_flow;
mod segmented;
mod session;
mod session_flow;
mod settings_apply;
mod shell;
mod smart_sources;
mod snapshots;
mod sources;
mod status_bar;
#[cfg(test)]
mod test_app;
#[cfg(test)]
mod test_ui;
mod theme;
mod thumb;
mod transfer;
mod undo;

use modals::Modal;

pub(crate) use app::{AppView, LibraryMode, RustroomApp};
pub(crate) use develop::canvas_geom::rects_around;
pub(crate) use library_state::{Library, LibrarySource};
pub(crate) use session::{PendingOpen, Session};

pub(crate) use rustroom_app::view_region::PREVIEW_MAX_DIM;
/// How long the selection must sit still before we commit to a decode. Holding an arrow key
/// therefore starts zero decodes until you settle.
const DECODE_DEBOUNCE: Duration = Duration::from_millis(180);

/// Milliseconds since the process started, for the `RUSTROOM_DEBUG_RENDER` open-path trace.
fn debug_ms() -> f64 {
    static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    START.elapsed().as_secs_f64() * 1e3
}
/// Superseded decodes are abandoned rather than aborted, so cap how many run at once.
const MAX_CONCURRENT_DECODES: usize = 2;

/// One at a time: a full-resolution decode holds a 313 MB buffer, and two in flight would be
/// 626 MB of transient allocation for no gain.
const MAX_CONCURRENT_FULL_DECODES: usize = 1;
/// Recently viewed photos kept decoded in memory (~85 MB each).
const BUFFER_CACHE_CAP: usize = 3;
/// GPU memory for preview textures (M9.14): six 26-Mpx RGBA pictures — the on-screen window
/// plus the neighbours fetched ahead (Develop's filmstrip pair, Compare's two candidates).
const PREVIEW_TEXTURE_BUDGET: usize = 6 * 52 * 1024 * 1024;

fn main() -> eframe::Result<()> {
    let mut native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    // ⚠ MEASUREMENT SWITCH (2026-09-29): `RUSTROOM_DEBUG_WINDOW=1440x875` opens the window at that
    // inner size, top left of the main display — how the design language's "nothing overlaps on a
    // 13-inch MacBook Pro" is checked. The window ignores Accessibility resizes, and macOS
    // otherwise restores whatever size the last instance closed at.
    if let Some((w, h)) = std::env::var("RUSTROOM_DEBUG_WINDOW")
        .ok()
        .and_then(|v| rustroom_app::bar_plan::parse_size(&v))
    {
        native_options.viewport = egui::ViewportBuilder::default()
            .with_inner_size([w, h])
            .with_position([0.0, 25.0]);
    }
    eframe::run_native(
        "Rustroom",
        native_options,
        Box::new(|cc| Ok(Box::new(RustroomApp::new(cc)))),
    )
}
