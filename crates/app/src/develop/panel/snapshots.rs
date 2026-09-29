//! The Develop snapshot strip: a photo's named saved states, pinned below the adjustments.
//!
//! ⚠⚠ **A deliberate departure from the exclusive-panel rule.** Every other control in this panel
//! belongs to exactly one rail tool; this one is drawn under all of them. A snapshot list is
//! *navigation*, not a tool — the whole point is that the other states of the photo you are
//! working on are always in reach. Behind a fifth rail tool it would be hidden precisely when it
//! is wanted.
//!
//! ⚠ `ui.set_min_height(ui.available_height())` is load-bearing: a resizable egui panel stores its
//! *content's* rendered rect as its size each frame, so content smaller than the panel makes it
//! shrink back every frame. The filmstrip already carries this note.
//!
//! ⚠⚠ **The scroll area here is a SIBLING of the panel's own, not nested inside it.** The strip is
//! allocated before the right panel's `ScrollArea`, so the two occupy disjoint rects and the wheel
//! is never ambiguous. This looks exactly like the nesting the Library panel's three inner scroll
//! areas were removed for on 2026-08-27 — and it is not that. Do not "fix" it.

use crate::develop::panel::section::{header, Header};
use crate::list_row::{self, RowAction, RowSpec};
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::icons;
use rustroom_app::settings::STRIP_HEIGHT_RANGE;

/// What a click in the strip asked for, applied after the panel closes.
///
/// ⚠ Collected rather than acted on immediately: egui borrows `app` through the closure, so a
/// click cannot call a `&mut self` method from inside it. The shape every panel here uses.
enum Action {
    None,
    Take,
    Restore(u32),
    Delete(u32),
    Rename(u32, Option<String>),
    MakeCopy(u32),
}

/// The strip's id, and so — via `.with("__resize")` — its resize handle's.
const PANEL_ID: &str = "snapshot_strip";
/// The gutter the timeline's dots and hairline sit in, left of every row's name.
const GUTTER: f32 = 22.0;
/// A dot's radius.
const DOT: f32 = 4.0;

/// One snapshot as the strip draws it.
struct Row {
    n: u32,
    label: String,
    taken: i64,
    /// The photo's current edits equal this snapshot's — "you are here".
    matches: bool,
}

/// What the Now row says about the edits since the newest snapshot.
///
/// ⚠ Pure, so the sentence is testable without a window: the Now row paints it rather than
/// adding a widget, and a painted string is one the harness cannot read.
fn drift_text(
    latest: Option<(&str, &edit_model::Edits)>,
    current: &edit_model::Edits,
    history: &rustroom_app::history::History<edit_model::Edits>,
) -> String {
    // ⚠ With no snapshot the row still says something useful: how many changes the photo has
    // taken since it was opened (user's call, 2026-09-21, replacing "no snapshots yet").
    let Some((label, edits)) = latest else {
        // ⚠⚠ Measured against the state the photo **opened** in, never `undo_depth()`. That counts
        // *entries*, so resetting an adjustment left "2 changes so far" on a photo where nothing
        // differed — reported in use 2026-09-23.
        //
        // ⚠ And the baseline is the opened edits, not `Edits::default()`: a photo that opened warm
        // from its sidecar has *changed* when it is reset to neutral.
        let opened = history.initial();
        return match current.changed_parameters(opened) {
            0 => "no changes yet".to_string(),
            1 => "1 change so far".to_string(),
            n => format!("{n} changes so far"),
        };
    };
    if edits == current {
        return format!("same as “{label}”");
    }
    // ⚠⚠ **What DIFFERS, never how many history steps happened.** This counted steps back to the
    // snapshot's state until 2026-09-23, so setting a slider, resetting it and setting it again read
    // as **3 changes since “Warm”** while the present differed from the snapshot in one parameter.
    // Reported in use twice: the first fix corrected only the no-snapshot branch above, because the
    // case tested was returning *to* the reference rather than passing through it.
    match current.changed_parameters(edits) {
        // ⚠ Unreachable — `edits == current` is handled above — but "0 changes" would be a poor way
        // to discover that had changed.
        0 => format!("same as “{label}”"),
        1 => format!("1 change since “{label}”"),
        k => format!("{k} changes since “{label}”"),
    }
}

