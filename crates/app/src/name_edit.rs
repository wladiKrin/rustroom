//! The one editable name a photo has, and what committing it means.
//!
//! ⚠⚠ **Which name it edits depends on the row, and that asymmetry is the feature.** On the
//! original it is the filename on disk; on a virtual copy it is that copy's own name. A copy has
//! no file of its own, so editing the filename from one renamed the shared file and dragged every
//! sibling copy along with it — reported as surprising, and the reason copy names exist.
//!
//! ⚠ This module exists so the Develop panel and the Library grid cannot drift apart. They had
//! one editable field between them and it lived in the panel; growing a second copy in the grid
//! is how "the same logic" becomes two subtly different logics.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::entries;
use rustroom_app::file_rename;
use rustroom_app::photo_id::PhotoId;
use std::path::PathBuf;

/// What the name field committed: a file on disk, or one copy's label.
#[derive(Debug, PartialEq, Eq)]
pub enum Rename {
    /// Rename the file on disk. Every copy of it follows.
    File(PathBuf),
    /// Set this copy's label. `None` clears it, so the badge falls back to the copy number.
    Label(Option<String>),
}

/// A name shown as a plain caption until it is clicked, then edited in place.
///
/// ⚠⚠ **A caption, not a field, until you ask for one.** Drawing the field for whichever row was
/// selected meant merely clicking a photo put you in a text box — and with the box focused, arrow
/// keys and Enter belong to it, so browsing the grid stopped working. Selecting a photo and
/// renaming it are two different intentions and now need two different clicks.
///
/// ⚠ The field is drawn whenever the draft belongs to this row, **regardless of selection**. If
/// it were tied to the selection instead, clicking another photo would remove the field before it
/// could lose focus, and the typed name would vanish without ever being committed.
pub fn caption_or_field(
    ui: &mut egui::Ui,
    id: &PhotoId,
    file_name: &str,
    name: Option<&str>,
    draft: &mut Option<(PhotoId, String)>,
    size: f32,
) -> Option<Rename> {
    // ⚠ An explicit size, not a `TextStyle`: the Library's cells are resizable and the name has
    // to grow with the thumbnail. A fixed caption beside a scaling thumbnail is what makes a size
    // control look broken at the extremes.
    let font = egui::FontId::proportional(size);
    let editing = draft.as_ref().is_some_and(|(d, _)| d == id);
    if !editing {
        let shown = entries::caption(file_name, id.copy);
        let r = ui.add(
            egui::Label::new(egui::RichText::new(shown.clone()).font(font))
                .sense(egui::Sense::click()),
        );
        if r.clicked() {
            *draft = Some((id.clone(), shown));
        }
        return None;
    }
    field(
        ui,
        NameField {
            id,
            file_name,
            label: name,
            mode: Editing::FileName,
        },
        draft,
        font,
        true,
    )
}

/// An editable name, drawn as a text field.
///
/// ⚠ Edits a **scratch** draft, not the stored value. Writing through would rename the file on
/// every keystroke — creating a file per character — and would commit a half-typed name the moment
/// focus moved. Same shape the export dialog's `export_draft` uses.
/// The gesture half of an in-place name field: take focus when it appears, end the edit on Enter
/// **or** on a press anywhere outside, and report when it ended.
///
/// ⚠⚠ **The commit condition is `lost_focus() || (has_focus() && entered)`, never
/// `lost_focus() && key_pressed(Enter)`** — and that difference is a bug this project has now
/// shipped once. A singleline `TextEdit` calls `surrender_focus` *within* `ui.add` when Enter is
/// pressed, so `focused()` is `None` immediately afterwards; the autofocus guard below then takes
/// the focus straight back, and because `Response::lost_focus()` reads **live** memory it reports
/// `false`. The `has_focus() && entered` half survives that. `theme::committed` is safe only for
/// a field that never re-requests focus.
///
/// ⚠⚠ **A click anywhere else must end the edit, and `lost_focus` alone does not deliver it.**
/// Clicking empty space moves focus *nowhere* — there is nothing focusable there — so the field
/// keeps focus and only Enter can close it. Surrendering here makes `lost_focus()` fire on the
/// next frame, which is the one commit path.
///
/// ⚠ Both `ui.input(..)` reads happen **before** `r` is touched: `Response` methods take the
/// context lock internally, and holding one across `ui.input(..)` panics inside epaint's mutex —
/// the re-entrant lock this file already paid for once.
///
/// ⚠ `autofocus` only where the field appears **on demand**. A field drawn every frame would grab
/// the keyboard the moment a photo opened, and every single-key shortcut would type into it.
pub fn commit_gesture(ui: &mut egui::Ui, r: &egui::Response, autofocus: bool) -> bool {
    if autofocus && ui.memory(|m| m.focused()).is_none() {
        r.request_focus();
    }
    let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let pressed_at = ui.input(|i| {
        i.pointer
            .any_pressed()
            .then(|| i.pointer.interact_pos())
            .flatten()
    });
    if let Some(pos) = pressed_at {
        if !r.rect.contains(pos) && r.has_focus() {
            r.surrender_focus();
        }
    }
    r.lost_focus() || (r.has_focus() && entered)
}

