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

/// What the name field committed: a file on disk, or one copy's own name.
pub enum Rename {
    File(PathBuf),
    Copy(Option<String>),
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
        let shown = entries::shown_name(file_name, id.copy, name);
        let r = ui.add(
            egui::Label::new(egui::RichText::new(shown.clone()).font(font))
                .sense(egui::Sense::click()),
        );
        if r.clicked() {
            *draft = Some((id.clone(), shown));
        }
        return None;
    }
    field(ui, id, file_name, name, draft, font, true)
}

/// An editable name, drawn as a text field.
///
/// ⚠ Edits a **scratch** draft, not the stored value. Writing through would rename the file on
/// every keystroke — creating a file per character — and would commit a half-typed name the moment
/// focus moved. Same shape the export dialog's `export_draft` uses.
pub fn field(
    ui: &mut egui::Ui,
    id: &PhotoId,
    file_name: &str,
    name: Option<&str>,
    draft: &mut Option<(PhotoId, String)>,
    font: egui::FontId,
    autofocus: bool,
) -> Option<Rename> {
    let on_copy = !id.is_original();
    let default = file_rename::default_copy_name(file_name, id.copy);
    // ⚠ The same function the Library caption uses, so the field can never show a name
    // different from the one beside it.
    let current = entries::shown_name(file_name, id.copy, name);

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
    if autofocus && ui.memory(|m| m.focused()).is_none() {
        r.request_focus();
    }
    // ⚠ Read the key press into a local *before* touching `r`. `Response` methods take the
    // context lock internally, and holding one across `ui.input(..)` panics inside epaint's
    // mutex — the re-entrant lock the canvas-navigation work already paid for.
    let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));

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

    if r.lost_focus() || (r.has_focus() && entered) {
        let typed = text.clone();
        *draft = None;
        if typed == current {
            // ⚠ Nothing typed, nothing recorded — or Ctrl+Z would appear to do nothing.
            return None;
        }
        if on_copy {
            let trimmed = typed.trim();
            // ⚠ Typing the default back, or clearing the field, stores `None` rather than that
            // literal string. `None` is what makes an unnamed copy *follow* a later rename of its
            // file; freezing the derived name would silently strand it on the old stem.
            let to = (!trimmed.is_empty() && trimmed != default).then(|| trimmed.to_string());
            return (to.as_deref() != name).then_some(Rename::Copy(to));
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
        Rename::Copy(to) => {
            let from = app.copy_name_of(id);
            if app.set_copy_name(id, to.clone()) {
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
            text, "a_1.RAF",
            "pre-filled with the name it already has, not blank"
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
}