/// Draw the strip. Returns whether the photo's edits changed, which the caller records as one
/// history entry — the contract `panel::show` already has.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    if app.session.is_none() {
        return false;
    }
    let mut action = Action::None;
    // ⚠ Capped at half the *live* panel height rather than only at the stored value, so a height
    // chosen in a large window is clamped rather than obeyed in a small one. egui clamps the
    // persisted size into this range itself — see `Panel::outer_size`.
    let max = (ui.available_height() * 0.5).max(STRIP_HEIGHT_RANGE.0 as f32);
    let stored = app.settings.clamped_strip_height();
    // ⚠ Local wall-clock seconds, `Snapshot::taken`'s own convention, so `when::short` compares
    // like with like.
    let now = chrono::Local::now().naive_local().and_utc().timestamp();

    let panel = egui::Panel::bottom(PANEL_ID)
        .frame(theme::panel_frame(ui.style()))
        .resizable(true)
        .default_size(stored)
        .min_size(STRIP_HEIGHT_RANGE.0 as f32)
        .max_size(max)
        .show(ui, |ui| {
            let (rows, drift) = {
                let s = app.session.as_ref().expect("checked above");
                // Newest first: the one you just took is the one you look for.
                let rows: Vec<Row> = s
                    .snapshots
                    .iter()
                    .rev()
                    .map(|x| Row {
                        n: x.n,
                        label: x.label(),
                        taken: x.taken,
                        matches: x.edits == s.edits,
                    })
                    .collect();
                let latest = s.snapshots.last().map(|x| (x.label(), &x.edits));
                let drift = drift_text(
                    latest.as_ref().map(|(l, e)| (l.as_str(), *e)),
                    &s.edits,
                    &s.history,
                );
                (rows, drift)
            };
            let count = match rows.len() {
                0 => "no snapshots".to_string(),
                1 => "1 snapshot".to_string(),
                n => format!("{n} snapshots"),
            };
            let mut out = header(
                ui,
                ui.make_persistent_id("snapshots"),
                true,
                Header::new("Snapshots").trailing(&count),
            );
            out.show_body(ui, |ui| {
                // Fill the panel's height so egui stores the panel size, not the smaller content
                // size — otherwise a resizable panel shrinks back each frame.
                ui.set_min_height(ui.available_height());
                now_row(ui, &drift, &mut action);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut dots: Vec<egui::Pos2> = Vec::with_capacity(rows.len());
                        for r in &rows {
                            // ⚠ egui's auto-ids shift with what was drawn before them, and this
                            // list changes as snapshots are taken and deleted — the hazard that
                            // broke the rename field under virtualisation.
                            ui.push_id(r.n, |ui| {
                                if let Some(c) = row(app, ui, r, now, &mut action) {
                                    dots.push(c);
                                }
                            });
                        }
                        hairline(ui, &dots);
                    });
            });
        });

    // ⚠ Persisted on the **release** frame only, never per frame: a resize runs at 60 Hz — the
    // `apply_view_demosaic` rule. egui deliberately recomputes the panel size on `drag_stopped`
    // so the released size is the one stored, and `InnerResponse::response.rect` is the panel's
    // *outer* rect — the same quantity `default_size` takes, so feeding it back cannot drift.
    let resize_id = egui::Id::new(PANEL_ID).with("__resize");
    if ui
        .read_response(resize_id)
        .is_some_and(|r| r.drag_stopped())
    {
        let h = panel.response.rect.height().round() as u32;
        if h != app.settings.snapshot_strip_height {
            app.settings.snapshot_strip_height = h;
            app.save_settings();
        }
    }

    apply(app, ctx, action)
}