/// Which string a name field is editing.
///
/// ⚠⚠ The field used to infer this from the copy number — filename on the original, name on a
/// copy. That inference is what made one control mean two things, and M6.0 removed its last
/// basis: copy 0 now carries **both** a filename and a label, so nothing distinguishes them but
/// an explicit choice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Editing {
    /// The file on disk. Every copy follows, since they share it.
    FileName,
    /// This copy's label. Never a path, never sanitized here.
    Label,
}

/// Which photo a name field is for, and which of its two strings it edits.
///
/// ⚠ A struct rather than four more parameters, and clippy is right to ask: `field` reached eight.
/// The same answer `FrameSpec` gave when `render_cached` reached the same limit — the body
/// destructures it on the first line, so it reads exactly as before.
pub struct NameField<'a> {
    pub id: &'a PhotoId,
    pub file_name: &'a str,
    pub label: Option<&'a str>,
    pub mode: Editing,
}

pub fn field(
    ui: &mut egui::Ui,
    spec: NameField<'_>,
    draft: &mut Option<(PhotoId, String)>,
    font: egui::FontId,
    autofocus: bool,
) -> Option<Rename> {
    let NameField {
        id,
        file_name,
        label,
        mode,
    } = spec;
    // ⚠ What the field shows is what the user is editing — the caption beside it, or the badge
    // above it. Showing one and committing the other is the bug M6.0 exists to remove.
    let current = match mode {
        Editing::FileName => entries::caption(file_name, id.copy),
        Editing::Label => label.unwrap_or_default().to_string(),
    };

    // ⚠ A draft belonging to a *different* photo is discarded, not shown. Without this the last
    // name typed followed you to every photo you opened afterwards.
    if draft.as_ref().is_some_and(|(d, _)| d != id) {
        *draft = None;
    }
    let (_, text) = draft.get_or_insert_with(|| (id.clone(), current.clone()));

    let r = ui.add(
        egui::TextEdit::singleline(text)
            .font(font)
            .desired_width(f32::INFINITY),
    );
    // ⚠ Take focus the frame the field appears, so a click on the caption lands you in the box
    // ready to type. Guarded on *nothing* being focused rather than on a "just started" flag:
    // once this field has focus, `focused()` is `Some(self)`, so it cannot steal focus later.
    //
    // ⚠⚠ **Only where the field appears on demand.** The Develop panel draws its field on every
    // frame, so autofocusing there would grab the keyboard the moment a photo opened — arrow keys
    // and every single-key shortcut would type into the filename instead.

    if r.gained_focus() {
        // Clicking in selects the whole name, so typing replaces it.
        if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id) {
            let end = text.chars().count();
            st.cursor
                .set_char_range(Some(egui::text_selection::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(end),
                )));
            st.store(ui.ctx(), r.id);
        }
    }

    if commit_gesture(ui, &r, autofocus) {
        let typed = text.clone();
        *draft = None;
        if typed == current {
            // ⚠ Nothing typed, nothing recorded — or Ctrl+Z would appear to do nothing.
            return None;
        }
        if mode == Editing::Label {
            let trimmed = typed.trim();
            // ⚠ Clearing the field, or typing back the copy number the badge already shows,
            // stores `None` rather than that literal. `None` is what keeps an unlabelled copy
            // following its number; freezing "2" as a label would look identical and then not
            // move if the copy were ever renumbered by `Reindex`.
            let derived = id.copy.to_string();
            let to = (!trimmed.is_empty() && trimmed != derived).then(|| trimmed.to_string());
            return (to.as_deref() != label).then_some(Rename::Label(to));
        }
        // ⚠ A refusal leaves the file alone and the field resets to the real name, so an empty
        // or unusable entry can never look like it worked.
        if let Ok(to) = file_rename::target(&id.path, &typed) {
            if to != id.path {
                return Some(Rename::File(to));
            }
        }
    }
    None
}

