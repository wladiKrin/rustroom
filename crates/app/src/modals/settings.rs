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
    drag_quality: DragQuality,
    display_space: color::OutputSpace,
    stacking: rustroom_app::stack::Stacking,
    immich_enabled: bool,
    immich_url: String,
    immich_key: String,
    immich_upload_key: String,
    thumb_budget_mb: u32,
    immich_thumb_budget_mb: u32,
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
            if crate::segmented::chip(ui, &mut on, &ext.to_uppercase()).changed() {
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
            drag_quality: s.drag_quality,
            display_space: s.display_space,
            stacking: s.stacking.clone(),
            immich_enabled: s.immich_enabled,
            immich_url: s.immich_url.clone(),
            immich_key: s.immich_key.clone(),
            immich_upload_key: s.immich_upload_key.clone(),
            thumb_budget_mb: s.thumb_budget_mb,
            immich_thumb_budget_mb: s.immich_thumb_budget_mb,
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
        if self.theme_choice != app.settings.theme || self.accent != app.settings.accent {
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
        Footer::confirm("Apply", true, "Cancel"),
        |ui| {
            let tab = tabs(
                ui,
                egui::Id::new("settings_tab"),
                &[
                    "Behaviour",
                    "Appearance",
                    "Display",
                    "Performance",
                    "Input",
                    "Immich",
                ],
            );
            ui.spacing_mut().item_spacing.y = GAP;
            match tab {
                0 => {
                    ui.checkbox(&mut d.confirm, "Confirm before deleting a preset")
                        .on_hover_text(
                            "A deleted preset can be brought back with Undo either way.",
                        );
                    ui.checkbox(
                        &mut d.camera_rendering,
                        "Open new photos with the camera rendering",
                    )
                    .on_hover_text(
                        "A RAW that has never been edited opens rendered the way the camera \
                         would. Off opens it colorimetric, which looks flat. A photo you have \
                         already edited keeps its own setting either way.",
                    );
                    ui.checkbox(&mut d.auto_lens, "Apply lens corrections to new photos")
                        .on_hover_text(
                            "Distortion, chromatic aberration and vignetting from the lens \
                             profile, on a RAW that has never been edited. A photo you have \
                             already edited keeps its own.",
                        );
                    ui.checkbox(&mut d.write_xmp, "Write XMP sidecars beside RAW files")
                        .on_hover_text(
                            "Ratings, colour labels and keywords are written to a stem.xmp file \
                             next to each RAW, which is what Lightroom and exiftool read. An \
                             existing sidecar is merged into, never replaced, so develop settings \
                             written by another application survive. Turning this off does not \
                             stop Rustroom reading sidecars somebody else changed, and turning it \
                             back on updates each photo at its next rating or keyword change \
                             rather than all at once.",
                        );
                    ui.checkbox(&mut d.auto_advance, "Move to the next photo after rating")
                        .on_hover_text(
                            "In the Library grid a rating, a flag or a colour label steps past \
                             everything that was selected; in Compare it steps the bordered pane, \
                             exactly as the arrow keys do. Clearing a rating with 0 or a flag \
                             with U never moves on — a correction should leave you on the photo \
                             you are fixing.",
                        );
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
                                .on_hover_text(
                                    "Dark is the default because a light surround changes how \
                                     you judge exposure.",
                                );
                            ui.end_row();

                            ui.label("Accent");
                            egui::ComboBox::from_id_salt("accent_pick")
                                .selected_text(d.accent.label())
                                .show_ui(ui, |ui| {
                                    for a in Accent::ALL {
                                        ui.selectable_value(&mut d.accent, a, a.label());
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "The accent marks active controls; Neutral turns it off.",
                                );
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
                                .on_hover_text(
                                    "Everything larger or smaller, text and controls alike — \
                                     for a big screen sat far away, or small type.",
                                );
                            ui.end_row();

                            // ⚠ In Appearance because stacking is a **browsing** decision, the same
                            // reasoning that keeps the format filter out of the Develop toolbar.
                            ui.label("Stacking");
                            ui.vertical(|ui| {
                                ui.checkbox(
                                    &mut d.stacking.enabled,
                                    "Group files that share a name",
                                )
                                .on_hover_text(
                                    "A RAW and its JPEG become one cell in the Library, with a \
                                     badge showing how many files it holds. Click the badge to see \
                                     them separately. Nothing is hidden or changed on disk — each \
                                     file keeps its own edits and its own rating.",
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
                                            .on_hover_text(
                                                "Which file a collapsed stack shows and opens. \
                                                 With “Edited file, otherwise RAW” a pair you have \
                                                 worked on shows the file you worked on; if both are \
                                                 edited it shows the RAW.",
                                            );
                                    });
                                    // ⚠⚠ The chips are the *participating* formats, and
                                    // `FormatFilter` is a set of **hidden** ones — so a lit chip
                                    // means "takes part". Reused for the jpg/jpeg aliasing.
                                    stack_format_chips(ui, &mut d.stacking);
                                });
                            });
                            ui.end_row();
                        });
                }
                2 => {
                    if os_manages_colour {
                        ui.horizontal(|ui| {
                            ui.label("Display");
                            ui.label("handled by macOS").on_hover_text(
                                "Your display's own colour profile is applied by the system, \
                                 including any calibration, and it follows the window if you \
                                 move it to another monitor.",
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
                                .on_hover_text(
                                    "The primaries of your monitor. Most are sRGB; recent Apple \
                                     displays and many laptops are Display P3. Affects only what \
                                     you see — never an export, and never the histogram.",
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
                            .on_hover_text(
                                "RAW only, and separate from the export setting. Sets the demosaic \
                                 used for the zoomed view. A slower tier costs that much extra on \
                                 every photo you stop on, not just on export — the full-resolution \
                                 decode starts on its own. The default is Better: Fast is 2.4x \
                                 quicker but visibly smears fine foliage, and Best is \
                                 indistinguishable from Better at 4x. Changing this re-decodes the \
                                 open photo.",
                            );
                        theme::hint(ui, view_quality_hint(d.tier));
                    });

                    theme::sub_label(ui, "Rendering");
                    ui.checkbox(&mut d.gpu_tail, "Use the GPU").on_hover_text(
                        "Runs develop work on the graphics card wherever it pays — while you \
                         edit and while you export, inside a mask as well as outside one. The CPU \
                         is the reference, and the picture should not change. ⚠ Work is only \
                         moved across when what it saves is worth the transfer, so a lightly \
                         edited photograph deliberately stays on the CPU. The first render after \
                         switching it on spends about a sixth of a second compiling shaders.",
                    );

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
                            .on_hover_text(
                                "Previews already extracted from your photos, so a folder you \
                                 have opened before fills in at once instead of re-reading every \
                                 file. Clearing costs nothing but the time to build it again. \
                                 Takes effect at once, not on Apply.",
                            )
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
                    .on_hover_text(
                        "How much disk the thumbnail cache may use. The least recently seen \
                         entries are deleted when it is exceeded. ⚠ Takes effect at the next folder \
                         open, not on Apply — rebuilding the loader now would throw away the open \
                         folder's thumbnails.",
                    );

                    theme::sub_label(ui, "Export at once");
                    let cores = std::thread::available_parallelism()
                        .map(|n| n.get() as u32)
                        .unwrap_or(1);
                    ui.add(egui::Slider::new(&mut d.export_width, 1..=cores.max(1)).text("photos"))
                        .on_hover_text(
                            "How many photos a batch export works on at once. Decoding a RAW uses \
                             one core, so several at a time is much faster — measured at about 3x \
                             at four, and 4.5x at eight. ⚠ Each photo in flight holds its decoded \
                             pixels in memory: roughly 300 MB on a 26-megapixel camera, so four is \
                             about 1.2 GB. Turn it down if exporting makes the machine swap. \
                             Applies to the next export.",
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
                            .on_hover_text(
                                "How pixels are read when straightening, correcting perspective \
                                 or applying a lens profile — the only three things that resample. \
                                 One setting for the preview and the export, so what you judge is \
                                 what you get. Sharper kernels recover detail a bilinear pass \
                                 throws away and ring slightly on hard edges; bilinear is the only \
                                 one that provably cannot, and is what Rustroom used before.",
                            );
                        theme::hint(ui, d.resampler.hint());
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
                        .on_hover_text(
                            "Which network finds a Subject mask. IS-Net takes about 2 s a photo; \
                             BiRefNet lite about 25 s, with a crisper edge. Each is downloaded the \
                             first time it is used, and every answer is kept.",
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
                        .on_hover_text(
                            "Scrolling and two-finger swipe always pan — this is the extra drag \
                             gesture, for when a middle button is missing or broken.",
                        );

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
                        .on_hover_text(
                            "Hold this button while painting a brush mask to erase instead, for \
                             the duration of the stroke. With Erase already ticked it paints.",
                        );
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
                        .on_hover_text(
                            "In the tone curve's Parametric mode, dragging up or down on the curve \
                             adjusts the region under the pointer. Off leaves the four sliders.",
                        );

                    theme::sub_label(ui, "Preview while dragging");
                    egui::ComboBox::from_id_salt("drag_quality_pick")
                        .selected_text(d.drag_quality.label())
                        .show_ui(ui, |ui| {
                            for q in DragQuality::ALL {
                                ui.selectable_value(&mut d.drag_quality, q, q.label());
                            }
                        })
                        .response
                        .on_hover_text(
                            "Renders at a lower resolution while a slider or the canvas is being \
                             dragged, then at full quality the moment you let go. Each step is \
                             about a quarter as many pixels. Sharpening is the one adjustment \
                             that reads weakly while dragging — judge it at 1:1, or choose Full \
                             quality.",
                        );
                }
                _ => {
                    // ⚠⚠ **The master toggle, above everything and outside the grid.** This tab is the
                    // one place Immich is named when the integration is off — hiding the tab too would
                    // leave no way back on. Everything below is hidden while it is unticked, so the
                    // tab holds a switch and nothing else.
                    // ⚠ A switch rather than a tick box: enabling or disabling a whole integration
                    // is a decision, not one preference among many (user's call, 2026-09-24).
                    theme::hint(
                        ui,
                        "Immich is a self-hosted photo server (immich.app). With a server and an API \
                         key, its albums appear in the Library and exports can upload to it.",
                    );
                    crate::segmented::switch(ui, &mut d.immich_enabled, "Enable Immich")
                        .on_hover_text(
                            "When off, Immich disappears from the rest of Rustroom — the left \
                             panel, the export dialog and the toolbar. Nothing is deleted: photos \
                             you have already downloaded stay under “immich (downloaded)” in \
                             Folders, with their edits and marks, and everything comes back when \
                             you switch it on again.",
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
                            .on_hover_text(
                                "The base URL of your Immich server. The /api suffix is added for \
                                 you, and http:// is accepted for a server on your own network.",
                            );
                            ui.end_row();

                            ui.label("API key");
                            ui.add(
                                egui::TextEdit::singleline(&mut d.immich_key)
                                    .password(true)
                                    .desired_width(260.0),
                            )
                            .on_hover_text(
                                "Paste a read-only key — asset.read, asset.view, asset.download \
                                 and album.read are all it needs. ⚠ It is stored in plain text in \
                                 settings.json, so a key that cannot write cannot damage your \
                                 library if that file is ever read by somebody else.",
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
                            .on_hover_text(
                                "Optional, and used only when an export uploads. Paste a key \
                                 granted asset.upload — that grant is separate from asset.update, \
                                 so it cannot modify or delete photos already on the server. Leave \
                                 it empty to keep uploading switched off. ⚠ Stored in plain text in \
                                 settings.json, like the key above.",
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
                                    .on_hover_text("Ask the server for its version.")
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
                                            .on_hover_text(e.as_str());
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
                            .on_hover_text(
                                "Thumbnails downloaded from Immich, so an album you have opened \
                                 before fills in without asking the server again. Clearing costs \
                                 nothing but the time to fetch them again. Takes effect at once, \
                                 not on Apply.",
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
                    .on_hover_text(
                        "How much disk Immich thumbnails may use. Separate from the local cache's \
                         budget, so neither can evict the other's entries.",
                    );

                        crate::theme::hint(
                            ui,
                            "Albums appear in the Immich section of the left panel. Photos are \
                         downloaded when you open them, and your edits are saved beside the \
                         downloaded copy — nothing is written back to the server.",
                        );
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
}