/// The Now row: a dashed dot, "Now", the drift since the newest snapshot, and the Save button.
///
/// ⚠ Not a snapshot and not a `list_row`: it cannot be restored, copied or deleted, and a row
/// that looked exactly like the others while doing none of what they do would be the worst
/// kind of affordance. It is a painted line with one real button.
fn now_row(ui: &mut egui::Ui, drift: &str, action: &mut Action) {
    let height = list_row::ROW_HEIGHT;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let v = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    let centre = egui::pos2(rect.left() + GUTTER / 2.0, rect.center().y);
    paint_dot(&painter, centre, Dot::Now, &v);
    // The hairline's first segment: from the Now dot down to the row's bottom, where the
    // scroll area's own segment picks it up.
    painter.line_segment(
        [
            egui::pos2(centre.x, centre.y + DOT + 2.0),
            egui::pos2(centre.x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
    );
    let font = egui::TextStyle::Body.resolve(ui.style());
    let name = painter.text(
        egui::pos2(rect.left() + GUTTER + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Now",
        font,
        v.weak_text_color(),
    );
    // The button first, at the right edge, so the drift text knows how much room it has.
    let button = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.add(egui::Button::new("+ Save").min_size(egui::vec2(0.0, 22.0)))
                    .on_hover_text("Save the current settings as a snapshot")
            },
        )
        .inner;
    // ⚠ The accessible name is the menu's wording, so the harness and a screen reader find
    // one "Take snapshot" whichever way it is reached.
    button
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Take snapshot"));
    if button.clicked() {
        *action = Action::Take;
    }
    let room = (button.rect.left() - 8.0 - (name.right() + 8.0)).max(0.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        drift.to_string(),
        egui::FontId::proportional(crate::theme::tokens::CAPTION),
        v.weak_text_color(),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(name.right() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        v.weak_text_color(),
    );
}

/// A timeline dot's state.
enum Dot {
    /// The present: a dashed ring.
    Now,
    /// The snapshot the photo is at: filled with the accent.
    Here,
    /// Any other snapshot: a hollow ring.
    Plain,
}

fn paint_dot(painter: &egui::Painter, centre: egui::Pos2, dot: Dot, v: &egui::Visuals) {
    match dot {
        Dot::Here => {
            painter.circle_filled(centre, DOT, v.selection.bg_fill);
        }
        Dot::Plain => {
            painter.circle_stroke(centre, DOT, egui::Stroke::new(1.2, v.weak_text_color()));
        }
        Dot::Now => {
            // Eight short arcs: a dashed ring, since egui strokes circles whole.
            let stroke = egui::Stroke::new(1.2, v.weak_text_color());
            for k in 0..8 {
                let a0 = k as f32 * std::f32::consts::TAU / 8.0;
                let a1 = a0 + std::f32::consts::TAU / 16.0;
                let pts: Vec<egui::Pos2> = (0..=3)
                    .map(|i| {
                        let a = a0 + (a1 - a0) * i as f32 / 3.0;
                        centre + egui::vec2(a.cos(), a.sin()) * DOT
                    })
                    .collect();
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
    }
}

/// The hairline joining the dots, from the top of the list down to the last dot.
fn hairline(ui: &egui::Ui, dots: &[egui::Pos2]) {
    let Some(last) = dots.last() else {
        return;
    };
    let top = ui.min_rect().top();
    let colour = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let p = ui.painter();
    // Between the dots, leaving each one a little air.
    let mut from = egui::pos2(last.x, top);
    for d in dots {
        p.line_segment(
            [from, egui::pos2(d.x, d.y - DOT - 2.0)],
            egui::Stroke::new(1.0, colour),
        );
        from = egui::pos2(d.x, d.y + DOT + 2.0);
    }
}

/// One snapshot's row. Returns its dot's centre, once painted.
fn row(
    app: &mut RustroomApp,
    ui: &mut egui::Ui,
    r: &Row,
    now: i64,
    action: &mut Action,
) -> Option<egui::Pos2> {
    // While this row owns the draft, the row is replaced by an editable field.
    if app.snapshot_rename.as_ref().is_some_and(|(m, _)| *m == r.n) {
        let mut text = app
            .snapshot_rename
            .as_ref()
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY));
        if let Some((_, t)) = app.snapshot_rename.as_mut() {
            *t = text.clone();
        }
        // ⚠⚠ **`name_edit::commit_gesture`, not `theme::committed` — and that distinction is a
        // bug this shipped with.** A singleline `TextEdit` calls `surrender_focus` *inside*
        // `ui.add` when Enter is pressed, so the autofocus guard immediately takes the focus back
        // and `lost_focus()` — which reads live memory — reports `false`. The commit never fired,
        // for Enter *or* for a click elsewhere. Reported in use, reproduced headlessly, and now
        // guarded by two tests. The shared helper is the same one the filename field uses, so the
        // two cannot drift.
        if crate::name_edit::commit_gesture(ui, &resp, true) {
            let typed = text.trim().to_string();
            // ⚠ Typing the default label back in stores `None`, so the snapshot keeps following
            // its number — the rule an unnamed virtual copy already follows. Freezing the derived
            // string would strand it on today's number.
            *action = Action::Rename(
                r.n,
                (!typed.is_empty() && typed != format!("Snapshot {}", r.n)).then_some(typed),
            );
        }
        return None;
    }

    let time = rustroom_app::when::short(r.taken, now);
    let out = list_row::row(
        ui,
        RowSpec {
            name: &r.label,
            count: None,
            meta: &time,
            selected: r.matches,
            indent: GUTTER,
            hover: "",
            actions: &[
                RowAction {
                    glyph: '×',
                    verb: "Delete",
                    hover: "Delete this snapshot",
                    enabled: true,
                },
                RowAction {
                    glyph: icons::COPY,
                    verb: "Make a virtual copy from",
                    hover: "Make a virtual copy with these settings",
                    enabled: true,
                },
            ],
        },
    );
    let centre = egui::pos2(out.rect.left() + GUTTER / 2.0, out.rect.center().y);
    if ui.is_rect_visible(out.rect) {
        let v = ui.visuals().clone();
        paint_dot(
            &ui.painter_at(out.rect),
            centre,
            if r.matches { Dot::Here } else { Dot::Plain },
            &v,
        );
    }
    match out.action {
        Some(0) => *action = Action::Delete(r.n),
        Some(_) => *action = Action::MakeCopy(r.n),
        None if out.double_clicked => app.snapshot_rename = Some((r.n, r.label.clone())),
        None if out.clicked => *action = Action::Restore(r.n),
        None => {}
    }
    out.response.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            app.snapshot_rename = Some((r.n, r.label.clone()));
            ui.close();
        }
        if ui.button("Make virtual copy").clicked() {
            *action = Action::MakeCopy(r.n);
            ui.close();
        }
        if ui.button("Delete").clicked() {
            *action = Action::Delete(r.n);
            ui.close();
        }
    });
    Some(centre)
}

