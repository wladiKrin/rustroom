//! Compare (X|Y) — two photos side by side, a **reference** and a **candidate**.
//!
//! The reference is the Library's active photo: the frame you are currently keeping. The
//! candidate is the one being judged against it, moved with the arrow keys and promoted to
//! reference with Enter when it wins.
//!
//! ⚠⚠ **The culling keys act on ONE pane — whichever carries the border.** A key that rated both
//! would answer nothing at all. The border starts on the candidate, because that is the pane the
//! arrows move and the question you arrived to ask; Enter or a click moves it to the other.
//!
//! ⚠ Enter used to *promote* the candidate to reference, which swapped the two panes. It was
//! taken out (user's call, 2026-09-02): a view whose whole job is holding two frames still while
//! you look between them must not rearrange itself under you.
//!
//! ⚠ Which photos the candidate moves through is [`rustroom_app::compare_pair::pool`]'s one
//! decision, and it is confined there.

use crate::develop::canvas::navigation::pan_delta;
use crate::multi;
use crate::{LibraryMode, RustroomApp};
use eframe::egui;
use rustroom_app::compare_pair;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::thumb_loader::ThumbRequest;
use rustroom_app::tile_layout;

/// What one frame of Compare decided, applied after the closure that borrows `app.library`.
///
/// ⚠ The deferred-action shape every view in this crate uses: the draw closure holds
/// `&mut app.library`, and all four of these need `&mut RustroomApp`.
#[derive(Default)]
struct Outcome {
    /// Open this row in Develop.
    open: Option<usize>,
    /// Mark this photo.
    mark: Option<(PhotoId, rustroom_app::marks::MarkAction)>,
    /// Remember this candidate. `Some(None)` clears it; `None` leaves it alone.
    candidate: Option<Option<PhotoId>>,
    /// Move the blue border to this pane.
    focus: Option<usize>,
    /// Make this row the reference — the arrows moving the *left* pane.
    reference: Option<usize>,
    /// Leave for the grid.
    to_grid: bool,
}

/// The line under the two photos.
///
/// ⚠⚠ Named rather than inline **so the glyph probe can reach it**. Its arrows shipped as tofu
/// boxes because they were chosen without checking — the fifth time this project has done that,
/// after the rail's four glyphs, the mark badges, the keyword checkmarks and the perspective
/// arrows. A literal inside a `RichText::new(..)` call cannot be probed by a test.
///
/// ⚠ `<-` and `->` as ASCII, not `←`/`→`: neither arrow is in any bundled font.
const INSTRUCTION: &str = "<- -> move the bordered photo · Enter or a click moves the border · \
                           digits, P, X and U rate it · scroll pans and pinch zooms, each pane \
                           on its own";

/// The photo the reference must step around: the candidate when there is one, else itself.
///
/// ⚠ `step`'s first "held still" argument is *excluded* from the walk, so passing the reference
/// there while stepping the reference would exclude the very photo being moved and skip a slot.
fn cand_or(candidate: Option<usize>, fallback: usize) -> usize {
    candidate.unwrap_or(fallback)
}

impl RustroomApp {
    /// Drop Compare's panes whenever Compare is not the view on screen.
    ///
    /// ⚠⚠ **Each pane holds a ~78 MB decode and a render worker**, so keeping them for a view
    /// nobody is looking at would leave ~350 MB and two threads behind after a single visit.
    ///
    /// ⚠ One edge check, not a release at each exit: `to_grid`, Escape, `G`, the segmented
    /// switch and `open_entry` all leave Compare, and one of the five would be forgotten. The
    /// per-view status line is cleared the same way and for the same reason.
    pub(crate) fn release_compare_panes(&mut self) {
        if self.library_mode != LibraryMode::Compare || self.view != crate::AppView::Library {
            self.compare_view.clear();
        }
    }
}

/// ⚠⚠ **The pair is derived ONCE, here, and both the panes and the drawing use it.** Resolving
/// it a second time inside the draw closure would be two computations of one fact — and while
/// they read the same `lib` in the same frame and so agree today, that is exactly the shape this
/// project has been bitten by three times (`render_max_dim`, `fit_image_size`, the crop feedback
/// loop). Deriving it once makes the disagreement unrepresentable rather than merely unlikely.
struct Plan {
    reference: usize,
    candidate: Option<usize>,
    /// The photos to build panes for, with the edits to render each through.
    want: Vec<(PhotoId, edit_model::Edits)>,
}

