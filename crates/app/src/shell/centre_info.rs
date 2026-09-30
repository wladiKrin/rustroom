//! The centred photo name and its facts, and the gap they share with the bar's two sides.

use super::top_bar::{view_style, VIEW_ITEMS};
use crate::segmented;
use crate::{develop, name_edit, AppView, RustroomApp};
use eframe::egui;
use rustroom_app::icons;
use rustroom_app::photo_id::PhotoId;

/// The gap the centred name may use: this view's own, intersected with the other view's.
///
/// ⚠ Each view records its cluster edges **as distances from the bar's edges** (so a resized
/// window does not stale them) every frame it is drawn; the other view's are read back. Develop's
/// left cluster is the one that binds, so until Develop has been drawn once its width is seeded
/// from `develop::toolbar_width` — the first switch of a session must not jump either.
/// `the_centred_name_sits_at_the_same_x_in_both_views` pins it.
pub(super) fn shared_gap(
    ui: &egui::Ui,
    view: AppView,
    row: egui::Rect,
    left: f32,
    right: f32,
    app: &RustroomApp,
) -> egui::Rangef {
    let key = |v: AppView| egui::Id::new(("top_bar_edges", v == AppView::Develop));
    let own = (left - row.left(), row.right() - right);
    ui.ctx().data_mut(|d| d.insert_temp(key(view), own));
    let other_view = if view == AppView::Library {
        AppView::Develop
    } else {
        AppView::Library
    };
    let recorded: Option<(f32, f32)> = ui.ctx().data(|d| d.get_temp(key(other_view)));
    let other = recorded.or_else(|| {
        (other_view == AppView::Develop && app.session.is_some()).then(|| {
            let gap = ui.spacing().item_spacing.x;
            let pad = ui.spacing().button_padding.x * 2.0;
            let export = ui
                .painter()
                .layout_no_wrap(
                    format!("{} Export…", icons::EXPORT),
                    egui::TextStyle::Button.resolve(ui.style()),
                    egui::Color32::PLACEHOLDER,
                )
                .size()
                .x
                + pad;
            (
                segmented::pill_width(ui, &VIEW_ITEMS, &view_style())
                    + gap
                    + SEPARATOR
                    + gap
                    + develop::toolbar_width(ui),
                export + gap + SEPARATOR + gap,
            )
        })
    });
    match other {
        Some((ol, or)) => egui::Rangef::new(left.max(row.left() + ol), right.min(row.right() - or)),
        None => egui::Rangef::new(left, right),
    }
}

/// A separator's own width in a horizontal row (egui's default `Separator` spacing).
const SEPARATOR: f32 = 6.0;

/// The centred line naming the current photo: its name, then in weak small text its camera,
/// decoded size and file size.
///
/// ⚠ A child `Ui` placed in the gap, not a third laid-out cluster: the bar's left cluster and
/// its right-to-left cluster own the row's allocation, and a third would be exactly the "the
/// status was pushed into a row that had no space left" bug the right cluster was created to
/// fix. `new_child` places without advancing the parent. The child is centred on the bar and
/// pulled into the gap when the centre is not in it.
///
/// ⚠ The name is the Library caption's widget: a click opens the file-rename field, Enter
/// commits, clicking away abandons (user's call, 2026-09-20 — the Develop panel's footer, which
/// carried the filename and the copy-label field, is gone). The draft is the bar's own, never
/// the grid's: a photo selected in the grid would otherwise open two fields on one string.
///
/// ⚠ Two faces on purpose: the name in the body face and the strong colour, the rest in the
/// 11 pt weak face — as one run of body text "the name and file format and resolution and size
/// all read the same" (seen in use, 2026-09-20).
pub(crate) fn centre_info(
    app: &mut RustroomApp,
    ui: &mut egui::Ui,
    row: egui::Rect,
    gap: egui::Rangef,
) -> Option<(PhotoId, name_edit::Rename)> {
    // ⚠⚠ **Which photo** depends on the view; **what is said about it** does not. The facts
    // come from one rule (`photo_info::facts`) fed the same two sources in both views — the
    // catalog's capture scan and the open session — so switching views with a photo selected
    // never changes the line ("the content of the name should not change when switching",
    // seen in use 2026-09-20: Develop read the camera off the decode and showed the pixel size,
    // the Library read the catalog and showed neither).
    let (path, copy) = match app.view {
        AppView::Develop => {
            let s = app.session.as_ref()?;
            (s.path.clone(), s.copy)
        }
        AppView::Library => {
            let lib = app.library.as_ref()?;
            let e = lib.entries.get(lib.selection.active()?)?;
            (e.id.path.clone(), e.id.copy)
        }
    };
    let catalog_camera = app
        .library
        .as_ref()
        .and_then(|lib| lib.captures.get(&path))
        .and_then(|c| c.camera.clone());
    let session = app.session.as_ref().map(|s| {
        (
            s.path.clone(),
            s.make_model.clone(),
            (s.buffer.width(), s.buffer.height()),
        )
    });
    let facts = rustroom_app::photo_info::facts(
        &path,
        catalog_camera.as_deref(),
        session
            .as_ref()
            .map(|(p, m, d)| (p.as_path(), m.as_str(), *d)),
    );
    let id = PhotoId::new(path.clone(), copy);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let bytes = app.info_sizes.bytes_of(&path);
    let meta = rustroom_app::photo_info::meta(facts.camera, facts.dims, bytes);

    const PAD: f32 = 12.0;
    const BETWEEN: f32 = 10.0;
    let avail = (gap.max - gap.min - PAD * 2.0).max(0.0);
    if avail < 60.0 {
        return None;
    }
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::FontId::proportional(crate::theme::tokens::CAPTION);
    let editing = app.info_draft.as_ref().is_some_and(|(d, _)| *d == id);
    let name_w = ui.fonts_mut(|f| {
        let shown = rustroom_app::entries::caption(&file_name, copy);
        f.layout_no_wrap(shown, body.clone(), egui::Color32::WHITE)
            .size()
            .x
    });
    // ⚠⚠ **The NAME is centred, never the name-and-facts block** (2026-09-29, seen in use): the
    // facts differ between the views — the Library knows no pixel size until the photo has been
    // decoded — so a centred block put the name 27 pt further left in the Library than in
    // Develop. The name is clamped by its own width into the shared gap and the facts take what
    // is left to its right, truncating if they must; `the_name_does_not_move_with_its_facts`.
    // The rename field still wants the whole gap.
    let (x, w) = if editing {
        let x = (row.center().x - avail / 2.0).max(gap.min + PAD);
        (x, avail)
    } else {
        let name_box = name_w + 2.0;
        let x = (row.center().x - name_box / 2.0)
            .max(gap.min + PAD)
            .min(gap.max - PAD - name_box);
        (x, (gap.max - PAD - x).max(name_box))
    };
    let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), egui::vec2(w, row.height()));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("centre_info"),
    );
    child.spacing_mut().item_spacing.x = BETWEEN;
    let strong = child.visuals().strong_text_color();
    child.visuals_mut().override_text_color = Some(strong);
    let rename = name_edit::caption_or_field(
        &mut child,
        &id,
        &file_name,
        None,
        &mut app.info_draft,
        body.size,
    );
    child.visuals_mut().override_text_color = None;
    if !editing && !meta.is_empty() {
        let weak = child.visuals().weak_text_color();
        child.add(
            egui::Label::new(egui::RichText::new(meta).font(small).color(weak))
                .selectable(false)
                .truncate(),
        );
    }
    rename.map(|r| (id, r))
}
