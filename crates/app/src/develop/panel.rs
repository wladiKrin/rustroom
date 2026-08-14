//! The right-hand adjustments panel: histogram, then every Develop control.

use crate::{preview_edits, theme, RustroomApp};
use edit_model::Edits;
use eframe::egui;
use rustroom_app::crop_tool;

/// Draw the adjustments panel.
///
/// Returns whether a photo edit settled into a new history entry.
///
/// ⚠ One history entry per *settled* drag, not per changed frame: the `slider` closure records
/// `r.drag_stopped()` separately from `r.changed()`, so dragging Exposure across its range is
/// one Ctrl+Z rather than two hundred.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui) -> bool {
    let mut photo_edited = false;
    egui::Panel::right("panel")
        .default_size(240.0)
        .show(ui, |ui| {
            if app.pending.is_some() {
                ui.disable();
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(s) = &mut app.session {
                    draw_histogram(ui, &s.hist);
                    ui.separator();

                    let mut changed = false;
                    let mut settled = false;
                    let mut slider = |ui: &mut egui::Ui, value: &mut f32, range, label| {
                        let r = ui.add(egui::Slider::new(value, range).text(label));
                        changed |= r.changed();
                        settled |= r.drag_stopped();
                    };
                    slider(ui, &mut s.edits.white_balance.temp, -100.0..=100.0, "Temp");
                    slider(ui, &mut s.edits.white_balance.tint, -100.0..=100.0, "Tint");
                    slider(ui, &mut s.edits.exposure, -5.0..=5.0, "Exposure");
                    slider(ui, &mut s.edits.contrast, -100.0..=100.0, "Contrast");
                    slider(ui, &mut s.edits.highlights, -100.0..=100.0, "Highlights");
                    slider(ui, &mut s.edits.shadows, -100.0..=100.0, "Shadows");
                    slider(ui, &mut s.edits.whites, -100.0..=100.0, "Whites");
                    slider(ui, &mut s.edits.blacks, -100.0..=100.0, "Blacks");
                    // Lightroom's "Presence" grouping, and it reflects that these are
                    // tonal rather than detail controls — which is also why they are in
                    // the Tone edit group rather than Detail.
                    slider(
                        ui,
                        &mut s.edits.local_contrast.texture,
                        -100.0..=100.0,
                        "Texture",
                    );
                    slider(
                        ui,
                        &mut s.edits.local_contrast.clarity,
                        -100.0..=100.0,
                        "Clarity",
                    );
                    slider(ui, &mut s.edits.dehaze.amount, -100.0..=100.0, "Dehaze");
                    slider(ui, &mut s.edits.saturation, -100.0..=100.0, "Saturation");
                    slider(ui, &mut s.edits.vibrance, -100.0..=100.0, "Vibrance");

                    ui.separator();
                    egui::CollapsingHeader::new("Hue").show(ui, |ui| {
                        for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
                            slider(ui, &mut s.edits.hsl.bands[i].hue, -100.0..=100.0, *name);
                        }
                    });
                    egui::CollapsingHeader::new("Saturation").show(ui, |ui| {
                        for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
                            slider(
                                ui,
                                &mut s.edits.hsl.bands[i].saturation,
                                -100.0..=100.0,
                                *name,
                            );
                        }
                    });
                    egui::CollapsingHeader::new("Luminance").show(ui, |ui| {
                        for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
                            slider(
                                ui,
                                &mut s.edits.hsl.bands[i].luminance,
                                -100.0..=100.0,
                                *name,
                            );
                        }
                    });
                    egui::CollapsingHeader::new("Sharpening").show(ui, |ui| {
                        let sh = &mut s.edits.sharpening;
                        slider(ui, &mut sh.amount, 0.0..=100.0, "Amount");
                        // The one slider not on a 0..100 scale: it is a physical pixel
                        // quantity, in full-resolution pixels, and naming it in pixels
                        // is what makes "scaled for the render" comprehensible.
                        slider(ui, &mut sh.radius, 0.5..=3.0, "Radius (px)");
                        slider(ui, &mut sh.detail, 0.0..=100.0, "Detail");
                        slider(ui, &mut sh.masking, 0.0..=100.0, "Masking");
                    });
                    egui::CollapsingHeader::new("Noise Reduction").show(ui, |ui| {
                        let nr = &mut s.edits.noise_reduction;
                        slider(ui, &mut nr.luminance, 0.0..=100.0, "Luminance");
                        slider(ui, &mut nr.detail, 0.0..=100.0, "Detail");
                        slider(ui, &mut nr.color, 0.0..=100.0, "Color");
                        slider(ui, &mut nr.smoothness, 0.0..=100.0, "Smoothness");
                    });
                    egui::CollapsingHeader::new("Vignette").show(ui, |ui| {
                        let v = &mut s.edits.vignette;
                        slider(ui, &mut v.amount, -100.0..=100.0, "Amount");
                        slider(ui, &mut v.midpoint, 0.0..=100.0, "Midpoint");
                        slider(ui, &mut v.roundness, -100.0..=100.0, "Roundness");
                        slider(ui, &mut v.feather, 0.0..=100.0, "Feather");
                    });

                    ui.separator();
                    if ui.button("Reset all").clicked() {
                        s.edits = Edits::default();
                        changed = true;
                        settled = true;
                    }

                    if s.geometry_mode {
                        ui.separator();
                        ui.label("Geometry");
                        // Quarter turns and mirroring. ⚠ Each button moves the crop by the
                        // *same* transform, or the crop lands on different content — see
                        // `CropRect::rotated_cw`. Keep the two in step.
                        ui.horizontal(|ui| {
                            let g = &mut s.edits.geometry;
                            let mut turned = false;
                            if ui.button("⟲").on_hover_text("Rotate left").clicked() {
                                g.orientation.rotate_ccw();
                                for _ in 0..3 {
                                    g.crop = g.crop.rotated_cw();
                                }
                                turned = true;
                            }
                            if ui.button("⟳").on_hover_text("Rotate right").clicked() {
                                g.orientation.rotate_cw();
                                g.crop = g.crop.rotated_cw();
                                turned = true;
                            }
                            if ui.button("⬌").on_hover_text("Flip horizontal").clicked() {
                                g.orientation.flip_h();
                                g.crop = g.crop.flipped_h();
                                turned = true;
                            }
                            if ui.button("⬍").on_hover_text("Flip vertical").clicked() {
                                g.orientation.flip_v();
                                g.crop = g.crop.flipped_h();
                                g.crop = g.crop.rotated_cw();
                                g.crop = g.crop.rotated_cw();
                                turned = true;
                            }
                            if turned {
                                // ⚠ `settled` too: a button press is a discrete act, so it
                                // is one history entry and one Ctrl+Z — unlike a slider,
                                // which pushes only on `drag_stopped`.
                                changed = true;
                                settled = true;
                            }
                        });
                        let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
                        let presets: [(&str, Option<f32>); 5] = [
                            ("Free", None),
                            ("1:1", Some(1.0)),
                            ("3:2", Some(1.5)),
                            ("4:3", Some(4.0 / 3.0)),
                            ("16:9", Some(16.0 / 9.0)),
                        ];
                        ui.horizontal(|ui| {
                            for (label, pa) in presets {
                                if ui.button(label).clicked() {
                                    s.edits.geometry.crop = match pa {
                                        Some(a) => crop_tool::snap_to_aspect(a / img_aspect),
                                        None => edit_model::CropRect::default(),
                                    };
                                    settled = true;
                                }
                            }
                        });
                        let ar = ui.add(
                            egui::Slider::new(&mut s.edits.geometry.angle, -45.0..=45.0)
                                .text("Straighten"),
                        );
                        changed |= ar.changed();
                        settled |= ar.drag_stopped();
                    }

                    if changed {
                        s.request(
                            preview_edits(&s.edits, s.geometry_mode),
                            s.render_max_dim(),
                            s.requested_region,
                        );
                    }
                    if settled {
                        photo_edited |= s.history.push(s.edits.clone());
                    }

                    ui.separator();
                    // The filename first: which photo this is matters more than what
                    // shot it, and the panel scrolls, so it belongs above the metadata.
                    if let Some(name) = s.path.file_name() {
                        ui.label(egui::RichText::new(name.to_string_lossy()).strong());
                    }
                    theme::caption(ui, &s.make_model);
                } else {
                    ui.label("No image open.");
                }
            });
        });
    photo_edited
}

fn draw_histogram(ui: &mut egui::Ui, hist: &[[u32; 256]; 3]) {
    let max = hist
        .iter()
        .flat_map(|c| c.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 80.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let colors = [
        egui::Color32::from_rgb(220, 80, 80),
        egui::Color32::from_rgb(80, 200, 80),
        egui::Color32::from_rgb(90, 120, 230),
    ];
    for (ci, chan) in hist.iter().enumerate() {
        for (i, &count) in chan.iter().enumerate() {
            let x = rect.left() + rect.width() * (i as f32 / 255.0);
            let h = rect.height() * (count as f32 / max);
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.0, colors[ci].linear_multiply(0.5)),
            );
        }
    }
}