/// The two photos Compare is about to show, with the edits to render them through.
///
/// ⚠ Resolved from `app` read-only and *before* the panel is drawn, because building a pane needs
/// the decode cache and the worker while the draw closure holds `app.library`.
///
/// ⚠ The **open** photo's edits come from the live session rather than from the row: the sidecar
/// is stale exactly while you are editing, which is `edit-accurate thumbnails`' rule and matters
/// here for the same reason — you arrive in Compare straight from Develop.
fn plan(app: &RustroomApp, stored: &Option<PhotoId>) -> Option<Plan> {
    let lib = app.library.as_ref()?;
    // ⚠ Falls back to the first visible row rather than refusing to draw. Arriving in Compare
    // with nothing selected is ordinary — pressing `C` is how you get here — and an empty state
    // you can only leave by going back to the grid is worse than a sensible starting pair.
    let reference = lib
        .selection
        .active()
        .or_else(|| lib.visible.first().copied())?;
    let selected: Vec<usize> = lib.selection.iter().collect();
    let pool = compare_pair::pool(&lib.visible, &selected);
    // ⚠ Resolved from the stored **id**, so a candidate whose row moved under a sort, was
    // filtered away, or has just been promoted falls back rather than showing the wrong
    // photograph. See `RustroomApp::compare_candidate`.
    let stored_row = stored
        .as_ref()
        .and_then(|id| lib.entries.iter().position(|e| &e.id == id));
    let candidate = compare_pair::candidate(&pool, reference, stored_row);
    let live = app.live_edits();
    let want = [Some(reference), candidate]
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let e = lib.entries.get(row)?;
            // ⚠ The **open** photo renders through its live edits, not the row's: the sidecar is
            // stale exactly while you are editing, and you arrive here straight from Develop.
            let edits = match &live {
                Some((p, c, ed)) if *p == e.id.path && *c == e.id.copy => ed.clone(),
                _ => e.edits.clone(),
            };
            Some((e.id.clone(), edits))
        })
        .collect();
    Some(Plan {
        reference,
        candidate,
        want,
    })
}

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ⚠ Hoisted before any `&mut app.*`: `live_edits` takes `&self`, so it borrows the whole of
    // `app` and cannot follow a mutable borrow of one field. The grid records the same rule.
    let live = app.live_edits();
    // ⚠ Read out here for the same reason — the closure below holds `app.library`.
    let stored = app.compare_candidate.clone();
    let mut out = Outcome::default();

    // ⚠⚠ **The panes are settled BEFORE the panel is drawn**, because building one needs the
    // decode cache and the decode worker while the draw closure is holding `app.library`.
    let plan = plan(app, &stored);
    let display_space = app.settings.effective_display_space(app.os_manages_colour);
    let intent = app.settings.canvas_intent;
    if let Some(p) = &plan {
        app.compare_view
            .sync(&p.want, &mut app.cache, &mut app.decoder, ctx);
    }
    // Two disjoint `&mut` fields, which is legal where a closure capturing `app` twice is E0499 —
    // the shape `masks::BrushTool` already uses.
    let view = &mut app.compare_view;
    let library = &mut app.library;
    let pan_binding = app.settings.pan_drag;
    let auto_advance = app.settings.auto_advance;
    let compare_focus = app.compare_focus;

    egui::CentralPanel::default().show(ui, |ui| {
        let Some(lib) = library else {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(ui, "Open a folder to compare photos.");
            });
            return;
        };
        if lib.visible.is_empty() {
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(ui, "Nothing to compare — every photo is filtered out.");
            });
            return;
        }

        // ⚠ The pair `plan` already resolved, never a second derivation of it — see `Plan`.
        let Some(Plan {
            reference,
            candidate,
            ..
        }) = plan
        else {
            return;
        };
        // `pool` is still needed live, because the arrow keys step through it.
        let selected: Vec<usize> = lib.selection.iter().collect();
        let pool = compare_pair::pool(&lib.visible, &selected);
        // Which pane the border is on, and therefore what the culling keys act on.
        let focus = compare_focus.min(1);

        // ── Keyboard ─────────────────────────────────────────────────────────────────────
        //
        // ⚠⚠ Behind the **same** `memory().focused()` gate the grid's keys use, and that is not
        // optional: the Library's search field is on screen in this view too, and a `3` typed
        // into it must not rate a photograph.
        let typing = ui.ctx().memory(|m| m.focused().is_some());
        ui.input(|i| {
            if typing {
                return;
            }
            // ⚠⚠ **What moves the bordered pane is decided ONCE, below.** The arrows and
            // auto-advance both set this flag rather than each carrying their own copy of the
            // step: two implementations of "step the bordered pane" is exactly how the two would
            // come to disagree about which pane the border means.
            let mut step_forward: Option<bool> = None;
            for (key, forward) in [(egui::Key::ArrowRight, true), (egui::Key::ArrowLeft, false)] {
                if i.key_pressed(key) {
                    step_forward = Some(forward);
                }
            }
            // ⚠⚠ **Enter moves the border, it does not move the photographs** (user's call,
            // 2026-09-02). It used to promote the candidate to reference, which swapped the two
            // panes — and a view whose whole job is holding two frames still while you look
            // between them must not rearrange itself under you. Now it toggles which pane the
            // culling keys act on, which is what the border was already saying.
            if i.key_pressed(egui::Key::Enter) {
                out.focus = Some(1 - focus);
            }
            // ⚠ Escape leaves for the grid, the same exit `G` gives. Without it the one key
            // everybody reaches for to back out of a mode would do nothing here.
            if i.key_pressed(egui::Key::Escape) {
                out.to_grid = true;
            }
            // ⚠ The **focused** pane alone, which is what the border marks — see the module doc.
            let target = if focus == 0 {
                Some(reference)
            } else {
                candidate
            };
            if let (Some(a), Some(t)) = (crate::interaction::culling_key(i), target) {
                if let Some(e) = lib.entries.get(t) {
                    out.mark = Some((e.id.clone(), a));
                }
                // ⚠⚠ **Auto-advance is the arrow's forward step and nothing else** — it goes
                // through the one block below, so the border means the same thing to both keys.
                //
                // ⚠ This is *not* the removal reversed on 2026-09-02: removal takes a frame out
                // of the comparison, advancing steps to the next frame to compare against.
                if auto_advance && a.advances() {
                    step_forward = Some(true);
                }
                // ⚠⚠ **Compare's keys mark and nothing else** (user's call, 2026-09-02). It
                // shipped for one round with `P` also removing the photo from the comparison,
                // which was a misreading: the subtractive act belongs to **Survey**, where the
                // wall you are narrowing is the whole point. Compare holds two frames still and
                // lets you decide between them; taking one away mid-comparison is the opposite
                // of that.
            }
            // ⚠⚠ **The arrows and auto-advance both land here, and this moves the BORDERED
            // pane** — "the border is where the buttons work" has to mean every key or it means
            // none of them. It shipped moving the candidate whichever pane was bordered, which
            // made the border a half-truth.
            //
            // ⚠ Stepping the reference is `step` with the roles swapped: the function's job is
            // "walk the pool, skipping this one photo", and which of the two is being held still
            // is the caller's business. No second traversal.
            //
            // ⚠ Reads the frame's *incoming* `focus`, so an Enter in the same frame moves the
            // border for the next one rather than redirecting this step — which is what the
            // arrows already did.
            if let Some(forward) = step_forward {
                if focus == 0 {
                    if let Some(next) = compare_pair::step(
                        &pool,
                        cand_or(candidate, reference),
                        Some(reference),
                        forward,
                    ) {
                        out.reference = Some(next);
                    }
                } else if let Some(next) = compare_pair::step(&pool, reference, candidate, forward)
                {
                    out.candidate = Some(lib.entries.get(next).map(|e| e.id.clone()));
                }
            }
        });

        // ── Draw ─────────────────────────────────────────────────────────────────────────
        ui.label(egui::RichText::new(INSTRUCTION).small().weak());

        let stage = multi::stage(ui);
        // ⚠ `pair`, never `survey(2, ..)`: Compare is side by side at every window shape,
        // because a left-to-right judgement is the entire point of the view.
        let tiles = tile_layout::pair(stage.width(), stage.height(), 10.0);
        if tiles.len() < 2 {
            return;
        }

        let Some(cand) = candidate else {
            // One photo cannot be compared with anything — say so rather than drawing a pane
            // with nothing in it.
            ui.centered_and_justified(|ui| {
                crate::theme::empty_state(
                    ui,
                    "Only one photo here — select two or more, or clear the filter.",
                );
            });
            return;
        };

        // ── Navigation ───────────────────────────────────────────────────────────────────
        //
        // ⚠ The same inputs the Develop canvas takes, through the same `pan_delta` — scroll or a
        // two-finger swipe pans, pinch or ctrl+scroll zooms, and the configured drag button pans.
        // Duplicating that idiom per view is how the two would drift apart.
        let (scroll, space_down, zoom_factor, pointer) = ui.input(|i| {
            (
                i.smooth_scroll_delta,
                i.key_down(egui::Key::Space),
                i.zoom_delta(),
                i.pointer.hover_pos(),
            )
        });

        let mut window: Vec<ThumbRequest> = Vec::new();
        for (slot, row) in [reference, cand].into_iter().enumerate() {
            let rect = multi::tile_rect(stage, tiles[slot]);
            let req = lib.thumb_request(row, live.as_ref());
            let tex = lib
                .cache
                .get(&req.key())
                // ⚠ This photo's previous render if the exact one is still in flight — never
                // grey while a re-render lands, the rule the grid and the filmstrip follow.
                .or_else(|| lib.cache.get_stale(&lib.entries[row].id));
            // ⚠ The photo's own name, nothing else. "Reference — " and "Candidate — " prefixed
            // every caption with a word that the border already says, and at a glance the label
            // read first and the filename second — which is backwards when the filename is what
            // you are trying to keep track of.
            let caption = lib.entries[row].caption();
            let marks = lib
                .marks
                .get(&lib.entries[row].id)
                .copied()
                .unwrap_or_default();
            // ⚠⚠ **The real render when there is one, the thumbnail only until it lands.** A
            // 512 px thumbnail cannot answer the question this view exists for, so the pane is
            // what makes zooming worth anything — but showing grey while a ~1 s decode runs
            // would make arriving in Compare feel broken.
            let img_area = multi::image_area(rect);
            // ⚠⚠ **Navigation is folded in BEFORE the render is asked for**, so a zoom gesture
            // and the rung it needs land in the same frame. Applied afterwards, every zoom would
            // render one frame stale — visible as the picture sharpening a beat late.
            //
            // ⚠ The pointer gates it: `hovered` is not usable here because the tile's response
            // does not exist yet, and `contains` is the same question asked of the rect that
            // will be drawn. It is what keeps a scroll over one pane out of the other.
            let over = pointer.is_some_and(|p| rect.contains(p));
            let pane_img = view.pane_mut(&lib.entries[row].id).map(|pane| {
                if over {
                    pane.navigate(img_area, zoom_factor, pointer, scroll);
                }
                pane.update(img_area, pane.zoom, display_space, intent, ui.ctx());
                (
                    pane.texture().cloned(),
                    pane.draw_scale(img_area, pane.zoom),
                    pane.pan,
                )
            });
            let image = match &pane_img {
                Some((Some(t), scale, offset)) => multi::TileImage::Pane {
                    tex: t,
                    scale: *scale,
                    offset: *offset,
                },
                // ⚠ `Placeholder`, not `Thumb`: it must occupy exactly the rect the render will,
                // or the photo jumps in size the moment the decode lands.
                _ => match tex {
                    Some(t) => multi::TileImage::Placeholder(t),
                    None => multi::TileImage::Thumb(None),
                },
            };
            let resp = multi::tile(
                ui,
                rect,
                ui.id().with(("compare", slot)),
                image,
                &caption,
                marks,
                // The focused pane carries the ring, because it is what the keys act on.
                slot == focus,
            );
            // ⚠⚠ **One gesture, both panes.** The navigation is read from whichever pane the
            // pointer is over and applied to the shared zoom and pan, so the two always show the
            // same part of the frame — see `CompareView`.
            // ⚠ The drag half of the navigation, which needs the tile's response and so can
            // only be folded in after it. The scroll and zoom halves are handled above, before
            // the render request — see there for why.
            //
            // ⚠ `egui::Vec2::ZERO` for the scroll: it was already applied above, and passing it
            // again would pan at twice the rate.
            let drag = pan_delta(&resp, egui::Vec2::ZERO, space_down, pan_binding, typing);
            if drag != egui::Vec2::ZERO {
                if let Some(pane) = view.pane_mut(&lib.entries[row].id) {
                    pane.navigate(img_area, 1.0, None, drag);
                }
            }
            if resp.double_clicked() {
                out.open = Some(row);
            } else if resp.clicked() {
                // ⚠ Clicking a pane focuses it — the same act as Enter, reached with the mouse,
                // and it now means the same thing on either side. It used to promote, so a click
                // on the left pane had to be a no-op to avoid inventing a second meaning; with
                // Enter reduced to a focus toggle that asymmetry is gone.
                out.focus = Some(slot);
            }
            window.push(req);
        }
        // ⚠ Prefetch the candidates either side, so stepping is instant rather than showing a
        // grey pane for a moment. Four requests in all — nothing against the byte budget.
        for forward in [true, false] {
            if let Some(next) = compare_pair::step(&pool, reference, candidate, forward) {
                window.push(lib.thumb_request(next, live.as_ref()));
            }
        }
        // ⚠⚠ The window is told about **everything drawn**, or `retain_window` would leave these
        // two photos unpinned and evictable while they are the only thing on screen.
        lib.sync_thumb_window(&window);
    });

    if let Some(c) = out.candidate {
        app.compare_candidate = c;
    }
    if let Some((id, action)) = out.mark {
        app.apply_mark_to(&[id], action);
    }
    if let Some(slot) = out.focus {
        app.compare_focus = slot.min(1);
    }
    if let Some(row) = out.reference {
        if let Some(lib) = app.library.as_mut() {
            // ⚠ `focus` when the row is already selected, `click` only as a fallback: `click`
            // replaces the whole selection, which would collapse "compare exactly these" to one
            // photo and silently widen the pool to the entire folder.
            if !lib.selection.focus(row) {
                lib.selection.click(row);
            }
        }
    }
    if out.to_grid {
        app.library_mode = LibraryMode::Grid;
    }
    // ⚠ `open_entry` switches to Develop itself, so this must not do it again — two facts that
    // could disagree, and the rule this file already follows for the candidate.
    if let Some(row) = out.open {
        app.open_entry(ctx, row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppView;

    /// Run one frame of Compare over a library of `n` synthetic photos.
    fn frame(app: &mut RustroomApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(crate::test_ui::screen()),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            show(app, ui, &ctx);
        });
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// ⚠⚠ **The wiring test for the whole feature.** `plan` is what turns the two rows
    /// on screen into two real decodes; get it wrong and Compare silently keeps showing 512 px
    /// thumbnails, which looks exactly like it working until you zoom.
    #[test]
    fn the_two_shown_photos_are_the_two_panes_asked_for() {
        let mut app = app_with(6);
        {
            let lib = app.library.as_mut().unwrap();
            lib.selection.select_indices(&[1, 4]);
            lib.selection.focus(1);
        }
        let want = plan(&app, &None).expect("a plan").want;
        let lib = app.library.as_ref().unwrap();
        let rows: Vec<usize> = want
            .iter()
            .filter_map(|(id, _)| lib.entries.iter().position(|e| e.id == *id))
            .collect();
        assert_eq!(
            rows.len(),
            2,
            "Compare wants exactly two panes, got {rows:?}"
        );
        assert_eq!(rows[0], 1, "the first pane is the reference");
        assert_ne!(rows[1], rows[0], "the candidate is not the reference");
    }

    /// ⚠ The **open** photo renders through its live edits, not the row's. The sidecar is stale
    /// exactly while you are editing, and you arrive in Compare straight from Develop — the rule
    /// edit-accurate thumbnails already follow, and it matters more here because these are the
    /// full-size renders you are judging.
    #[test]
    fn the_open_photos_pane_uses_its_live_edits() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(4);
        crate::test_app::open_test_photo(&mut app, &ctx, 8, 4);
        let open_id = {
            let s = app.session.as_mut().unwrap();
            s.edits.exposure = 1.75;
            rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy)
        };
        {
            let lib = app.library.as_mut().unwrap();
            lib.entries[0].id = open_id.clone();
            lib.entries[0].edits = edit_model::Edits::default();
            lib.selection.select_indices(&[0, 1]);
            lib.selection.focus(0);
        }
        let want = plan(&app, &None).expect("a plan").want;
        let live = want
            .iter()
            .find(|(id, _)| *id == open_id)
            .expect("the open photo is a pane");
        assert_eq!(
            live.1.exposure, 1.75,
            "the pane took the stale sidecar edits rather than the live ones"
        );
    }

    /// ⚠⚠ Leaving Compare must drop the panes: each holds a ~78 MB decode and a render worker,
    /// and nothing else in the app would notice them being kept.
    #[test]
    fn leaving_compare_drops_its_panes() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(4);
        {
            let lib = app.library.as_mut().unwrap();
            lib.selection.select_indices(&[0, 1]);
        }
        let id = app.library.as_ref().unwrap().entries[0].id.clone();
        app.cache
            .insert(id.path.clone(), crate::test_app::fake_decode());
        frame(&mut app, &ctx, Vec::new());
        assert!(
            app.compare_view.pane_mut(&id).is_some(),
            "no pane was built from the cached buffer"
        );
        app.library_mode = LibraryMode::Grid;
        app.release_compare_panes();
        assert!(
            app.compare_view.pane_mut(&id).is_none(),
            "the panes survived leaving Compare"
        );
    }

    fn app_with(n: usize) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.view = AppView::Library;
        app.library_mode = LibraryMode::Compare;
        app.library = Some(crate::test_app::library_of(n));
        app
    }

    #[test]
    fn compare_asks_for_the_two_photos_it_is_showing() {
        // ⚠⚠ The constraint the whole feature hangs on: `retain_window` **pins** what is on
        // screen, so a view that drew from the cache without contributing a want-list would
        // show grey panes *and* let its own photos be evicted. Asserted on the want-list rather
        // than on the drawing, which is the only half this view owns.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(200);
        app.library.as_mut().unwrap().selection.click(0);

        frame(&mut app, &ctx, Vec::new());

        let want = &app.library.as_ref().unwrap().last_want;
        assert!(!want.is_empty(), "Compare asked for nothing at all");
        assert!(
            want.len() <= 4,
            "asked for {} thumbnails — Compare shows two",
            want.len()
        );
        // The reference is row 0, and the candidate is the next one along.
        let paths: Vec<_> = want.iter().map(|r| r.id.path.clone()).collect();
        let entries = &app.library.as_ref().unwrap().entries;
        assert!(
            paths.contains(&entries[0].id.path),
            "the reference is missing"
        );
        assert!(
            paths.contains(&entries[1].id.path),
            "the candidate is missing"
        );
    }

    #[test]
    fn an_arrow_moves_the_candidate_and_leaves_the_reference_alone() {
        // ⚠ Both halves, because moving the *reference* would decode a photo and would make the
        // comparison meaningless — and a test asserting only that the candidate moved cannot
        // see it.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(2);

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.active(), Some(2), "the reference moved");
        assert_eq!(
            app.compare_candidate.as_ref(),
            Some(&lib.entries[4].id),
            "the candidate did not step past the default (row 3)"
        );
    }

    #[test]
    fn enter_moves_the_border_and_leaves_the_photos_where_they_are() {
        // ⚠⚠ **This test used to assert the opposite**, and the rewrite is deliberate: Enter
        // promoted the candidate to reference, which swapped the two panes. It was taken out
        // (user's call, 2026-09-02) because a view whose whole job is holding two frames still
        // while you look between them must not rearrange itself under you.
        //
        // ⚠ Both halves matter. That the border moved is the feature; that the photographs did
        // *not* is the bug that was being fixed, and an assertion on the border alone would pass
        // against the old swapping behaviour too.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(2);
        let before_active = app.library.as_ref().unwrap().selection.active();
        let before_candidate = app.compare_candidate.clone();
        let before_focus = app.compare_focus;

        frame(&mut app, &ctx, vec![key(egui::Key::Enter)]);

        assert_ne!(app.compare_focus, before_focus, "the border did not move");
        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.selection.active(),
            before_active,
            "Enter moved the photographs"
        );
        assert_eq!(
            app.compare_candidate, before_candidate,
            "Enter changed the candidate"
        );
    }

    /// ⚠⚠ **The arrows move the bordered pane.** "The border is where the buttons work" has to
    /// mean every key or it means none of them — it shipped moving the candidate whichever pane
    /// was bordered, which made the border a half-truth.
    #[test]
    fn the_arrows_move_the_bordered_pane() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(8);
        app.library
            .as_mut()
            .unwrap()
            .selection
            .select_indices(&[1, 3, 5]);
        app.library.as_mut().unwrap().selection.focus(1);

        // Border on the candidate: the right pane moves, the reference holds.
        app.compare_focus = 1;
        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(1),
            "the reference moved while the border was on the candidate"
        );
        assert!(
            app.compare_candidate.is_some(),
            "the candidate did not move"
        );

        // Border on the reference: the left pane moves.
        app.compare_focus = 0;
        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);
        assert_ne!(
            app.library.as_ref().unwrap().selection.active(),
            Some(1),
            "the reference did not move while the border was on it"
        );
    }

    /// ⚠ Stepping the reference must not collapse the selection — `click` replaces it, which
    /// would widen the pool to the whole folder with nothing on screen to announce it.
    #[test]
    fn stepping_the_reference_keeps_the_selection() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(8);
        app.library
            .as_mut()
            .unwrap()
            .selection
            .select_indices(&[1, 3, 5]);
        app.compare_focus = 0;
        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.len(),
            3,
            "stepping the reference collapsed the selection"
        );
    }

    /// ⚠ And it is a *toggle*: press it twice and you are back where you started, which is what
    /// makes it safe to try.
    #[test]
    fn enter_toggles_rather_than_stepping_one_way() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(2);
        let start = app.compare_focus;
        frame(&mut app, &ctx, vec![key(egui::Key::Enter)]);
        frame(&mut app, &ctx, vec![key(egui::Key::Enter)]);
        assert_eq!(app.compare_focus, start);
    }

    /// ⚠⚠ The culling keys follow the border, which is the whole reason the border exists. With
    /// it on the reference, a rating must land on the reference — an assertion that only checked
    /// "something was rated" would pass whichever pane it hit.
    #[test]
    fn the_culling_keys_act_on_the_bordered_pane() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(2);
        // ⚠ Without this `set_marks` is a no-op on an uncatalogued photo and the test fails
        // against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);
        app.compare_focus = 0;
        frame(&mut app, &ctx, vec![key(egui::Key::Num3)]);
        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.marks.get(&lib.entries[2].id).map(|m| m.rating),
            Some(3),
            "the bordered pane was not the one rated"
        );
        assert_eq!(
            lib.marks.get(&lib.entries[3].id).map(|m| m.rating),
            None,
            "the unbordered pane was rated too"
        );
    }

    #[test]
    fn moving_the_border_inside_a_two_photo_selection_keeps_both_selected() {
        // ⚠⚠ **This used to test `promote`'s use of `focus` over `click`** — `click` replaces the
        // whole selection, so a swap would collapse "compare exactly these two" to one photo and
        // silently widen the pool to the entire folder. Enter no longer touches the selection at
        // all, so the property is now trivially safe; the test is kept because the property is
        // what matters, not the mechanism that used to threaten it.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library
            .as_mut()
            .unwrap()
            .selection
            .select_indices(&[1, 4]);

        frame(&mut app, &ctx, vec![key(egui::Key::Enter)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(lib.selection.len(), 2, "the selection collapsed");
        assert_eq!(lib.selection.active(), Some(1), "the reference moved");
    }

    #[test]
    fn a_culling_key_rates_the_candidate_and_not_the_reference() {
        // ⚠ The module's central rule, and the half a "did anything get rated?" assertion
        // cannot see: rating the reference would undo a decision already made.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(0);
        // ⚠ Without this `set_marks` is a no-op on an uncatalogued photo and the test fails
        // against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);

        frame(&mut app, &ctx, vec![key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert_eq!(
            lib.marks.get(&lib.entries[1].id).map(|m| m.rating),
            Some(3),
            "the candidate was not rated"
        );
        assert_eq!(
            lib.marks.get(&lib.entries[0].id).map(|m| m.rating),
            None,
            "the reference was rated too"
        );
    }

    #[test]
    fn a_key_typed_into_a_text_field_does_not_rate_anything() {
        // ⚠ The gate the Library's own keys sit behind, re-asserted here because this view has
        // its own `ui.input` block and could have forgotten it. Simulated by giving egui's
        // memory a focused widget before the frame runs.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(0);
        crate::test_app::catalog_library(&mut app);
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("a text field")));

        frame(&mut app, &ctx, vec![key(egui::Key::Num3)]);

        let lib = app.library.as_ref().unwrap();
        assert!(
            lib.marks.is_empty(),
            "a digit typed into a field rated a photo: {:?}",
            lib.marks
        );
    }

    #[test]
    fn a_stale_candidate_falls_back_rather_than_showing_nothing() {
        // ⚠ The self-healing half of `compare_candidate`. A remembered id that no longer names
        // a visible row — filtered away, deleted, or from another folder — must not leave the
        // right-hand pane empty with no way to refill it.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(6);
        app.library.as_mut().unwrap().selection.click(0);
        app.compare_candidate = Some(PhotoId::new(
            std::path::PathBuf::from("/nonexistent/gone.RAF"),
            0,
        ));

        frame(&mut app, &ctx, Vec::new());

        let want = &app.library.as_ref().unwrap().last_want;
        let entries = &app.library.as_ref().unwrap().entries;
        assert!(
            want.iter().any(|r| r.id == entries[1].id),
            "the stale candidate did not fall back to a real one"
        );
    }

    #[test]
    fn a_filtered_library_compares_only_visible_photos() {
        // ⚠⚠ Tested with a *filtered* library rather than a dense one, because `visible` is a
        // sparse index list and this project has made the sparse-versus-dense mistake three
        // times. A pool taken from `entries` rather than `visible` would offer a photo the
        // filter says is not there.
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(20);
        let filter = rustroom_app::library_filter::LibraryFilter {
            text: "0000".into(),
            ..Default::default()
        };
        app.library.as_mut().unwrap().refilter(&filter);
        let visible = app.library.as_ref().unwrap().visible.clone();
        assert!(
            visible.len() > 1 && visible.len() < 20,
            "the fixture must actually filter something: {visible:?}"
        );
        app.library.as_mut().unwrap().selection.click(visible[0]);

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowRight)]);

        let lib = app.library.as_ref().unwrap();
        let shown: Vec<usize> = lib
            .last_want
            .iter()
            .filter_map(|r| lib.entries.iter().position(|e| e.id == r.id))
            .collect();
        for row in shown {
            assert!(
                lib.visible.contains(&row),
                "row {row} is hidden by the filter and Compare showed it anyway"
            );
        }
    }

    /// ⚠⚠ **Probe before choosing, never after.** This project has shipped tofu boxes four times
    /// — the rail's four glyphs, the mark badges, the keyword checkmarks and the perspective
    /// arrows — and this instruction line made it five: `←` and `→` were chosen without checking
    /// and rendered as empty squares. Reported in use.
    #[test]
    fn every_compare_glyph_is_renderable() {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        // ⚠ A frame must run first: a fresh `Context` has not built its atlas, so it answers
        // about egui's defaults rather than the stack production actually uses.
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for c in INSTRUCTION.chars().filter(|c| !c.is_ascii()) {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                "{c:?} (U+{:04X}) is in no bundled font and will draw as a tofu box",
                c as u32
            );
        }
    }

    /// ⚠⚠ **Auto-advance is exactly the arrow-key step on the bordered pane**, and both halves
    /// matter: rating the candidate must move the candidate, and rating the *reference* must move
    /// the reference. "The border is where the buttons work" has to mean every key or it means
    /// none of them.
    ///
    /// ⚠ This is **not** the removal that was reversed on 2026-09-02. Removal takes a frame out
    /// of the comparison; advancing steps to the next frame to compare against.
    #[test]
    fn rating_advances_the_bordered_pane() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(8);
        app.library.as_mut().unwrap().selection.click(2);
        crate::test_app::catalog_library(&mut app);

        // Border on the candidate: the right pane moves, the reference holds.
        app.compare_focus = 1;
        let before = app.compare_candidate.clone();
        frame(&mut app, &ctx, vec![key(egui::Key::Num3)]);
        assert_eq!(
            app.library.as_ref().unwrap().selection.active(),
            Some(2),
            "the reference moved while the border was on the candidate"
        );
        assert_ne!(
            app.compare_candidate, before,
            "the candidate did not advance"
        );

        // Border on the reference: the left pane moves.
        app.compare_focus = 0;
        frame(&mut app, &ctx, vec![key(egui::Key::Num4)]);
        assert_ne!(
            app.library.as_ref().unwrap().selection.active(),
            Some(2),
            "the reference did not advance while the border was on it"
        );
    }

    /// ⚠ The same correction rule the grid follows, re-asserted here because this view has its
    /// own `ui.input` block and could have forgotten it.
    #[test]
    fn a_correction_does_not_advance_in_compare() {
        let ctx = crate::test_ui::ctx();
        let mut app = app_with(8);
        app.library.as_mut().unwrap().selection.click(2);
        crate::test_app::catalog_library(&mut app);
        app.compare_focus = 1;
        let before = app.compare_candidate.clone();

        frame(&mut app, &ctx, vec![key(egui::Key::U)]);

        assert_eq!(
            app.compare_candidate, before,
            "clearing a flag moved on to the next candidate"
        );
    }
}
