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
use rustroom_app::settings::{Accent, DragQuality, EraseDrag, PanDrag, Settings, ThemeChoice};

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
    export_width: u32,
    theme_choice: ThemeChoice,
    accent: Accent,
    pan_drag: PanDrag,
    erase_drag: EraseDrag,
    drag_quality: DragQuality,
    display_space: color::OutputSpace,
    immich_url: String,
    immich_key: String,
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
            export_width: s.export_concurrency,
            theme_choice: s.theme,
            accent: s.accent,
            pan_drag: s.pan_drag,
            erase_drag: s.erase_drag,
            drag_quality: s.drag_quality,
            display_space: s.display_space,
            immich_url: s.immich_url.clone(),
            immich_key: s.immich_key.clone(),
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
        if self.drag_quality != app.settings.drag_quality {
            app.settings.drag_quality = self.drag_quality;
            plain = true;
        }
        if self.auto_advance != app.settings.auto_advance {
            app.settings.auto_advance = self.auto_advance;
            plain = true;
        }
        if self.immich_url != app.settings.immich_url || self.immich_key != app.settings.immich_key
        {
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
        if plain {
            app.save_settings();
        }
        if self.theme_choice != app.settings.theme || self.accent != app.settings.accent {
            app.apply_appearance(self.theme_choice, self.accent, ctx);
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
                        let store = rustroom_app::thumb_store::ThumbStore::user_default();
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
                    egui::Grid::new("settings_immich")
                        .num_columns(2)
                        .spacing([12.0, GAP])
                        .show(ui, |ui| {
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

                            ui.label("");
                            // ⚠⚠ Tests what is **typed**, not what is saved — the whole point of a
                            // test button is checking a key before committing to it, so the job
                            // carries its own connection rather than reading `app.settings`.
                            if ui
                                .button("Test")
                                .on_hover_text("Ask the server for its version.")
                                .clicked()
                            {
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
                            ui.end_row();
                        });
                    crate::theme::hint(
                        ui,
                        "Albums appear in the Immich section of the left panel. Photos are \
                         downloaded when you open them, and your edits are saved beside the \
                         downloaded copy — nothing is written back to the server.",
                    );
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
}
