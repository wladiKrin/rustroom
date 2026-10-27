//! Photo → Photo merge → HDR… (M10.17a): Lightroom's HDR Merge Preview — the merged photo on the
//! left, the five options on the right, Cancel and Merge.
//!
//! ⚠ The preview well follows the canvas's idiom (`develop/canvas/paint.rs`): a spinner before the
//! first preview, and the last one lightly dimmed while the next is made — never an empty flash.

use super::{dialog, Footer, Modal};
use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use photo_merge::deghost::Deghost;
use rustroom_app::view_region;

/// The options column's width.
const OPTIONS_W: f32 = 220.0;

fn deghost_hover(d: Deghost) -> &'static str {
    match d {
        Deghost::None => "Blends every photo everywhere, for a still scene on a tripod",
        Deghost::Low => "Takes only clearly moved things from one photo",
        Deghost::Medium => "Takes moving people and leaves from one photo",
        Deghost::High => "Takes anything that changed between the photos from one photo",
    }
}

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Some(d) = app.hdr.dialog.as_mut() else {
        app.modal = Modal::None;
        return;
    };
    let mut draft = d.draft;
    let count = d.plan.frames.len();
    let name = d
        .plan
        .output
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let preview = d.preview.clone();
    let (stale, failed) = (d.stale, d.failed.clone());
    let (mut zoom, mut pan) = (d.zoom, d.pan);
    let pan_drag = app.settings.pan_drag;

    let out = dialog(
        ctx,
        &mut app.settings,
        "hdr_merge",
        "HDR merge preview",
        theme::DialogSize::Wide,
        Footer::confirm(
            "Merge",
            "Merges the photos into one HDR photo beside them, in the background",
            true,
            "Cancel",
        ),
        |ui| {
            ui.horizontal_top(|ui| {
                let well = egui::vec2(
                    (ui.available_width() - OPTIONS_W - ui.spacing().item_spacing.x).max(160.0),
                    ui.available_height().max(160.0),
                );
                let (rect, response) = ui.allocate_exact_size(well, egui::Sense::drag());
                // Develop's gestures through the canvas's own code: pinch or ⌘-scroll zooms about
                // the pointer, a swipe or the pan binding's drag moves, and at fit it centres.
                let factor = ui.input(|i| i.zoom_delta());
                if factor != 1.0 && response.hovered() {
                    let old = zoom;
                    zoom = view_region::zoom_step(old, factor);
                    if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                        let anchor = rect.center() + pan;
                        let (dx, dy) = view_region::zoom_anchor_shift(
                            (p.x, p.y),
                            (anchor.x, anchor.y),
                            old,
                            zoom,
                        );
                        pan += egui::vec2(dx, dy);
                    }
                }
                let scroll = ui.input(|i| i.smooth_scroll_delta);
                let space = ui.input(|i| i.key_down(egui::Key::Space));
                let focused = ui.ctx().memory(|m| m.focused().is_some());
                pan += crate::develop::canvas::navigation::pan_delta(
                    &response, scroll, space, pan_drag, focused,
                );
                if zoom <= view_region::MIN_ZOOM {
                    pan = egui::Vec2::ZERO;
                }
                let t = theme::tokens::of(ui);
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(theme::tokens::RADIUS),
                    t.canvas,
                );
                match &preview {
                    Some((plain, tinted)) => {
                        let tex = if draft.overlay {
                            tinted.as_ref().unwrap_or(plain)
                        } else {
                            plain
                        };
                        let size = tex.size_vec2();
                        let fit = (rect.width() / size.x).min(rect.height() / size.y);
                        let at =
                            egui::Rect::from_center_size(rect.center() + pan, size * fit * zoom);
                        let mut well_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                        well_ui.set_clip_rect(rect);
                        egui::Image::new(tex).paint_at(&well_ui, at);
                        if stale {
                            well_ui.painter().rect_filled(
                                at,
                                egui::CornerRadius::ZERO,
                                theme::stale_dim(),
                            );
                        }
                    }
                    None => {
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                        child.centered_and_justified(|ui| {
                            if failed.is_none() {
                                ui.spinner();
                            }
                        });
                    }
                }
                response.hover(format!(
                    "{count} photos merged into {name}, as it will open; pinch to zoom, swipe to move"
                ));

                ui.vertical(|ui| {
                    ui.set_width(OPTIONS_W);
                    if let Some(e) = &failed {
                        theme::error_hint(ui, e);
                    }
                    ui.checkbox(&mut draft.auto_align, "Auto align")
                        .hover("Lines the photos up when the camera moved between them");
                    ui.checkbox(&mut draft.auto_settings, "Auto settings")
                        .hover("Sets the merged photo's exposure, whites and blacks automatically");
                    ui.add_space(8.0);
                    theme::sub_label(ui, "Deghost");
                    let items: Vec<(Deghost, &str, &str)> = Deghost::ALL
                        .iter()
                        .map(|&g| (g, g.label(), deghost_hover(g)))
                        .collect();
                    crate::segmented::segmented(ui, &mut draft.deghost, &items);
                    ui.add_enabled(
                        draft.deghost != Deghost::None,
                        egui::Checkbox::new(&mut draft.overlay, "Show deghost overlay"),
                    )
                    .hover("Shows in red what was taken from one photo alone");
                    ui.add_space(8.0);
                    ui.checkbox(&mut draft.create_stack, "Create stack")
                        .hover("Stacks the merged photo on top of the photos it was made from");
                });
            });
        },
    );

    if let Some(d) = app.hdr.dialog.as_mut() {
        d.draft = draft;
        d.zoom = zoom;
        d.pan = pan;
    }
    if out.primary {
        app.start_hdr_merge();
    } else if out.dismissed {
        app.close_hdr_merge();
    }
}
