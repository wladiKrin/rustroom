//! Photo → Photo merge → Panorama… (M10.17b): Lightroom's Panorama Merge Preview — the stitched panorama
//! on the left with Auto crop's rectangle, the options on the right, Cancel and Merge.
//!
//! ⚠ The well is the HDR dialog's (`hdr_merge.rs`): Develop's zoom and pan through the canvas's own code,
//! a spinner before the first preview, the last one dimmed while the next is made.

use super::{dialog, Footer, Modal};
use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use photo_merge::projection::Projection;
use rustroom_app::view_region;

/// The options column's width.
const OPTIONS_W: f32 = 220.0;

fn projection_hover(p: Projection) -> &'static str {
    match p {
        Projection::Spherical => "Lays the panorama on a sphere, for very wide or tall sweeps",
        Projection::Cylindrical => "Lays the panorama on a cylinder, keeping verticals straight",
        Projection::Perspective => "Keeps straight lines straight, for a narrow sweep",
    }
}

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Some(d) = app.pano.dialog.as_mut() else {
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
    let (stale, failed, crop, picked) = (d.stale, d.failed.clone(), d.crop, d.projection);
    let (no_profile, unconnected, too_wide) =
        (d.no_profile.clone(), d.unconnected.clone(), d.too_wide);
    let can_merge = d.can_merge();
    let stacks = d.plan.beside_frames();
    let models = d.models.clone();
    let (mut zoom, mut pan) = (d.zoom, d.pan);
    let pan_drag = app.settings.pan_drag;
    let mut offer_download = false;

    let out = dialog(
        ctx,
        &mut app.settings,
        "pano_merge",
        "Panorama merge preview",
        theme::DialogSize::Wide,
        Footer::confirm(
            "Merge",
            "Stitches the photos into one panorama beside them, in the background",
            can_merge,
            "Cancel",
        ),
        |ui| {
            ui.horizontal_top(|ui| {
                let well = egui::vec2(
                    (ui.available_width() - OPTIONS_W - ui.spacing().item_spacing.x).max(160.0),
                    ui.available_height().max(160.0),
                );
                let (rect, response) = ui.allocate_exact_size(well, egui::Sense::drag());
                let factor = ui.input(|i| i.zoom_delta());
                if factor != 1.0 && response.hovered() {
                    let old = zoom;
                    zoom = view_region::zoom_step(old, factor);
                    if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                        let anchor = rect.center() + pan;
                        let (dx, dy) = view_region::zoom_anchor_shift((p.x, p.y), (anchor.x, anchor.y), old, zoom);
                        pan += egui::vec2(dx, dy);
                    }
                }
                let scroll = ui.input(|i| i.smooth_scroll_delta);
                let space = ui.input(|i| i.key_down(egui::Key::Space));
                let focused = ui.ctx().memory(|m| m.focused().is_some());
                pan += crate::develop::canvas::navigation::pan_delta(&response, scroll, space, pan_drag, focused);
                if zoom <= view_region::MIN_ZOOM {
                    pan = egui::Vec2::ZERO;
                }
                let t = theme::tokens::of(ui);
                ui.painter().rect_filled(rect, egui::CornerRadius::same(theme::tokens::RADIUS), t.canvas);
                match &preview {
                    Some(tex) => {
                        let size = tex.size_vec2();
                        let fit = (rect.width() / size.x).min(rect.height() / size.y);
                        let at = egui::Rect::from_center_size(rect.center() + pan, size * fit * zoom);
                        let mut well_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                        well_ui.set_clip_rect(rect);
                        egui::Image::new(tex).paint_at(&well_ui, at);
                        // Auto crop's rectangle: outside it dimmed and a border around it, as the crop tool draws a crop.
                        if let (Some(c), true) = (crop, draft.auto_crop && draft.auto_crop_applies()) {
                            let inner = egui::Rect::from_min_max(
                                at.lerp_inside(egui::vec2(c.left, c.top)),
                                at.lerp_inside(egui::vec2(c.right, c.bottom)),
                            );
                            let p = well_ui.painter();
                            for band in [
                                egui::Rect::from_min_max(at.min, egui::pos2(at.max.x, inner.min.y)),
                                egui::Rect::from_min_max(egui::pos2(at.min.x, inner.max.y), at.max),
                                egui::Rect::from_min_max(egui::pos2(at.min.x, inner.min.y), egui::pos2(inner.min.x, inner.max.y)),
                                egui::Rect::from_min_max(egui::pos2(inner.max.x, inner.min.y), egui::pos2(at.max.x, inner.max.y)),
                            ] {
                                p.rect_filled(band, egui::CornerRadius::ZERO, theme::stale_dim());
                            }
                            // The crop tool's border: only what is inside it is the photo, until Fill edges.
                            crate::develop::canvas::crop_overlay::border(p, inner, false);
                        }
                        if stale {
                            well_ui.painter().rect_filled(at, egui::CornerRadius::ZERO, theme::stale_dim());
                        }
                    }
                    None => {
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                        child.centered_and_justified(|ui| {
                            if failed.is_none() && unconnected.is_none() && !too_wide {
                                ui.spinner();
                            }
                        });
                    }
                }
                response.hover(format!("{count} photos stitched into {name}; pinch to zoom, swipe to move"));

                ui.vertical(|ui| {
                    ui.set_width(OPTIONS_W);
                    if let Some(names) = &unconnected {
                        theme::error_hint(ui, &format!("{} shares nothing with the others", names.join(", ")));
                    }
                    if too_wide {
                        theme::error_hint(ui, "Too wide for Perspective");
                    }
                    if let Some(e) = &failed {
                        theme::error_hint(ui, e);
                    }
                    theme::sub_label(ui, "Projection");
                    let items: Vec<(Projection, &str, &str)> = [Projection::Spherical, Projection::Cylindrical, Projection::Perspective]
                        .into_iter()
                        .map(|p| (p, p.label(), projection_hover(p)))
                        .collect();
                    // With Auto select on the control shows Auto's pick and cannot be changed.
                    let mut shown = if draft.auto_projection { picked } else { draft.projection };
                    ui.add_enabled_ui(!draft.auto_projection, |ui| {
                        crate::segmented::segmented(ui, &mut shown, &items);
                    });
                    if !draft.auto_projection {
                        draft.projection = shown;
                    }
                    let was_auto = draft.auto_projection;
                    ui.checkbox(&mut draft.auto_projection, "Auto select projection")
                        .hover("Picks the projection that suits how wide the sweep is");
                    if was_auto && !draft.auto_projection {
                        // Turning Auto off starts from what Auto had picked.
                        draft.projection = picked;
                    }
                    ui.add_space(8.0);
                    ui.add_enabled(draft.auto_crop_applies(), egui::Checkbox::new(&mut draft.auto_crop, "Auto crop"))
                        .hover("Crops to the largest rectangle the photos cover, which you can widen later")
                        .on_disabled_hover_text("Fill edges leaves nothing to crop away");
                    ui.checkbox(&mut draft.fill_edges, "Fill edges")
                        .hover("Fills the empty edges from what surrounds them");
                    if crate::pano_flow::needs_lama(&draft, models.as_deref()) {
                        theme::hint(ui, "Fill edges needs the LaMa model");
                        if ui.button("Download…").hover("Downloads the model Remove uses to fill").clicked() {
                            offer_download = true;
                        }
                    }
                    ui.add_space(8.0);
                    // A merge from Immich is saved in another folder, where no stack can hold it.
                    ui.add_enabled(stacks, egui::Checkbox::new(&mut draft.create_stack, "Create stack"))
                        .hover("Stacks the panorama on top of the photos it was made from")
                        .on_disabled_hover_text("The panorama is saved in another folder than its photos");
                    if let Some(lens) = &no_profile {
                        theme::hint(ui, &format!("No lens profile for {lens}: distortion and vignetting are not corrected"));
                    }
                });
            });
        },
    );

    if let Some(d) = app.pano.dialog.as_mut() {
        d.draft = draft;
        d.zoom = zoom;
        d.pan = pan;
    }
    if offer_download {
        app.offer_lama_download();
    } else if out.primary {
        app.start_pano_merge();
    } else if out.dismissed {
        app.close_pano_merge();
    }
}
