//! The Settings window.
//!
//! ⚠ **A draft, applied on Apply** (2026-09-22, user's call): the first tabbed version applied
//! every change as it was made and said so in the footer; the user wanted Apply and Cancel.
//! `SettingsDraft` holds the fifteen preferences while the dialog is open, and `apply` writes
//! back exactly what changed, through the same `apply_*` methods the live version called —
//! so a cancelled dialog leaves nothing behind, and an applied one saves the file once.
//!
//! ⚠ **The descriptions are hovers, not paragraphs under each control** (same day, same
//! call): five settings with a paragraph each read as one block of text. The rule the Masks
//! panel already follows. The two short, *value-dependent* notes — the demosaic tier's and the
//! resampler's — stay inline beside their combos, because they change with the choice.

use super::shell::tabs;
use super::{dialog, Footer};
use crate::decode::view_quality_hint;
use crate::theme::Hover as _;
use crate::{theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::settings::{
    Accent, DragQuality, EraseDrag, PanDrag, ParametricDrag, Settings, ThemeChoice,
};

/// Air between two settings on a tab.
const GAP: f32 = 8.0;

/// The preferences the dialog edits, held while it is open.
#[derive(Clone, PartialEq, Debug)]
pub struct SettingsDraft {
    confirm: bool,
    write_xmp: bool,
    auto_advance: bool,
    camera_rendering: bool,
    auto_lens: bool,
    tier: raw_decode::Demosaic,
    gpu_tail: bool,
    resampler: pipeline::Resampler,
    ai_subject: ai_mask::SubjectModel,
    export_width: u32,
    theme_choice: ThemeChoice,
    accent: Accent,
    ui_scale: rustroom_app::settings::UiScale,
    pan_drag: PanDrag,
    erase_drag: EraseDrag,
    parametric_drag: ParametricDrag,
    stack_drag: rustroom_app::settings::StackDrag,
    drag_quality: DragQuality,
    display_space: color::OutputSpace,
    stacking: rustroom_app::stack::Stacking,
    layout: rustroom_app::layout::Layout,
    immich_enabled: bool,
    immich_url: String,
    immich_key: String,
    immich_upload_key: String,
    thumb_budget_mb: u32,
    immich_thumb_budget_mb: u32,
    preview_demosaic: Option<raw_decode::Demosaic>,
    preview_budget_mb: u32,
}

/// Which formats take part in stacking, as chips.
///
/// ⚠⚠ **A lit chip means "takes part", and `FormatFilter` is a set of *hidden* formats** — so the
/// chip's boolean is the inverse of what the field stores. Reused rather than reinvented because it
/// already collapses `jpg`/`jpeg` and `tif`/`tiff` into one format each, which stacking needs: a
/// folder mixing `.JPG` and `.jpeg` must not be two formats.
///
/// ⚠ Over the **global** format list, not the open folder's — this is Settings, and `shell`'s
/// `format_chips` is the folder-scoped one. Two callers, two scopes, deliberately not shared.
fn stack_format_chips(ui: &mut egui::Ui, s: &mut rustroom_app::stack::Stacking) {
    let mut seen: Vec<&str> = Vec::new();
    for ext in catalog::RAW_EXTENSIONS
        .iter()
        .chain(catalog::IMAGE_EXTENSIONS)
    {
        let c = rustroom_app::format_filter::canonical_ext(ext);
        if !seen.contains(&c) {
            seen.push(c);
        }
    }
    ui.horizontal_wrapped(|ui| {
        for ext in seen {
            let mut on = !s.formats.is_hidden(ext);
            let says = format!(
                "Includes {} files when grouping photos that share a name",
                ext.to_uppercase()
            );
            if crate::segmented::chip(ui, &mut on, &ext.to_uppercase(), &says).changed() {
                s.formats.toggle(ext);
            }
        }
    });
}

impl SettingsDraft {
    fn from_settings(s: &Settings) -> Self {
        Self {
            confirm: s.confirm_preset_delete,
            write_xmp: s.write_xmp,
            auto_advance: s.auto_advance,
            camera_rendering: s.default_rendering == edit_model::Rendering::Camera,
            auto_lens: s.auto_lens_correction,
            tier: s.view_demosaic,
            gpu_tail: s.gpu_tail,
            resampler: s.resampler,
            ai_subject: s.ai_subject_model,
            export_width: s.export_concurrency,
            theme_choice: s.theme,
            accent: s.accent,
            ui_scale: s.ui_scale,
            pan_drag: s.pan_drag,
            erase_drag: s.erase_drag,
            parametric_drag: s.parametric_drag,
            stack_drag: s.stack_drag,
            drag_quality: s.drag_quality,
            display_space: s.display_space,
            stacking: s.stacking.clone(),
            layout: s.layout,
            immich_enabled: s.immich_enabled,
            immich_url: s.immich_url.clone(),
            immich_key: s.immich_key.clone(),
            immich_upload_key: s.immich_upload_key.clone(),
            thumb_budget_mb: s.thumb_budget_mb,
            immich_thumb_budget_mb: s.immich_thumb_budget_mb,
            preview_demosaic: s.preview_demosaic,
            preview_budget_mb: s.preview_budget_mb,
        }
    }

    /// Write back what differs from the live settings, through the methods that know what
    /// each change entails (a re-decode, a re-render, a restyle), and save once.
    fn apply(self, app: &mut RustroomApp, ctx: &egui::Context) {
        let mut plain = false;
        if self.confirm != app.settings.confirm_preset_delete {
            app.settings.confirm_preset_delete = self.confirm;
            plain = true;
        }
        if self.write_xmp != app.settings.write_xmp {
            app.settings.write_xmp = self.write_xmp;
            plain = true;
        }
        let want_rendering = if self.camera_rendering {
            edit_model::Rendering::Camera
        } else {
            edit_model::Rendering::None
        };
        if want_rendering != app.settings.default_rendering {
            app.settings.default_rendering = want_rendering;
            plain = true;
        }
        if self.auto_lens != app.settings.auto_lens_correction {
            app.settings.auto_lens_correction = self.auto_lens;
            plain = true;
        }
        if self.export_width != app.settings.export_concurrency {
            app.settings.export_concurrency = self.export_width;
            plain = true;
        }
        if self.pan_drag != app.settings.pan_drag {
            app.settings.pan_drag = self.pan_drag;
            plain = true;
        }
        if self.erase_drag != app.settings.erase_drag {
            app.settings.erase_drag = self.erase_drag;
            plain = true;
        }
        if self.parametric_drag != app.settings.parametric_drag {
            app.settings.parametric_drag = self.parametric_drag;
            plain = true;
        }
        if self.stack_drag != app.settings.stack_drag {
            app.settings.stack_drag = self.stack_drag;
            plain = true;
        }
        if self.drag_quality != app.settings.drag_quality {
            app.settings.drag_quality = self.drag_quality;
            plain = true;
        }
        if self.auto_advance != app.settings.auto_advance {
            app.settings.auto_advance = self.auto_advance;
            plain = true;
        }
        // ⚠⚠ Applied plainly, **not** through `set_connection`. That accessor owns the *browse*
        // connection; the upload key is a separate credential and must not join it, or the
        // read-only guarantee browsing relies on would quietly acquire a write grant.
        if self.stacking != app.settings.stacking {
            app.settings.stacking = self.stacking.clone();
            // ⚠⚠ A **rebuild**, because stacking decides what `entries` contains and every consumer
            // indexes into that list. Without this the grid keeps the old grouping until the next
            // folder open — the same shape as the Immich worker that kept a stale connection.
            app.rebuild_entries(None);
            plain = true;
        }
        // M10.8b: where the panels sit. ⚠ A new place for the filmstrip shows it — choosing where it
        // goes is asking to see it there.
        if self.layout != app.settings.layout {
            if self.layout.filmstrip != app.settings.layout.filmstrip {
                app.show_filmstrip = true;
            }
            app.settings.layout = self.layout;
            plain = true;
        }
        if self.immich_upload_key != app.settings.immich_upload_key {
            app.settings.immich_upload_key = self.immich_upload_key.clone();
            plain = true;
        }
        // ⚠⚠ **ONE branch for EVERY input to `connection` — the URL, the key, and the master
        // switch.** An earlier version gave the switch its own branch that only re-pointed the
        // shared thumbnail cell, so turning Immich off and then on again left the **worker** holding
        // a snapshot of a `None` connection and browsing stayed dead until the next restart, while
        // the Test button — which builds its own client from what is typed — still said "connected".
        // Found by use, 2026-09-24. The comment three lines down already said the worker has to be
        // rebuilt; splitting the condition is exactly what lost it.
        if self.immich_url != app.settings.immich_url
            || self.immich_key != app.settings.immich_key
            || self.immich_enabled != app.settings.immich_enabled
        {
            app.settings.immich_enabled = self.immich_enabled;
            // ⚠ Through the accessor, which is the only writer of those two fields — so moving
            // the key to the OS keychain later changes one file rather than this one too.
            rustroom_app::immich_source::set_connection(
                &mut app.settings,
                &self.immich_url,
                &self.immich_key,
            );
            // ⚠⚠ And the shared cell the thumbnail loader reads, or a key pasted here would not
            // reach the loader until the next restart.
            rustroom_app::immich_source::refresh_shared(&app.settings);
            // ⚠ The worker holds a snapshot of the connection, so it has to be rebuilt — a job
            // already in flight finishes against the server it started with, which is the point of
            // the snapshot.
            app.immich = rustroom_app::immich_worker::ImmichWorker::new(
                rustroom_app::immich_worker::live_fetcher(rustroom_app::immich_source::connection(
                    &app.settings,
                )),
                {
                    let ctx = ctx.clone();
                    move || ctx.request_repaint()
                },
            );
            // ⚠ Forget the album list: it belongs to whatever server was configured before.
            app.immich_albums = None;
            plain = true;
        }
        if self.thumb_budget_mb != app.settings.thumb_budget_mb {
            app.settings.thumb_budget_mb = self.thumb_budget_mb;
            plain = true;
        }
        if self.immich_thumb_budget_mb != app.settings.immich_thumb_budget_mb {
            app.settings.immich_thumb_budget_mb = self.immich_thumb_budget_mb;
            plain = true;
        }
        if self.preview_demosaic != app.settings.preview_demosaic {
            app.settings.preview_demosaic = self.preview_demosaic;
            plain = true;
        }
        if self.preview_budget_mb != app.settings.preview_budget_mb {
            app.settings.preview_budget_mb = self.preview_budget_mb;
            plain = true;
        }
        if self.ai_subject != app.settings.ai_subject_model {
            app.settings.ai_subject_model = self.ai_subject;
            // ⚠ The open photo's maps came from the other network: dropped, so the next frame
            // loads (or computes) this one's. Other photos key on the model, so they need nothing.
            if let Some(s) = &app.session {
                s.engine.ai_maps().write().expect("ai maps").clear();
                s.request(
                    s.view_edits(app.tool),
                    s.render_max_dim(),
                    s.requested_region,
                );
            }
            plain = true;
        }
        if plain {
            app.save_settings();
        }
        // ⚠ The size is part of the appearance: a size-only change used to be dropped here, and
        // Apply did nothing (reported 2026-09-28).
        if self.theme_choice != app.settings.theme
            || self.accent != app.settings.accent
            || self.ui_scale != app.settings.ui_scale
        {
            app.apply_appearance(self.theme_choice, self.accent, self.ui_scale, ctx);
        }
        if self.tier != app.settings.view_demosaic {
            app.apply_view_demosaic(self.tier, ctx);
        }
        if self.gpu_tail != app.settings.gpu_tail {
            app.apply_gpu_tail(self.gpu_tail);
        }
        if self.resampler != app.settings.resampler {
            app.apply_resampler(self.resampler);
        }
        if self.display_space != app.settings.display_space {
            app.apply_display_space(self.display_space, ctx);
        }
    }
}

/// The Settings window: preferences that survive a restart.
pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut d = app
        .settings_draft
        .take()
        .unwrap_or_else(|| SettingsDraft::from_settings(&app.settings));
    let os_manages_colour = app.os_manages_colour;

    let out = dialog(
        ctx,
        &mut app.settings,
        "settings",
        "Settings",
        theme::DialogSize::Standard,
        Footer::confirm(
            "Apply",
            "Keeps these settings and closes the dialog",
            true,
            "Cancel",
        ),
        |ui| {
            let tab = tabs(
                ui,
                egui::Id::new("settings_tab"),
                &[
                    ("Behaviour", "Shows what happens when you delete, open and save"),
                    ("Appearance", "Shows the theme, accent, text size, stacking and where the panels sit"),
                    ("Display", "Shows the colour space the photo is shown in"),
                    ("Performance", "Shows preview quality, the caches, the GPU and how many photos export at once"),
                    ("Input", "Shows which mouse buttons pan, erase and draw"),
                    ("Immich", "Shows the connection to your Immich server"),
                ],
            );
            ui.spacing_mut().item_spacing.y = GAP;
            match tab {
                0 => {
                    ui.checkbox(&mut d.confirm, "Confirm before deleting a preset")
                        .hover("Asks before a preset is deleted");
                    ui.checkbox(
                        &mut d.camera_rendering,
                        "Open new photos with the camera rendering",
                    )
                    .hover("Opens unedited RAWs with the camera's own tone and colour");
                    ui.checkbox(&mut d.auto_lens, "Apply lens corrections to new photos")
                        .hover("Applies the lens profile's corrections to unedited RAWs");
                    ui.checkbox(&mut d.write_xmp, "Write XMP sidecars beside RAW files")
                        .hover(
                            "Writes ratings, labels and keywords to an .xmp file beside each RAW for other apps to read",
                        );
                    ui.checkbox(&mut d.auto_advance, "Move to the next photo after rating")
                        .hover("Moves to the next photo after you rate, flag or label one");
                }
                1 => {
                    egui::Grid::new("settings_appearance")
                        .num_columns(2)
                        .spacing([12.0, GAP])
                        .show(ui, |ui| {
                            ui.label("Theme");
                            egui::ComboBox::from_id_salt("theme_pick")
                                .selected_text(d.theme_choice.label())
                                .show_ui(ui, |ui| {
                                    for t in ThemeChoice::ALL {
                                        ui.selectable_value(&mut d.theme_choice, t, t.label());
                                    }
                                })
                                .response
                                .hover(
                                    "Chooses a dark or light interface, or follows the system",
                                );
                            ui.end_row();

                            ui.label("Accent").hover(
                                "Sets the colour that marks selected and active controls",
                            );
                            // ⚠ Swatches, not a list of names: the choice is a colour, so it is
                            // made by looking. Shown in the theme's own half of each pair.
                            let dark = ui.visuals().dark_mode;
                            let orange_hover =
                                "Uses orange, which sits on the white-balance axis and can colour \
                                 that judgement";
                            let items: Vec<crate::segmented::Swatch<'_, Accent>> = Accent::ALL
                                .iter()
                                .map(|&a| crate::segmented::Swatch {
                                    value: a,
                                    colour: theme::accent_color(a, dark),
                                    label: a.label(),
                                    hover: if a == Accent::Orange { orange_hover } else { "" },
                                })
                                .collect();
                            crate::segmented::swatches(ui, &mut d.accent, &items);
                            ui.end_row();
                            ui.label("Interface size");
                            egui::ComboBox::from_id_salt("ui_scale_pick")
                                .selected_text(d.ui_scale.label())
                                .show_ui(ui, |ui| {
                                    for z in rustroom_app::settings::UiScale::ALL {
                                        ui.selectable_value(&mut d.ui_scale, z, z.label());
                                    }
                                })
                                .response
                                .hover(
                                    "Makes all text and controls larger or smaller",
                                );
                            ui.end_row();

                            // ⚠ In Appearance because stacking is a **browsing** decision, the same
                            // reasoning that keeps the format filter out of the Develop toolbar.
                            // ⚠ Pinned to the first row's height, so it reads beside the checkbox
                            // rather than drifting to the foot of its two-row cell.
                            ui.allocate_ui_with_layout(
                                egui::vec2(0.0, theme::tokens::CONTROL_ROW),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| ui.label("Stacking"),
                            );
                            ui.vertical(|ui| {
                                ui.checkbox(
                                    &mut d.stacking.enabled,
                                    "Group files that share a name",
                                )
                                .hover(
                                    "Shows a RAW and its JPEG as one photo in the Library",
                                );
                                ui.add_enabled_ui(d.stacking.enabled, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Show");
                                        egui::ComboBox::from_id_salt("stack_primary")
                                            .selected_text(d.stacking.primary.label())
                                            .show_ui(ui, |ui| {
                                                for pr in rustroom_app::stack::StackPrimary::ALL {
                                                    ui.selectable_value(
                                                        &mut d.stacking.primary,
                                                        pr,
                                                        pr.label(),
                                                    );
                                                }
                                            })
                                            .response
                                            .hover(
                                                "Chooses which file of a stack the Library shows and opens",
                                            );
                                    });
                                    // ⚠⚠ The chips are the *participating* formats, and
                                    // `FormatFilter` is a set of **hidden** ones — so a lit chip
                                    // means "takes part". Reused for the jpg/jpeg aliasing.
                                    stack_format_chips(ui, &mut d.stacking);
                                });
                                // ⚠ Outside the switch above: the pile draws every stack, the ones
                                // made by hand included (M10.8c after use).
                                ui.horizontal(|ui| {
                                    ui.label("Pile");
                                    crate::segmented::segmented(
                                        ui,
                                        &mut d.stacking.look,
                                        &[
                                            (
                                                rustroom_app::stack::StackLook::Peek,
                                                "Photos below",
                                                "Shows the photos below a stack's top one peeking out",
                                            ),
                                            (
                                                rustroom_app::stack::StackLook::Cards,
                                                "Cards",
                                                "Draws plain cards below a stack's top photo, which loads no extra thumbnails",
                                            ),
                                            (
                                                rustroom_app::stack::StackLook::Off,
                                                "None",
                                                "Draws no pile, so only the count on a stack's top photo says it is a stack",
                                            ),
                                        ],
                                    );
                                });
                            });
                            ui.end_row();
                            super::settings_layout::rows(ui, &mut d.layout);
                        });
                }
                2 => {
                    if os_manages_colour {
                        ui.horizontal(|ui| {
                            ui.label("Display");
                            ui.label("handled by macOS").hover(
                                "macOS applies your display's colour profile, including any calibration",
                            );
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("Display");
                            egui::ComboBox::from_id_salt("display_space_pick")
                                .selected_text(d.display_space.label())
                                .show_ui(ui, |ui| {
                                    for s in color::OutputSpace::DISPLAYS {
                                        ui.selectable_value(&mut d.display_space, s, s.label());
                                    }
                                })
                                .response
                                .hover(
                                    "Sets your monitor's colour primaries so the preview shows colours correctly",
                                );
                        });
                    }
                }
                3 => {
                    theme::sub_label(ui, "1:1 view quality");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("view_demosaic_pick")
                            .selected_text(d.tier.label())
                            .show_ui(ui, |ui| {
                                for t in raw_decode::Demosaic::ALL {
                                    ui.selectable_value(&mut d.tier, t, t.label());
                                }
                            })
                            .response
                            // ⚠ The chosen tier's cost leads the hover; it used to be a line of
                            // 9-pt text beside the combo (`docs/DESIGN.md` §8).
                            .hover(format!(
                                "Sets the demosaic quality of the zoomed 1:1 view ({}: {})",
                                d.tier.label(),
                                view_quality_hint(d.tier)
                            ));
                    });

                    theme::sub_label(ui, "Preview quality");
                    ui.horizontal(|ui| {
                        let label = |t: Option<raw_decode::Demosaic>| match t {
                            None => "Same as 1:1 view".to_string(),
                            Some(t) => t.label().to_string(),
                        };
                        egui::ComboBox::from_id_salt("preview_demosaic_pick")
                            .selected_text(label(d.preview_demosaic))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut d.preview_demosaic, None, label(None));
                                for t in raw_decode::Demosaic::ALL {
                                    ui.selectable_value(
                                        &mut d.preview_demosaic,
                                        Some(t),
                                        t.label(),
                                    );
                                }
                            })
                            .response
                            .hover("Sets the quality of the previews built for edited photos");
                    });

                    theme::sub_label(ui, "Preview cache");
                    ui.horizontal(|ui| {
                        let store = rustroom_app::preview_store::PreviewStore::user_default(
                            d.preview_budget_mb as u64 * 1024 * 1024,
                        );
                        let bytes = *app
                            .preview_cache_bytes
                            .get_or_insert_with(|| store.size_bytes());
                        ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
                        if ui
                            .button("Clear")
                            .hover("Deletes the rendered previews, which are rebuilt when needed")
                            .clicked()
                        {
                            store.clear();
                            app.preview_cache_bytes = Some(0);
                        }
                    });
                    ui.add(
                        egui::Slider::new(&mut d.preview_budget_mb, 1024..=102400)
                            .step_by(1024.0)
                            .text("MB budget"),
                    )
                    .hover("Sets how much disk space rendered previews may use");

                    theme::sub_label(ui, "Rendering");
                    ui.checkbox(&mut d.gpu_tail, "Use the GPU")
                        .hover("Runs editing and export on the graphics card where that is faster");

                    theme::sub_label(ui, "Thumbnail cache");
                    ui.horizontal(|ui| {
                        let store = rustroom_app::thumb_store::ThumbStore::user_default(
                            d.thumb_budget_mb as u64 * 1024 * 1024,
                        );
                        let bytes = *app
                            .thumb_cache_bytes
                            .get_or_insert_with(|| store.size_bytes());
                        ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
                        if ui
                            .button("Clear")
                            .hover("Deletes the cached thumbnails, which are rebuilt when needed")
                            .clicked()
                        {
                            store.clear();
                            app.thumb_cache_bytes = Some(0);
                        }
                    });
                    ui.add(
                        egui::Slider::new(&mut d.thumb_budget_mb, 512..=51200)
                            .step_by(512.0)
                            .text("MB budget"),
                    )
                    .hover("Sets how much disk space thumbnails may use");

                    theme::sub_label(ui, "Export at once");
                    let cores = std::thread::available_parallelism()
                        .map(|n| n.get() as u32)
                        .unwrap_or(1);
                    ui.add(egui::Slider::new(&mut d.export_width, 1..=cores.max(1)).text("photos"))
                        .hover(
                            "Sets how many photos an export works on at once, trading memory for speed",
                        );

                    theme::sub_label(ui, "Resampling");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("resampler_pick")
                            .selected_text(d.resampler.label())
                            .show_ui(ui, |ui| {
                                for k in pipeline::Resampler::ALL {
                                    ui.selectable_value(&mut d.resampler, k, k.label());
                                }
                            })
                            .response
                            .hover(
                                "Sets how pixels are resampled when the photo is straightened, \
                                 corrected or scaled",
                            );
                    });

                    theme::sub_label(ui, "AI subject model");
                    egui::ComboBox::from_id_salt("ai_subject_pick")
                        .selected_text(d.ai_subject.label())
                        .show_ui(ui, |ui| {
                            for m in ai_mask::SubjectModel::ALL {
                                ui.selectable_value(&mut d.ai_subject, m, m.label());
                            }
                        })
                        .response
                        .hover(
                            "Chooses the network that finds Subject masks, BiRefNet lite being slower with crisper edges",
                        );
                }
                // ⚠⚠ An explicit index, **not** `_`. This arm was the catch-all, so adding a sixth
                // tab would have made Immich render Input's contents — it compiles and it looks
                // plausible.
                4 => {
                    theme::sub_label(ui, "Pan with");
                    egui::ComboBox::from_id_salt("pan_drag_pick")
                        .selected_text(d.pan_drag.label())
                        .show_ui(ui, |ui| {
                            for b in PanDrag::ALL {
                                ui.selectable_value(&mut d.pan_drag, b, b.label());
                            }
                        })
                        .response
                        .hover("Chooses the mouse drag that pans a zoomed photo");

                    theme::sub_label(ui, "Erase with");
                    let clash =
                        d.erase_drag.button(d.pan_drag).is_none() && d.erase_drag != EraseDrag::Off;
                    egui::ComboBox::from_id_salt("erase_drag_pick")
                        .selected_text(d.erase_drag.label())
                        .show_ui(ui, |ui| {
                            for b in EraseDrag::ALL {
                                ui.selectable_value(&mut d.erase_drag, b, b.label());
                            }
                        })
                        .response
                        .hover("Chooses the button to hold to erase while painting a brush mask");
                    // ⚠ A clash is a problem, not a description: it stays on the page.
                    if clash {
                        theme::error_hint(
                            ui,
                            &format!(
                                "Unavailable: {} is already bound to panning, which always wins. \
                                 Pick another button, or change what pans.",
                                d.erase_drag.label()
                            ),
                        );
                    }

                    theme::sub_label(ui, "Parametric curve");
                    egui::ComboBox::from_id_salt("parametric_drag_pick")
                        .selected_text(d.parametric_drag.label())
                        .show_ui(ui, |ui| {
                            for b in ParametricDrag::ALL {
                                ui.selectable_value(&mut d.parametric_drag, b, b.label());
                            }
                        })
                        .response
                        .hover("Lets you drag on the curve to adjust a region in Parametric mode");

                    theme::sub_label(ui, "Reorder a stack");
                    egui::ComboBox::from_id_salt("stack_drag_pick")
                        .selected_text(d.stack_drag.label())
                        .show_ui(ui, |ui| {
                            for b in rustroom_app::settings::StackDrag::ALL {
                                ui.selectable_value(&mut d.stack_drag, b, b.label());
                            }
                        })
                        .response
                        .hover("Chooses the drag that moves a photo within an unfurled stack");

                    theme::sub_label(ui, "Preview while dragging");
                    egui::ComboBox::from_id_salt("drag_quality_pick")
                        .selected_text(d.drag_quality.label())
                        .show_ui(ui, |ui| {
                            for q in DragQuality::ALL {
                                ui.selectable_value(&mut d.drag_quality, q, q.label());
                            }
                        })
                        .response
                        .hover(
                            "Renders at a lower resolution while you drag, for a smoother response",
                        );
                }
                _ => {
                    // ⚠⚠ **The master toggle, above everything and outside the grid.** This tab is the
                    // one place Immich is named when the integration is off — hiding the tab too would
                    // leave no way back on. Everything below is hidden while it is unticked, so the
                    // tab holds a switch and nothing else.
                    // ⚠ A switch rather than a tick box: enabling or disabling a whole integration
                    // is a decision, not one preference among many (user's call, 2026-09-24).
                    // ⚠ What Immich is, and what switching it off does, are the switch's hover
                    // (`docs/DESIGN.md` §8) — they were a paragraph above it.
                    crate::segmented::switch(ui, &mut d.immich_enabled, "Enable Immich")
                        .hover(
                            "Shows your Immich server's albums in the Library and lets exports upload to it",
                        );
                    // ⚠ An `if` rather than an early `return`: the match happens to be the last
                    // thing in this closure today, so a `return` works — and would silently skip
                    // anything a later change added after it.
                    if d.immich_enabled {
                        ui.add_space(GAP);
                        egui::Grid::new("settings_immich")
                        .num_columns(2)
                        .spacing([12.0, GAP])
                        .show(ui, |ui| {
                            // ⚠ Editing either field clears the last answer: a stale "Connected"
                            // sitting under credentials it was never tested against is worse than no
                            // message at all.
                            let before = (d.immich_url.clone(), d.immich_key.clone());

                            ui.label("Server");
                            ui.add(
                                egui::TextEdit::singleline(&mut d.immich_url)
                                    .hint_text("https://photos.example.com")
                                    .desired_width(260.0),
                            )
                            .hover(
                                "The address of your Immich server",
                            );
                            ui.end_row();

                            ui.label("API key");
                            ui.add(
                                egui::TextEdit::singleline(&mut d.immich_key)
                                    .password(true)
                                    .desired_width(260.0),
                            )
                            .hover(
                                "A read-only Immich API key, stored in plain text in settings.json",
                            );
                            ui.end_row();

                            // ⚠⚠ Its own field on purpose. Browsing, thumbnails and the rail run
                            // constantly and unattended; giving that traffic a write-capable key
                            // would make "browsing cannot write" depend on our code being right
                            // rather than on the credential. See `Settings::immich_upload_key`.
                            ui.label("Upload key");
                            ui.add(
                                egui::TextEdit::singleline(&mut d.immich_upload_key)
                                    .password(true)
                                    .desired_width(260.0),
                            )
                            .hover(
                                "An optional Immich API key allowed to upload, used only when an export uploads",
                            );
                            ui.end_row();

                            if before != (d.immich_url.clone(), d.immich_key.clone()) {
                                app.immich_test = None;
                            }

                            ui.label("");
                            ui.horizontal(|ui| {
                                // ⚠⚠ Tests what is **typed**, not what is saved — the whole point of
                                // a test button is checking a key before committing to it, so the job
                                // carries its own connection rather than reading `app.settings`.
                                if ui
                                    .button("Test")
                                    .hover("Checks the connection by asking the server for its version")
                                    .clicked()
                                {
                                    // ⚠ Cleared first, so the previous answer does not sit there
                                    // looking like this one while the request is in flight.
                                    app.immich_test = None;
                                    let typed = (!d.immich_url.trim().is_empty()
                                        && !d.immich_key.trim().is_empty())
                                    .then(|| rustroom_app::immich_source::Connection {
                                        url: d.immich_url.trim().to_string(),
                                        key: d.immich_key.trim().to_string(),
                                    });
                                    app.immich.request(rustroom_app::immich_worker::Job::About {
                                        conn: typed,
                                    });
                                }
                                // ⚠⚠ **Beside the button, not in the status line** — the status line
                                // is behind the modal, so the answer to a button you just pressed was
                                // invisible. Reported in use.
                                match &app.immich_test {
                                    Some(Ok(v)) => {
                                        ui.colored_label(
                                            ui.visuals().selection.bg_fill,
                                            format!("Connected to Immich {v}"),
                                        );
                                    }
                                    Some(Err(e)) => {
                                        ui.colored_label(ui.visuals().error_fg_color, e.as_str())
                                            .hover(e.as_str());
                                    }
                                    None => {}
                                }
                            });
                            ui.end_row();
                        });
                        ui.add_space(GAP);
                        theme::sub_label(ui, "Thumbnail cache");
                        ui.horizontal(|ui| {
                            let store = rustroom_app::thumb_store::ThumbStore::immich_default(
                                d.immich_thumb_budget_mb as u64 * 1024 * 1024,
                            );
                            // ⚠ `size_bytes()` walks the directory, so it is computed when the dialog
                            // opens and cached — never in a body, which runs sixty times a second.
                            let bytes = *app
                                .immich_thumb_cache_bytes
                                .get_or_insert_with(|| store.size_bytes());
                            ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
                            if ui
                            .button("Clear")
                            .hover(
                                "Deletes the cached Immich thumbnails, which are fetched again when needed",
                            )
                            .clicked()
                        {
                            store.clear();
                            app.immich_thumb_cache_bytes = Some(0);
                        }
                        });
                        ui.add(
                            egui::Slider::new(&mut d.immich_thumb_budget_mb, 512..=51200)
                                .step_by(512.0)
                                .text("MB budget"),
                        )
                        .hover("Sets how much disk space Immich thumbnails may use");
                    }
                }
            }
        },
    );

    if out.primary {
        d.apply(app, ctx);
        app.modal = Modal::None;
    } else if out.dismissed {
        app.modal = Modal::None;
    } else {
        app.settings_draft = Some(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    fn settle(ctx: &egui::Context, app: &mut RustroomApp) {
        let _ = test_ui::labelled_centres(ctx, Vec::new(), |_| show(app, ctx));
    }

    fn open_settings() -> (egui::Context, RustroomApp) {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.modal = Modal::Settings;
        (ctx, app)
    }

    /// ⚠ Nothing lands before Apply, and Cancel discards — the contract the buttons make.
    #[test]
    fn a_change_lands_on_apply_and_not_before() {
        let (ctx, mut app) = open_settings();
        assert_eq!(
            app.settings.default_rendering,
            edit_model::Rendering::Camera
        );
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Open new photos with the camera rendering", |_| {
            show(&mut app, &ctx)
        });
        assert_eq!(
            app.settings.default_rendering,
            edit_model::Rendering::Camera,
            "the change landed before Apply"
        );
        test_ui::click(&ctx, "Apply", |_| show(&mut app, &ctx));
        assert_eq!(
            app.settings.default_rendering,
            edit_model::Rendering::None,
            "Apply did not land the change"
        );
        assert!(
            matches!(app.modal, Modal::None),
            "Apply must close the dialog"
        );
        assert!(app.settings_draft.is_none(), "and drop the draft");
    }

    #[test]
    fn cancel_discards_the_draft() {
        let (ctx, mut app) = open_settings();
        assert!(app.settings.auto_lens_correction);
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Apply lens corrections to new photos", |_| {
            show(&mut app, &ctx)
        });
        test_ui::click(&ctx, "Cancel", |_| show(&mut app, &ctx));
        assert!(
            app.settings.auto_lens_correction,
            "Cancel let a change through"
        );
        assert!(matches!(app.modal, Modal::None));
        assert!(app.settings_draft.is_none());
        // Reopening starts from the live settings, not the abandoned draft.
        app.modal = Modal::Settings;
        settle(&ctx, &mut app);
        assert_eq!(app.settings_draft.as_ref().map(|d| d.auto_lens), Some(true));
    }

    /// ⚠⚠ **The failure this catches is a field added to the draft but forgotten in `apply`** — the
    /// dialog then looks right, accepts the typing, and silently discards it on Apply. A text field
    /// cannot be exercised by clicking a label, so the draft is set directly and Apply is clicked.
    ///
    /// ⚠ It also pins that the upload key does **not** ride on the browse connection: applying it
    /// alone must leave `immich_key` untouched.
    #[test]
    fn the_upload_key_survives_apply_and_leaves_the_browse_key_alone() {
        let (ctx, mut app) = open_settings();
        settle(&ctx, &mut app);
        assert!(
            app.settings.immich_upload_key.is_empty(),
            "the fixture must start with no upload key"
        );
        let browse_before = app.settings.immich_key.clone();

        app.settings_draft
            .as_mut()
            .expect("the draft exists once the dialog has been shown")
            .immich_upload_key = "pasted-upload-key".to_string();

        assert!(
            app.settings.immich_upload_key.is_empty(),
            "the draft must not land before Apply"
        );
        test_ui::click(&ctx, "Apply", |_| show(&mut app, &ctx));
        assert_eq!(
            app.settings.immich_upload_key, "pasted-upload-key",
            "Apply dropped the upload key"
        );
        assert_eq!(
            app.settings.immich_key, browse_before,
            "the browse key must not change when only the upload key does"
        );
    }

    /// M10.8b: where the panels sit is set in Appearance — the draft lands on Apply, and a new place
    /// for the filmstrip shows it.
    #[test]
    fn the_layout_lands_on_apply_and_a_new_strip_place_shows_it() {
        use rustroom_app::layout::{Side, StripPlace};
        let (ctx, mut app) = open_settings();
        app.show_filmstrip = false;
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Appearance", |_| show(&mut app, &ctx));
        let labels: Vec<String> = test_ui::nodes(&ctx, |_| show(&mut app, &ctx))
            .into_iter()
            .map(|n| n.label)
            .collect();
        assert!(
            labels.iter().any(|l| l == "Bottom"),
            "the rows are drawn: {labels:?}"
        );
        let d = app.settings_draft.as_mut().unwrap();
        d.layout.library_rail = Side::Right;
        d.layout.filmstrip = StripPlace::Left;
        assert_eq!(
            app.settings.layout.library_rail,
            Side::Left,
            "not before Apply"
        );
        test_ui::click(&ctx, "Apply", |_| show(&mut app, &ctx));
        assert_eq!(app.settings.layout.library_rail, Side::Right);
        assert_eq!(app.settings.layout.filmstrip, StripPlace::Left);
        assert!(app.show_filmstrip, "a new place shows the strip");
        let saved = app.settings_store.load();
        assert_eq!(saved.layout, app.settings.layout, "and it was saved");
    }
}
