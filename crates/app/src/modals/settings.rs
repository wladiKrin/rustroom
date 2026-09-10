//! The Settings window.

use super::dialog;
use crate::decode::view_quality_hint;
use crate::{theme, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::settings::{Accent, DragQuality, EraseDrag, PanDrag, ThemeChoice};

/// The Settings window: preferences that survive a restart.
pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut confirm = app.settings.confirm_preset_delete;
    let mut write_xmp = app.settings.write_xmp;
    let mut auto_advance = app.settings.auto_advance;
    let mut camera_rendering = app.settings.default_rendering == edit_model::Rendering::Camera;
    let mut auto_lens = app.settings.auto_lens_correction;
    let mut tier = app.settings.view_demosaic;
    let mut gpu_hsl = app.settings.gpu_hsl;
    let mut resampler = app.settings.resampler;
    let mut theme_choice = app.settings.theme;
    let mut accent = app.settings.accent;
    let mut pan_drag = app.settings.pan_drag;
    let mut erase_drag = app.settings.erase_drag;
    let mut drag_quality = app.settings.drag_quality;
    let mut display_space = app.settings.display_space;
    // Copied out before the closure borrows `app`.
    let os_manages_colour = app.os_manages_colour;
    let mut close = false;

    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "settings",
        "Settings",
        theme::DialogSize::Standard,
        |ui| {
            ui.checkbox(&mut confirm, "Confirm before deleting a preset");
            ui.separator();

            // ⚠⚠ Both preferences existed since M5.4b and M3.2 and had NO control — they lived
            // only in the settings file, and this file's own handoff told the user to change one
            // "in Settings". Reported 2026-09-08. They apply to a photo with no sidecar; a photo
            // already edited keeps what it stored (`Settings::initial_edits`).
            ui.checkbox(
                &mut camera_rendering,
                "Open new photos with the camera rendering",
            );
            theme::hint(
                ui,
                "A RAW that has never been edited opens rendered the way the camera would. Off \
             opens it colorimetric, which looks flat. A photo you have already edited keeps \
             its own setting either way.",
            );
            ui.checkbox(&mut auto_lens, "Apply lens corrections to new photos");
            theme::hint(
                ui,
                "Distortion, chromatic aberration and vignetting from the lens profile, on a \
             RAW that has never been edited. A photo you have already edited keeps its own.",
            );
            ui.separator();

            ui.checkbox(&mut write_xmp, "Write XMP sidecars beside RAW files");
            theme::hint(
                ui,
                "Ratings, colour labels and keywords are written to a stem.xmp file next to \
             each RAW, which is what Lightroom and exiftool read. An existing sidecar is \
             merged into, never replaced, so develop settings written by another application \
             survive. Turning this off does not stop Rustroom reading sidecars somebody else \
             changed, and turning it back on updates each photo at its next rating or keyword \
             change rather than all at once.",
            );
            ui.separator();

            ui.checkbox(&mut auto_advance, "Move to the next photo after rating");
            theme::hint(
                ui,
                "In the Library grid a rating, a flag or a colour label steps past everything \
                 that was selected; in Compare it steps the bordered pane, exactly as the arrow \
                 keys do. Clearing a rating with 0 or a flag with U never moves on — a \
                 correction should leave you on the photo you are fixing.",
            );
            ui.separator();

            // ⚠ One grid so the two controls line up. The Display row below sits in a runtime
            // branch and gets its **own** grid: a `Grid` whose row count changes between frames
            // is an id-and-column-width problem for no gain.
            egui::Grid::new("settings_appearance")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Theme");
                    egui::ComboBox::from_id_salt("theme_pick")
                        .selected_text(theme_choice.label())
                        .show_ui(ui, |ui| {
                            for t in ThemeChoice::ALL {
                                ui.selectable_value(&mut theme_choice, t, t.label());
                            }
                        });
                    ui.end_row();

                    ui.label("Accent");
                    egui::ComboBox::from_id_salt("accent_pick")
                        .selected_text(accent.label())
                        .show_ui(ui, |ui| {
                            for a in Accent::ALL {
                                ui.selectable_value(&mut accent, a, a.label());
                            }
                        });
                    ui.end_row();
                });
            theme::hint(
                ui,
                "Dark is the default because a light surround changes how you judge exposure. \
             The accent marks active controls; Neutral turns it off.",
            );
            ui.separator();

            // ⚠ A **runtime** branch, not `#[cfg]`, so both halves compile on every platform. The
            // menu-bar work shipped a `#[cfg]`-gated UI branch that the developer never compiled and
            // Linux CI never exercised, leaving both halves unverified by whoever was looking.
            if os_manages_colour {
                // ⚠ A note, not a disabled dropdown. There is nothing to choose: the OS is using the
                // display's calibrated profile and following the window between monitors, which no
                // value here could improve on.
                ui.horizontal(|ui| {
                    ui.label("Display");
                    ui.label("handled by macOS");
                });
                theme::hint(
                    ui,
                    "Your display's own colour profile is applied by the system, including any \
                 calibration, and it follows the window if you move it to another monitor.",
                );
            } else {
                ui.horizontal(|ui| {
                    ui.label("Display");
                    egui::ComboBox::from_id_salt("display_space_pick")
                        .selected_text(display_space.label())
                        .show_ui(ui, |ui| {
                            // ⚠ `DISPLAYS`, not `ALL`: no ProPhoto monitor exists, so offering it is
                            // a pure footgun.
                            for s in color::OutputSpace::DISPLAYS {
                                ui.selectable_value(&mut display_space, s, s.label());
                            }
                        });
                });
                theme::hint(
                    ui,
                    "The primaries of your monitor. Most are sRGB; recent Apple displays and many \
                 laptops are Display P3. Affects only what you see — never an export, and never \
                 the histogram.",
                );
            }
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("1:1 view quality");
                egui::ComboBox::from_id_salt("view_demosaic_pick")
                    .selected_text(tier.label())
                    .show_ui(ui, |ui| {
                        for d in raw_decode::Demosaic::ALL {
                            ui.selectable_value(&mut tier, d, d.label());
                        }
                    });
                theme::hint(ui, view_quality_hint(tier));
            });
            // ⚠ Not decoration. Unlike the export setting, this cost is *recurring*: the
            // full-resolution decode is eager, so a slower tier is paid on every photo you
            // stop on. The dialog is the only place a user would find that out.
            theme::hint(
                ui,
                "RAW only, and separate from the export setting. Sets the demosaic used \
             for the zoomed view. A slower tier costs that much extra on every photo \
             you stop on, not just on export — the full-resolution decode starts on \
             its own. The default is Better: Fast is 2.4x quicker but visibly smears \
             fine foliage, and Best is indistinguishable from Better at 4x. \
             Changing this re-decodes the open photo.",
            );

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("HSL");
                ui.checkbox(&mut gpu_hsl, "Use the GPU");
            });
            theme::hint(
                ui,
                "Runs the global HSL sliders on the graphics card. Off by default: \
             the CPU is the reference, and the two agree to about a hundredth of an \
             8-bit level, so the picture should not change. ⚠ Does not apply to HSL \
             inside a mask.",
            );

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Thumbnail cache");
                // ⚠ Read once, on the first frame the dialog is open. `get_or_insert_with` is
                // what makes that "once" rather than "every frame" — walking the directory sixty
                // times a second is the `apply_view_demosaic` mistake in another costume.
                let store = rustroom_app::thumb_store::ThumbStore::user_default();
                let bytes = *app
                    .thumb_cache_bytes
                    .get_or_insert_with(|| store.size_bytes());
                ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
                // ⚠ No confirmation, deliberately. This destroys nothing the app cannot rebuild
                // — the distinction that already separates deleting a collection from forgetting
                // a folder, which does take ratings and keywords with it.
                if ui.button("Clear").clicked() {
                    store.clear();
                    app.thumb_cache_bytes = Some(0);
                }
            });
            theme::hint(
                ui,
                "Previews already extracted from your photos, so a folder you have \
             opened before fills in at once instead of re-reading every file. \
             Clearing costs nothing but the time to build it again.",
            );

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Resampling");
                egui::ComboBox::from_id_salt("resampler_pick")
                    .selected_text(resampler.label())
                    .show_ui(ui, |ui| {
                        for k in pipeline::Resampler::ALL {
                            ui.selectable_value(&mut resampler, k, k.label());
                        }
                    });
                // ⚠ Per-kernel, from `Resampler::hint`. The names are accurate but mean nothing
                // to a photographer on their own, so the trade has to be spelled out.
                theme::hint(ui, resampler.hint());
            });
            theme::hint(
                ui,
                "How pixels are read when straightening, correcting perspective or applying a \
             lens profile — the only three things that resample. One setting for the preview \
             and the export, so what you judge is what you get. Sharper kernels recover detail \
             a bilinear pass throws away and ring slightly on hard edges; bilinear is the only \
             one that provably cannot, and is what Rustroom used before.",
            );

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Pan with");
                egui::ComboBox::from_id_salt("pan_drag_pick")
                    .selected_text(pan_drag.label())
                    .show_ui(ui, |ui| {
                        for b in PanDrag::ALL {
                            ui.selectable_value(&mut pan_drag, b, b.label());
                        }
                    });
            });
            theme::hint(
                ui,
                "Scrolling and two-finger swipe always pan — this is the extra drag gesture, \
             for when a middle button is missing or broken.",
            );

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Erase with");
                egui::ComboBox::from_id_salt("erase_drag_pick")
                    .selected_text(erase_drag.label())
                    .show_ui(ui, |ui| {
                        for b in EraseDrag::ALL {
                            ui.selectable_value(&mut erase_drag, b, b.label());
                        }
                    });
            });
            // ⚠ The hint names the *configured* pan binding rather than saying "right-drag"
            // literally, or it becomes a lie the moment that setting changes — `PanDrag::label`'s
            // reason, and the same trap the toolbar hint was fixed for.
            theme::hint(
                ui,
                &if erase_drag.button(pan_drag).is_none() && erase_drag != EraseDrag::Off {
                    format!(
                        "Unavailable: {} is already bound to panning, which always wins. \
                         Pick another button, or change what pans.",
                        erase_drag.label()
                    )
                } else {
                    "Hold this button while painting a brush mask to erase instead, for the \
                     duration of the stroke. With Erase already ticked it paints."
                        .to_string()
                },
            );

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Preview while dragging");
                egui::ComboBox::from_id_salt("drag_quality_pick")
                    .selected_text(drag_quality.label())
                    .show_ui(ui, |ui| {
                        for q in DragQuality::ALL {
                            ui.selectable_value(&mut drag_quality, q, q.label());
                        }
                    });
            });
            theme::hint(
                ui,
                "Renders at a lower resolution while a slider or the canvas is being dragged, \
                 then at full quality the moment you let go. Each step is about a quarter as \
                 many pixels. Sharpening is the one adjustment that reads weakly while dragging \
                 — judge it at 1:1, or choose Full quality.",
            );

            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        },
    );
    close |= dismissed;

    if confirm != app.settings.confirm_preset_delete {
        app.settings.confirm_preset_delete = confirm;
        app.save_settings();
    }
    // ⚠ Change-guarded like its neighbours: this body runs every frame the dialog is open, and
    // an unguarded write would rewrite the settings file sixty times a second.
    if write_xmp != app.settings.write_xmp {
        app.settings.write_xmp = write_xmp;
        app.save_settings();
    }
    // ⚠ Change-guarded for the same reason. Neither needs a re-decode or a re-render: both are
    // read at the next photo *open*, by `initial_edits`, and nothing on screen changes now.
    let want_rendering = if camera_rendering {
        edit_model::Rendering::Camera
    } else {
        edit_model::Rendering::None
    };
    if want_rendering != app.settings.default_rendering {
        app.settings.default_rendering = want_rendering;
        app.save_settings();
    }
    if auto_lens != app.settings.auto_lens_correction {
        app.settings.auto_lens_correction = auto_lens;
        app.save_settings();
    }
    // ⚠ The inequality is what stops the worker being rebuilt and a decode fired on
    // *every frame* the dialog is open — `settings_modal` runs once per frame.
    // ⚠ The same one-frame rule: `show` runs once per frame the dialog is open, so an
    // unguarded re-apply would rebuild both themes' styles sixty times a second.
    if theme_choice != app.settings.theme || accent != app.settings.accent {
        app.apply_appearance(theme_choice, accent, ctx);
    }
    if tier != app.settings.view_demosaic {
        app.apply_view_demosaic(tier, ctx);
    }
    // ⚠ Guarded like its neighbours: `show` runs every frame the dialog is open, and this one
    // re-renders — the `apply_view_demosaic` rule.
    if gpu_hsl != app.settings.gpu_hsl {
        app.apply_gpu_hsl(gpu_hsl);
    }
    // ⚠ Guarded like its neighbours: `show` runs every frame the dialog is open, and this one
    // rebuilds the thumbnail loader and re-renders. Unguarded it would reload the whole folder's
    // thumbnails sixty times a second — the `apply_view_demosaic` rule.
    if resampler != app.settings.resampler {
        app.apply_resampler(resampler);
    }
    // ⚠ Guarded like its neighbours: `show` runs every frame the dialog is open, and this one
    // rebuilds the thumbnail loader and re-renders, so an unguarded call would reload the whole
    // folder's thumbnails sixty times a second.
    if display_space != app.settings.display_space {
        app.apply_display_space(display_space, ctx);
    }
    // ⚠ A change guard like its neighbours, though for a weaker reason: this has no side
    // effect beyond the write, because the canvas reads `settings.pan_drag` afresh every
    // frame — no worker rebuild, no re-decode. The guard is here so the settings file is not
    // rewritten on every frame the dialog is open.
    if pan_drag != app.settings.pan_drag {
        app.settings.pan_drag = pan_drag;
        app.save_settings();
    }
    // ⚠ A change guard like its neighbours: the canvas reads `settings.erase_drag` afresh every
    // frame, so this has no side effect beyond keeping the file from being rewritten each frame.
    if erase_drag != app.settings.erase_drag {
        app.settings.erase_drag = erase_drag;
        app.save_settings();
    }
    // ⚠ A change guard like its neighbours, and for `pan_drag`'s weaker reason: this has no side
    // effect beyond the write, because the canvas reads `settings.drag_quality` afresh every
    // frame — no worker rebuild, no re-decode. The guard is here so the settings file is not
    // rewritten on every frame the dialog is open.
    if drag_quality != app.settings.drag_quality {
        app.settings.drag_quality = drag_quality;
        app.save_settings();
    }
    // ⚠ A change guard like its neighbours, and for `pan_drag`'s weaker reason: this has no side
    // effect beyond the write, because both views read `settings.auto_advance` afresh every
    // frame. The guard is here so the settings file is not rewritten on every frame the dialog
    // is open.
    if auto_advance != app.settings.auto_advance {
        app.settings.auto_advance = auto_advance;
        app.save_settings();
    }
    if close {
        app.modal = Modal::None;
        // ⚠ Cleared on close so the next open re-reads the size rather than showing a stale one
        // — in particular after a Clear, or after a folder has been browsed in between.
        app.thumb_cache_bytes = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    /// One layout frame with no input, so the modal's resize and scroll settle — the
    /// `modals::info` tests' rule: the Close button's centre on the very first frame is not
    /// where it is drawn from the second on, and a click aimed from it lands beside the widget.
    fn settle(ctx: &egui::Context, app: &mut RustroomApp) {
        let _ = test_ui::labelled_centres(ctx, Vec::new(), |_| show(app, ctx));
    }

    fn open_settings() -> (egui::Context, RustroomApp) {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.modal = Modal::Settings;
        (ctx, app)
    }

    /// ⚠⚠ The report: *"I don't see the option in the settings … to turn off the automatic
    /// camera rendering."* There was none. This is the control existing and reaching the
    /// preference — the half a test on `initial_edits` cannot see.
    #[test]
    fn the_camera_rendering_preference_has_a_control_that_reaches_it() {
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
            edit_model::Rendering::None,
            "the click never reached the preference"
        );
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Open new photos with the camera rendering", |_| {
            show(&mut app, &ctx)
        });
        assert_eq!(
            app.settings.default_rendering,
            edit_model::Rendering::Camera
        );
    }

    #[test]
    fn the_lens_correction_preference_has_a_control_that_reaches_it() {
        let (ctx, mut app) = open_settings();
        assert!(app.settings.auto_lens_correction);
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Apply lens corrections to new photos", |_| {
            show(&mut app, &ctx)
        });
        assert!(
            !app.settings.auto_lens_correction,
            "the click never reached the preference"
        );
    }
}