/// Apply what the frame asked for. Returns whether the photo's edits changed.
fn apply(app: &mut RustroomApp, ctx: &egui::Context, action: Action) -> bool {
    match action {
        Action::None => false,
        Action::Take => {
            app.take_snapshot();
            false
        }
        Action::Restore(n) => app.restore_snapshot(n),
        Action::Delete(n) => {
            app.delete_snapshot(n);
            false
        }
        Action::Rename(n, name) => {
            app.rename_snapshot(n, name);
            app.snapshot_rename = None;
            false
        }
        Action::MakeCopy(n) => {
            app.copy_from_snapshot(ctx, n);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{test_app, test_ui};
    use rustroom_app::tool::Tool;

    /// ⚠⚠ **The requirement the strip exists to meet, and the only test that can see it.**
    /// "Always active" is otherwise a visual claim: a strip drawn under `Tool::Edit` alone looks
    /// perfectly correct in any screenshot of the Edit panel.
    #[test]
    fn the_strip_is_present_under_every_tool() {
        for tool in Tool::ALL {
            let ctx = test_ui::ctx();
            let mut app = test_app::for_test();
            test_app::open_test_photo(&mut app, &ctx, 8, 4);
            app.tool = tool;

            let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                crate::develop::panel::show(&mut app, ui);
            });
            assert!(
                labels.iter().any(|(l, _)| l == "Take snapshot"),
                "the Take-snapshot button must be reachable under {tool:?}, got {:?}",
                labels.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }

    /// The strip draws nothing when no photo is open — the panel says "No image open." instead.
    #[test]
    fn the_strip_draws_nothing_without_a_session() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        assert!(!labels.iter().any(|(l, _)| l == "Take snapshot"));
    }

    /// A row is labelled by `Snapshot::label`, so a named one shows its name rather than its
    /// number — the one rule every consumer of a snapshot's name goes through.
    #[test]
    fn a_named_snapshot_shows_its_name_in_the_strip() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        app.session
            .as_mut()
            .unwrap()
            .snapshots
            .push(rustroom_app::sidecar::Snapshot {
                n: 1,
                name: Some("Before the crop".to_string()),
                taken: 0,
                edits: Default::default(),
            });

        let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        assert!(
            labels.iter().any(|(l, _)| l == "Before the crop"),
            "got {:?}",
            labels.iter().map(|(l, _)| l).collect::<Vec<_>>()
        );
    }

    /// ⚠ The height is app-wide, not per photo — it lives in `Settings`, not on `Session`. Stated
    /// as a property so a future move onto `Session` breaks a test rather than a habit.
    #[test]
    fn the_strip_height_is_app_wide_not_per_photo() {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.snapshot_strip_height = 200;
        test_app::open_test_photo(&mut app, &ctx, 8, 4);
        test_app::open_test_photo(&mut app, &ctx, 16, 8);
        assert_eq!(
            app.settings.snapshot_strip_height, 200,
            "opening another photo must not reset the strip"
        );
    }

    /// Run one frame of the real panel with `events`.
    fn frame(ctx: &egui::Context, app: &mut crate::RustroomApp, events: Vec<egui::Event>) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                events,
                ..Default::default()
            },
            |ui| {
                crate::develop::panel::show(app, ui);
            },
        );
    }

    /// An app with one snapshot, in the sidecar *and* in the session cache — `set_snapshot_name`
    /// writes through the sidecar, so a cache-only fixture would prove nothing.
    fn app_with_a_snapshot(dir: &test_app::TestDir, ctx: &egui::Context) -> crate::RustroomApp {
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"x").unwrap();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, ctx, 8, 4);
        app.session.as_mut().unwrap().path = photo.clone();
        app.take_snapshot();
        app
    }

    /// ⚠⚠ **Reported from use: "clicking enter or somewhere else does not accept the rename".**
    #[test]
    fn pressing_enter_commits_a_snapshot_rename() {
        let dir = test_app::TestDir::new("renameenter");
        let ctx = test_ui::ctx();
        let mut app = app_with_a_snapshot(&dir, &ctx);
        app.snapshot_rename = Some((1, "Warm".to_string()));

        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );

        assert_eq!(
            app.session.as_ref().unwrap().snapshots[0].name.as_deref(),
            Some("Warm"),
            "Enter must commit the typed name"
        );
        assert!(app.snapshot_rename.is_none(), "and close the field");
    }

    /// ⚠⚠ **The other half of the same report, and a deliberate reversal of the spec.** The design
    /// said clicking away would *abandon* a half-typed name. In use the expectation was the
    /// opposite — and the filename field, which people already use, commits on click-away. One
    /// idiom beats two, so the strip follows it.
    #[test]
    fn clicking_elsewhere_commits_a_snapshot_rename() {
        let dir = test_app::TestDir::new("renameclick");
        let ctx = test_ui::ctx();
        let mut app = app_with_a_snapshot(&dir, &ctx);
        app.snapshot_rename = Some((1, "Warm".to_string()));

        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());
        // A press far from the field: the strip surrenders focus, so `lost_focus` fires next
        // frame — which is the one commit path.
        let away = egui::pos2(10.0, 10.0);
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(away),
                egui::Event::PointerButton {
                    pos: away,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(&ctx, &mut app, Vec::new());

        assert_eq!(
            app.session.as_ref().unwrap().snapshots[0].name.as_deref(),
            Some("Warm"),
            "a click elsewhere must accept the typed name, not discard it"
        );
        assert!(app.snapshot_rename.is_none(), "and close the field");
    }

    /// ⚠ The rename must reach **disk**, not only the session cache — a name that vanished on the
    /// next folder open would look identical in every assertion above.
    #[test]
    fn a_committed_rename_reaches_the_sidecar() {
        let dir = test_app::TestDir::new("renamedisk");
        let ctx = test_ui::ctx();
        let mut app = app_with_a_snapshot(&dir, &ctx);
        app.snapshot_rename = Some((1, "Warm".to_string()));
        frame(&ctx, &mut app, Vec::new());
        frame(&ctx, &mut app, Vec::new());
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );

        let photo = app.session.as_ref().unwrap().path.clone();
        assert_eq!(
            rustroom_app::sidecar::load_all(&photo).snapshots_of(0)[0]
                .name
                .as_deref(),
            Some("Warm")
        );
    }

    /// The Now row's sentence, without a window.
    #[test]
    fn the_now_row_says_how_far_the_edits_have_drifted() {
        use edit_model::Edits;
        let base = Edits::default();
        let warm = Edits {
            exposure: 0.5,
            ..Default::default()
        };
        let mut h = rustroom_app::history::History::new(base.clone());
        assert_eq!(super::drift_text(None, &base, &h), "no changes yet");
        assert_eq!(
            super::drift_text(Some(("Warm", &base)), &base, &h),
            "same as “Warm”"
        );
        h.push(warm.clone());
        let more = Edits {
            contrast: 10.0,
            ..warm.clone()
        };
        h.push(more.clone());
        assert_eq!(super::drift_text(None, &more, &h), "2 changes so far");
        assert_eq!(
            super::drift_text(Some(("Warm", &warm)), &more, &h),
            "1 change since “Warm”"
        );
        assert_eq!(
            super::drift_text(Some(("Warm", &base)), &more, &h),
            "2 changes since “Warm”"
        );
        // ⚠ A snapshot state the history never held is no longer a special case: the count comes
        // from what differs, so it answers with a number. `elsewhere` differs from `more` in both
        // exposure and contrast.
        let elsewhere = Edits {
            exposure: -1.0,
            ..Default::default()
        };
        assert_eq!(
            super::drift_text(Some(("Warm", &elsewhere)), &more, &h),
            "2 changes since “Warm”"
        );
    }

    /// ⚠⚠ **Reported in use, 2026-09-23.** Reset an adjustment and the Now row still said changes had
    /// been made: the no-snapshot branch asked `undo_depth()`, which counts *entries*, so a value set
    /// and then set back read as two changes with nothing different. The count has to be measured
    /// against the state the photo opened in, not against the length of the undo stack.
    #[test]
    fn resetting_back_to_the_opened_state_reports_no_changes() {
        use edit_model::Edits;
        let base = Edits::default();
        let warm = Edits {
            exposure: 0.5,
            ..Default::default()
        };
        let mut h = rustroom_app::history::History::new(base.clone());
        h.push(warm.clone());
        h.push(base.clone()); // reset
        assert_eq!(h.undo_depth(), 2, "the entries are still there");
        assert_eq!(
            super::drift_text(None, &base, &h),
            "no changes yet",
            "but nothing differs from how the photo opened"
        );
    }

    /// ⚠⚠ **Reported in use 2026-09-23, AFTER a first fix that corrected only the no-snapshot
    /// branch.** With a snapshot taken at exposure 0.5: set exposure, reset it, set it again, and the
    /// row said **"3 changes since “Warm”"** — three history steps back to the snapshot's state —
    /// while the present differed from that snapshot in exactly one parameter.
    ///
    /// ⚠ The first fix missed this because the case tested was returning *to* the reference state,
    /// which step-counting handles correctly. Passing *through* it and ending elsewhere is what
    /// inflates, and that is the shape a "reset" actually has.
    #[test]
    fn passing_through_the_snapshot_state_does_not_inflate_the_count() {
        use edit_model::Edits;
        let e = |v: f32| Edits {
            exposure: v,
            ..Default::default()
        };
        let neutral = Edits::default();
        let snap = e(0.5);

        let mut h = rustroom_app::history::History::new(neutral.clone());
        h.push(snap.clone()); // the state the snapshot holds
        h.push(e(1.0)); // set exposure
        h.push(neutral.clone()); // reset it
        h.push(e(0.8)); // set it again
        assert_eq!(h.undo_depth(), 4, "four entries were pushed");

        assert_eq!(
            super::drift_text(Some(("Warm", &snap)), &e(0.8), &h),
            "1 change since “Warm”",
            "one parameter differs from the snapshot, whatever route got here"
        );
    }

    /// ⚠ And the same route back to the snapshot's own state reads as no drift at all.
    #[test]
    fn returning_to_the_snapshot_state_reads_as_the_same() {
        use edit_model::Edits;
        let snap = Edits {
            exposure: 0.5,
            ..Default::default()
        };
        let mut h = rustroom_app::history::History::new(Edits::default());
        h.push(snap.clone());
        h.push(Edits::default());
        h.push(snap.clone());
        assert_eq!(
            super::drift_text(Some(("Warm", &snap)), &snap, &h),
            "same as “Warm”"
        );
    }

    /// ⚠ A photo that opened **with** edits from its sidecar: the baseline is those edits, not
    /// `Edits::default()`. Resetting to neutral is then a real change and must still be counted.
    #[test]
    fn a_photo_opened_with_edits_measures_against_those_edits() {
        use edit_model::Edits;
        let opened = Edits {
            exposure: 0.5,
            ..Default::default()
        };
        let neutral = Edits::default();
        let mut h = rustroom_app::history::History::new(opened.clone());
        h.push(neutral.clone());
        assert_eq!(
            super::drift_text(None, &neutral, &h),
            "1 change so far",
            "resetting a photo that opened warm IS a change"
        );
        h.push(opened.clone());
        assert_eq!(
            super::drift_text(None, &opened, &h),
            "no changes yet",
            "and going back to how it opened is not"
        );
    }

    /// ⚠ Counting from the **most recent** visit to the opened state, which is the convention the
    /// snapshot branch already uses for "n changes since “Warm”". Set, reset, set again is one change
    /// away from the original, not three.
    #[test]
    fn the_count_runs_from_the_last_time_the_edits_were_back_at_the_start() {
        use edit_model::Edits;
        let base = Edits::default();
        let a = Edits {
            exposure: 0.5,
            ..Default::default()
        };
        let b = Edits {
            contrast: 10.0,
            ..Default::default()
        };
        let mut h = rustroom_app::history::History::new(base.clone());
        h.push(a);
        h.push(base.clone());
        h.push(b.clone());
        assert_eq!(h.undo_depth(), 3);
        assert_eq!(super::drift_text(None, &b, &h), "1 change so far");
    }

    /// The row the photo is at reports selected; after an edit it no longer does; and both
    /// cells are named for the harness and a screen reader.
    #[test]
    fn the_matching_row_is_lit_and_the_cells_are_named() {
        let dir = test_app::TestDir::new("snaplit");
        let ctx = test_ui::ctx();
        let mut app = app_with_a_snapshot(&dir, &ctx);
        let selected = |app: &mut crate::RustroomApp| -> Option<bool> {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    crate::develop::panel::show(app, ui);
                },
            );
            let update = out.platform_output.accesskit_update?;
            update.nodes.iter().find_map(|(_, n)| {
                (n.label() == Some("Snapshot 1"))
                    .then(|| n.toggled() == Some(egui::accesskit::Toggled::True))
            })
        };
        assert_eq!(
            selected(&mut app),
            Some(true),
            "just taken: the photo is at it"
        );
        app.session.as_mut().unwrap().edits.exposure = 0.7;
        assert_eq!(selected(&mut app), Some(false), "edited: it is not");
        let labels = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        for want in ["Delete Snapshot 1", "Make a virtual copy from Snapshot 1"] {
            assert!(
                labels.iter().any(|(l, _)| l == want),
                "{want:?} missing in {:?}",
                labels.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }
}