/// Apply what [`field`] committed, and record it so Ctrl+Z can take it back.
///
/// ⚠ Must be called **outside** whatever borrow drew the field: it writes the sidecar and rebuilds
/// the Library, both of which need `&mut RustroomApp`.
pub fn commit(app: &mut RustroomApp, ctx: &egui::Context, id: &PhotoId, rename: Rename) {
    match rename {
        Rename::File(to) => {
            let from = id.path.clone();
            if app.rename_file(ctx, &from, &to) {
                // ⚠ Pushed only when the rename actually happened — a refused one (name taken,
                // empty) must not leave an undo step that would move a file nobody moved.
                //
                // ⚠ Global rather than photo-scoped: after the rename the photo is at a
                // *different* path, so scoping it to the old one would make it unreachable the
                // moment it took effect.
                app.actions.push(ActionKind::Rename { from, to }, None);
            }
        }
        Rename::Label(to) => {
            let from = app.copy_name_of(id);
            if app.set_label(id, to.clone()) {
                // ⚠ Scoped to its copy, unlike a file rename: the photo still exists at the same
                // path, so `reachable` can compare against it.
                app.actions.push(
                    ActionKind::CopyRename {
                        id: id.clone(),
                        from,
                        to,
                    },
                    Some(id),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo() -> PhotoId {
        PhotoId::new(PathBuf::from("/x/a.RAF"), 1)
    }

    /// Lay the row out, click, release, then draw again — **four** passes. `Response` state is
    /// per-frame, so a single-pass harness reads zeros (the mistake that made
    /// `a_left_drag_does_not_pan` pass vacuously); and the click is only known on the *release*
    /// pass, so the field it opens cannot exist until the frame after that.
    ///
    /// `click` selects whether a click is delivered at all, so the same harness can show that
    /// *not* clicking leaves the row alone.
    fn run(click: bool, draft: &mut Option<(PhotoId, String)>) -> bool {
        let ctx = egui::Context::default();
        let mut target = egui::Pos2::ZERO;
        let mut was_field = false;
        for pass in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                events: {
                    let mut e = vec![egui::Event::PointerMoved(target)];
                    if click && (pass == 1 || pass == 2) {
                        e.push(egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed: pass == 1,
                            modifiers: Default::default(),
                        });
                    }
                    e
                },
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                let before = ui.next_widget_position();
                caption_or_field(ui, &photo(), "a.RAF", None, draft, 11.0);
                if pass == 0 {
                    target = before + egui::vec2(20.0, 6.0);
                }
                // On the last pass, report whether a text field is what got drawn.
                // ⚠ Focus, not `draft.is_some()`, is the honest signal: a draft belonging to a
                // *different* photo is `Some` while this row still draws a plain caption. It is
                // also what the user experiences — a focused box owns the arrow keys.
                if pass == 3 {
                    was_field = ui.ctx().memory(|m| m.focused().is_some());
                }
            });
        }
        was_field
    }

    #[test]
    fn a_row_shows_a_caption_until_its_name_is_clicked() {
        // ⚠⚠ The reported behaviour: merely selecting a photo put you inside a text box, which
        // then owned the arrow keys and Enter, so browsing the grid stopped working. Selecting
        // and renaming are two intentions and take two clicks.
        let mut draft = None;
        assert!(
            !run(false, &mut draft),
            "no click, so no field and nothing being edited"
        );
        assert_eq!(draft, None);
    }

    #[test]
    fn clicking_the_name_starts_editing_it() {
        let mut draft = None;
        assert!(
            run(true, &mut draft),
            "clicking the caption opens the field"
        );
        let (id, text) = draft.expect("a draft was started");
        assert_eq!(id, photo());
        assert_eq!(
            text, "a.RAF",
            "pre-filled with the file name it is captioned with, not blank"
        );
    }

    #[test]
    fn a_draft_belonging_to_another_photo_is_not_shown_here() {
        // ⚠ Without this the last name typed followed you to every photo opened afterwards.
        let other = PhotoId::new(PathBuf::from("/x/b.RAF"), 1);
        let mut draft = Some((other, "typed elsewhere".to_string()));
        assert!(
            !run(false, &mut draft),
            "another photo's draft must not put this row into edit mode"
        );
    }

    /// ⚠⚠ The label field's own commit rule, which differs from the filename field's on purpose.
    /// Typing back the copy number the badge already shows stores `None`, so the copy keeps
    /// *following* its number rather than freezing "2" as a literal label — which would look
    /// identical and then not move if `Reindex` ever renumbered it.
    #[test]
    fn typing_the_copy_number_back_clears_the_label() {
        let id = PhotoId::new(PathBuf::from("/x/a.RAF"), 2);
        // A label field showing "2" for copy 2 is showing the derived value, not a stored one.
        assert_eq!(
            label_commit(&id, "2", Some("Warm")),
            Some(Rename::Label(None))
        );
        assert_eq!(
            label_commit(&id, "  ", Some("Warm")),
            Some(Rename::Label(None)),
            "clearing the field clears the label"
        );
        assert_eq!(
            label_commit(&id, "B&W", Some("Warm")),
            Some(Rename::Label(Some("B&W".into()))),
            "a real label is kept exactly as typed, ampersand and all"
        );
        assert_eq!(
            label_commit(&id, "Warm", Some("Warm")),
            None,
            "unchanged commits nothing, or Ctrl+Z would appear to do nothing"
        );
    }

    /// The decision `field` makes on commit in `Editing::Label`, without a `Ui`.
    fn label_commit(id: &PhotoId, typed: &str, current: Option<&str>) -> Option<Rename> {
        if typed == current.unwrap_or_default() {
            return None;
        }
        let trimmed = typed.trim();
        let derived = id.copy.to_string();
        let to = (!trimmed.is_empty() && trimmed != derived).then(|| trimmed.to_string());
        (to.as_deref() != current).then_some(Rename::Label(to))
    }
}
