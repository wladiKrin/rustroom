//! The one place a [`Command`] is performed.
//!
//! ⚠ Three input sources feed this — menu clicks, key presses and the rail — and each has a
//! different reason to exist. Routing them all through one exhaustive `match` is what stops a
//! menu item and its shortcut drifting apart, which is exactly what happened with the old Copy
//! button and Ctrl+C before they were made to share a code path.

use crate::{modals::Modal, AppView, LibraryMode, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::menu::Command;
use rustroom_app::view_region;

/// One click of Zoom in / Zoom out.
///
/// ⚠ A ratio, not an increment: zoom is multiplicative, so a fixed step would be enormous at
/// 0.2x and imperceptible at 8x.
const ZOOM_STEP: f32 = 1.25;

/// Whether the zoom keys act on the **pending** view: a photo still decoding with no session of
/// its own. ⚠⚠ A ghost session (`ghost.rs`) is the pending photo's own and is what the canvas
/// draws, so the keys must reach it — the first version checked `pending` first and ⌘1 moved a
/// view nobody could see (reported 2026-09-29).
fn pending_is_the_view(app: &RustroomApp) -> bool {
    match (&app.pending, &app.session) {
        (Some(p), Some(s)) => !(s.path == p.id.path && s.copy == p.id.copy),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

fn zoom_by(app: &mut RustroomApp, factor: f32) {
    if pending_is_the_view(app) {
        if let Some(p) = &mut app.pending {
            p.zoom = view_region::zoom_step(p.zoom, factor);
            if p.zoom <= view_region::MIN_ZOOM {
                p.pan = egui::Vec2::ZERO;
            }
        }
    } else if let Some(s) = &mut app.session {
        s.zoom = view_region::zoom_step(s.zoom, factor);
    }
}

pub fn execute(app: &mut RustroomApp, ctx: &egui::Context, cmd: Command) {
    match cmd {
        Command::OpenFile => app.open_dialog(ctx),
        Command::OpenFolder => app.open_folder(ctx),
        Command::OpenRecent(i) => {
            if let Some(dir) = app.settings.recent_folders.get(i).cloned() {
                app.open_folder_at(ctx, dir);
            }
        }
        Command::Import => app.open_import(),
        Command::BuildPreviews => app.start_preview_build(),
        Command::Export => {
            if !app.export_jobs().is_empty() {
                // Reset the working copy from the committed value, so a previously cancelled
                // edit does not linger.
                app.export_draft = app.settings.export.clone();
                app.modal = Modal::ExportSettings;
            }
        }
        Command::Settings => app.modal = Modal::Settings,
        Command::TakeSnapshot => app.take_snapshot(),
        Command::CreateVirtualCopy => app.create_virtual_copy(ctx),
        Command::DeleteVirtualCopy => app.delete_virtual_copy(ctx),
        Command::Undo => app.undo_action(ctx),
        Command::Redo => app.redo_action(ctx),
        Command::CopySettings => app.copy_settings(ctx),
        Command::PasteSettings => app.paste_last_groups(),
        Command::PasteSettingsWith => {
            if app.clipboard.is_some() {
                app.modal = Modal::PasteSettings;
            }
        }
        Command::SyncSelection => {
            let selected = app.library.as_ref().map_or(0, |l| l.selection.len());
            if selected > 1 {
                app.modal = Modal::SyncSettings;
            }
        }
        Command::ResetAdjustments => app.reset_adjustments(),
        // ⚠ `G` is Grid, not merely Library: it is the way back out of Compare and Survey, so
        // it must reset the mode as well as the module or the key would appear to do nothing
        // from the two views that most need an exit.
        Command::ShowLibrary => {
            app.view = AppView::Library;
            app.library_mode = LibraryMode::Grid;
        }
        Command::ShowDevelop => app.view = AppView::Develop,
        // ⚠ Both set the module too, so `C` from Develop lands somewhere rather than changing
        // a mode nothing is showing. ⚠ The candidate is deliberately **not** cleared: it is
        // resolved against the pool every frame, so a stale one is already harmless, and
        // clearing it would throw away the candidate you were mid-way through choosing every
        // time you dipped into Develop to look closer.
        Command::ShowCompare => {
            app.view = AppView::Library;
            app.library_mode = LibraryMode::Compare;
        }
        Command::ShowSurvey => {
            app.view = AppView::Library;
            app.library_mode = LibraryMode::Survey;
        }
        Command::ZoomIn => zoom_by(app, ZOOM_STEP),
        Command::ZoomOut => zoom_by(app, 1.0 / ZOOM_STEP),
        Command::FitView => {
            if pending_is_the_view(app) {
                if let Some(p) = &mut app.pending {
                    p.zoom = 1.0;
                    p.pan = egui::Vec2::ZERO;
                }
            } else if let Some(s) = &mut app.session {
                s.zoom = 1.0;
                s.pan = egui::Vec2::ZERO;
            }
        }
        Command::ZoomActual => {
            let pending_view = pending_is_the_view(app);
            if let Some(p) = app.pending.as_mut().filter(|_| pending_view) {
                // ⚠ 1:1 against the **preview** while the photo decodes: one preview pixel per
                // point. On an X-T4 the camera JPEG is 71% of the RAW, so this sits between the
                // half-size buffer's 1:1 and the true one; the session inherits the zoom, and
                // the preview keeps standing in there until the full decode lands.
                let long = app
                    .preview_textures
                    .get(&p.preview_request.key())
                    .map(|(t, _)| t.size()[0].max(t.size()[1]) as f32)
                    .or_else(|| {
                        p.placeholder
                            .as_ref()
                            .map(|t| t.size()[0].max(t.size()[1]) as f32)
                    });
                if let Some(long) = long {
                    if p.fit_long > 0.0 {
                        p.zoom = view_region::zoom_step(1.0, long / p.fit_long);
                        p.pan = egui::Vec2::ZERO;
                    }
                }
            } else if let Some(s) = &mut app.session {
                // ⚠ A ghost session's buffer is the 512-px thumbnail; its 1:1 is the preview's
                // (the camera JPEG standing in), else the thumbnail's — never "no zoom".
                let native_long = if s.buffer_is_ghost {
                    let key = rustroom_app::thumb_key::ThumbKey::new(
                        rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy),
                        &s.edits,
                    );
                    app.preview_textures
                        .get(&key)
                        .map(|(t, _)| t.size()[0].max(t.size()[1]))
                        .unwrap_or(s.buffer_long)
                } else {
                    s.buffer_long
                };
                // ⚠ 1:1 against the **editing buffer**, which is what this app's "native" rung
                // has always meant. `fit_image_size` is measured at the fit cap, so the buffer
                // is `buffer_long / fit_cap` times larger; dividing by the canvas's fit scale
                // turns that into a zoom factor.
                // ⚠ Through `Session::fit_cap`, not derived again here. Its own doc carries the
                // rule that every whole-image request must use it rather than `PREVIEW_MAX_DIM`,
                // and a second derivation is how the two come to disagree.
                let fit_cap = s.fit_cap() as f32;
                if fit_cap > 0.0 && s.fit_scale > 0.0 {
                    let target = (native_long as f32 / fit_cap) / s.fit_scale;
                    // ⚠ Through `zoom_step` from 1.0 rather than a separate clamp helper, so
                    // the MIN/MAX rule lives in exactly one place.
                    s.zoom = view_region::zoom_step(1.0, target);
                    s.pan = egui::Vec2::ZERO;
                }
            }
        }
        Command::ToggleBeforeAfter => {
            // ⚠ Must re-render: before/after now changes *what is rendered*, not which of two
            // textures is shown. Without this the toggle would appear to do nothing until some
            // other edit happened to request a frame — the M3.1a `draw_mask_overlay` bug.
            let tool = app.tool;
            if let Some(s) = &mut app.session {
                s.show_before = !s.show_before;
                s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
            }
        }
        Command::ToggleFilmstrip => app.show_filmstrip = !app.show_filmstrip,
        Command::TogglePanels => app.show_panels = !app.show_panels,
        Command::LightsOut => app.lights_out = (app.lights_out + 1) % 3,
        Command::ToggleInfoOverlay => app.show_info_overlay = !app.show_info_overlay,
        Command::ToggleClipping => {
            app.show_clipping = !app.show_clipping;
            // Built now from the frame already on screen; the next frame rebuilds it anyway.
            if let Some(s) = app.session.as_mut() {
                s.clip_texture = if app.show_clipping {
                    s.readout_bytes
                        .as_ref()
                        .and_then(|(b, w, h)| crate::shell::clipping_texture(ctx, b, *w, *h))
                } else {
                    None
                };
            }
        }
        // ⚠ Every mask's eye at once — the keyboard twin of the panel's "Show all / Hide all".
        Command::ToggleMaskOverlay => {
            if let Some(s) = &mut app.session {
                s.sync_mask_shown();
                let all = s.all_masks_shown();
                s.set_all_masks_shown(!all);
            }
        }
        // ⚠ Persisted immediately rather than on quit. This is one enum in a tiny file, the key
        // is pressed a handful of times a session, and the same `save_settings` the panel's
        // dropdown calls — one write path, so the two controls cannot disagree about what is
        // stored. Not the `apply_view_demosaic` hazard: this runs on a *keypress*, not in a body
        // that re-runs sixty times a second.
        Command::CycleCompositionGuide => {
            app.settings.composition_guide = app.settings.composition_guide.next();
            app.status = format!("Guide: {}", app.settings.composition_guide.label());
            app.save_settings();
        }
        // ⚠ Delegates rather than setting `app.tool` here: `select_tool` owns the re-render
        // decision, and duplicating it in this arm is how the rail and the menu drift apart.
        Command::SelectTool(t) => app.select_tool(t),
        Command::RotateLeft => app.rotate(false),
        Command::RotateRight => app.rotate(true),
        Command::SavePreset => {
            app.modal = Modal::SavePreset {
                name: String::new(),
                groups: EditGroups::default(),
            }
        }
        // ⚠ Guarded on an actual change, then re-rendered. A menu body runs every frame it is
        // open, so an unguarded request is the `apply_view_demosaic` mistake — and *not*
        // re-rendering at all is the `draw_mask_overlay` one, where the setting appears to do
        // nothing until some unrelated edit happens to ask for a frame.
        Command::SoftProof(space) => {
            let tool = app.tool;
            if let Some(s) = &mut app.session {
                if s.soft_proof != space {
                    s.soft_proof = space;
                    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
                }
            }
            // ⚠ Said out loud, because a proof can be genuinely subtle — and on this Mac the OS
            // colour-manages the window, so the *absolute* colour of the canvas is not a reliable
            // signal that anything happened. The status line is.
            app.status = match space {
                Some(sp) => format!("Soft proof: {}", sp.label()),
                None => "Soft proof off".to_string(),
            };
        }
        Command::GamutWarning => {
            let tool = app.tool;
            let mut on = false;
            if let Some(s) = &mut app.session {
                s.gamut_warning = !s.gamut_warning;
                on = s.gamut_warning;
                // ⚠ Only when a proof is on. With no proof the flag changes no pixel, so a
                // request here would spend a render — and, while proofing, a whole extra tail
                // run — to produce the identical frame.
                if s.soft_proof.is_some() {
                    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
                }
            }
            app.status = if on {
                "Gamut warning on".to_string()
            } else {
                "Gamut warning off".to_string()
            };
        }
        Command::RenderingIntent(intent) => {
            // ⚠ Guarded, for `SoftProof`'s reason above: a menu body runs every frame it is open.
            if app.settings.canvas_intent != intent {
                app.apply_canvas_intent(intent);
            }
            app.status = format!("Rendering intent: {}", intent.label());
        }
        Command::KeyboardShortcuts => app.modal = Modal::KeyboardShortcuts,
        Command::About => app.modal = Modal::About,
    }
    // ⚠ No catch-all arm above. A new `Command` must fail to compile here rather than becoming
    // a menu item that silently does nothing — that is the whole reason this enum exists.
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustroom_app::composition_guides::CompositionGuide;

    /// ⌘1 with a **ghost session** for the pending photo zooms the session — the view on
    /// screen — to the preview's 1:1, and leaves the pending view alone.
    #[test]
    fn the_zoom_keys_reach_a_ghost_session_not_the_pending_view_behind_it() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 8, 4);
        let id = {
            let s = app.session.as_mut().unwrap();
            s.buffer_is_ghost = true;
            s.buffer_is_full_res = false;
            rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy)
        };
        let req = app.preview_request(id.clone(), app.session.as_ref().unwrap().edits.clone());
        app.preview_textures.insert(
            req.key(),
            (
                crate::test_app::texture(&ctx, "ghost-preview", 64, 32),
                rustroom_app::preview_loader::PreviewSource::Camera,
            ),
            64 * 32 * 4,
        );
        app.pending = Some(crate::PendingOpen {
            id,
            placeholder: None,
            start_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
            seq: None,
            preview_request: req,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fit_long: 0.0,
        });
        execute(&mut app, &ctx, Command::ZoomActual);
        let s = app.session.as_ref().unwrap();
        // fit_cap of an 8-px buffer is 8, fit_scale 1: 64 px of preview at 1:1 is 8× fit.
        assert!(
            (s.zoom - 8.0).abs() < 1e-3,
            "the session did not zoom to the preview's 1:1: {}",
            s.zoom
        );
        assert_eq!(
            app.pending.as_ref().unwrap().zoom,
            1.0,
            "the pending view was touched"
        );
        execute(&mut app, &ctx, Command::FitView);
        assert_eq!(app.session.as_ref().unwrap().zoom, 1.0);
    }

    /// 2026-09-29: ⌘1 while a photo decodes zooms the pending preview to its own 1:1; ⌘0 and
    /// ⌘− act on it too. Before, the keys reached only the previous session behind it.
    #[test]
    fn the_zoom_keys_act_on_the_pending_preview_while_a_photo_decodes() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let id = rustroom_app::photo_id::PhotoId::original(std::path::PathBuf::from(
            "/test/pending.RAF",
        ));
        let req = app.preview_request(id.clone(), app.settings.initial_edits(None));
        app.preview_textures.insert(
            req.key(),
            (
                crate::test_app::texture(&ctx, "pending-preview", 1600, 800),
                rustroom_app::preview_loader::PreviewSource::Camera,
            ),
            1600 * 800 * 4,
        );
        app.pending = Some(crate::PendingOpen {
            id,
            placeholder: None,
            start_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
            seq: None,
            preview_request: req,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            // The canvas fitted the 1600-px preview to 400 points.
            fit_long: 400.0,
        });
        execute(&mut app, &ctx, Command::ZoomActual);
        let zoom = app.pending.as_ref().unwrap().zoom;
        assert!(
            (zoom - 4.0).abs() < 1e-3,
            "1:1 on a 1600-px preview fitted to 400 pt is 4×, got {zoom}"
        );
        execute(&mut app, &ctx, Command::ZoomOut);
        assert!(app.pending.as_ref().unwrap().zoom < 4.0);
        execute(&mut app, &ctx, Command::FitView);
        assert_eq!(app.pending.as_ref().unwrap().zoom, 1.0);
    }

    #[test]
    fn the_guide_key_cycles_the_setting_and_persists_it() {
        // ⚠ **Both**, in one test, because either alone is a half-feature: a cycle that does not
        // persist forgets your choice on every launch — the reason `PanelSections` is stored in
        // `Settings` at all — and a persist that does not cycle stores the same value forever.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        assert_eq!(app.settings.composition_guide, CompositionGuide::Off);

        execute(&mut app, &ctx, Command::CycleCompositionGuide);
        assert_eq!(app.settings.composition_guide, CompositionGuide::Thirds);
        execute(&mut app, &ctx, Command::CycleCompositionGuide);
        assert_eq!(
            app.settings.composition_guide,
            CompositionGuide::GoldenRatio
        );

        assert_eq!(
            app.settings_store.load().composition_guide,
            CompositionGuide::GoldenRatio,
            "the choice must reach the settings file, not only memory"
        );
    }

    #[test]
    fn the_guide_key_says_what_it_did() {
        // ⚠⚠ The key is **not** gated to the Develop view, so it can be pressed in the Library
        // where the crop overlay does not exist and nothing on screen would change. The status
        // line is the only feedback there, and without it the key reads as broken.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        execute(&mut app, &ctx, Command::CycleCompositionGuide);
        assert!(
            app.status.contains(CompositionGuide::Thirds.label()),
            "the status line must name the guide; got {:?}",
            app.status
        );
    }

    #[test]
    fn cycling_the_guide_is_not_a_photo_edit() {
        // ⚠ A view preference is not part of the photograph, so it must push no history entry —
        // `selected_mask`'s rule. Without this, a future implementation that routed the guide
        // through `Edits` would make Ctrl+Z undo a *guide change* instead of a real edit, and
        // nothing else here would notice.
        //
        // ⚠⚠ **The obvious sabotage for this is inert, and knowing why is the point.** Adding
        // `s.history.push(s.edits.clone())` to the arm leaves this green, because `History::push`
        // is a documented no-op when the value is unchanged — so "did it push?" is only visible
        // when the pushed value *differs*. The sabotage that fails it nudges `exposure` first.
        // Any real regression here does change the edits, which is why the test still earns its
        // place; a version asserting `history.len()` would not have been stronger.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        assert!(!app.session.as_ref().unwrap().history.can_undo());

        execute(&mut app, &ctx, Command::CycleCompositionGuide);

        assert!(
            !app.session.as_ref().unwrap().history.can_undo(),
            "changing the guide must not become an undo step"
        );
    }
}
