//! A photo's right-click menu, shared by the grid and the filmstrip, and what its actions do.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::photo_id::PhotoId;
use std::path::PathBuf;

/// What a photo's right-click menu can do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellAction {
    /// Open it in Develop.
    Edit,
    /// Rate, flag or label every target — the digits' and P/X/U's work, findable with a mouse.
    Mark(rustroom_app::marks::MarkAction),
    /// The Export dialog for the selection.
    Export,
    /// Open it and make a virtual copy of it.
    VirtualCopy,
    /// Show the file in the platform's file manager.
    Reveal,
    /// Put its caption into edit mode.
    Rename,
    /// Show everything known about it.
    Info,
    /// Add every target to a collection.
    ///
    /// ⚠ Acts on the **selection**, not on the right-clicked index — which is why the menu
    /// selects the photo first when it is not already selected.
    AddToCollection(i64),
    /// Delete the virtual copies among the targets.
    DeleteVirtualCopies,
    /// Take every target out of the collection on screen.
    RemoveFromCollection(i64),
    /// Assign a keyword across the targets, or remove it from all of them.
    SetKeyword { id: i64, on: bool },
}

/// The photos a right-click acts on.
///
/// ⚠⚠ **The clicked photo alone when it is not selected**, because `menu_select` is applied
/// *after* the frame — so while the menu is drawn `lib.selection` still holds the set that is
/// about to be replaced. Reading the selection unconditionally would build the menu for photos
/// the click has already discarded.
pub fn menu_targets(lib: &crate::Library, clicked: usize) -> Vec<PhotoId> {
    if lib.selection.contains(clicked) {
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i))
            .map(|e| e.id.clone())
            .collect()
    } else {
        lib.entries
            .get(clicked)
            .map(|e| vec![e.id.clone()])
            .into_iter()
            .flatten()
            .collect()
    }
}

/// The distinct **files** among some targets.
///
/// ⚠ Keywords are per file and collections per copy, so the two lists differ deliberately: two
/// copies of one photo are one file to tag and two rows to collect.
pub fn distinct_files(targets: &[PhotoId]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for t in targets {
        if !files.contains(&t.path) {
            files.push(t.path.clone());
        }
    }
    files
}

/// The keyword menu's three states.
///
/// ⚠⚠ **Probed, never assumed.** `✔` and `✓` are in **no** bundled font and would have shipped
/// as tofu boxes — the same finding the rail's four glyphs and the mark badges each produced.
/// `every_keyword_mark_is_renderable` is the standing guard.
const MARK_ALL: &str = "■ ";

const MARK_SOME: &str = "– ";

const MARK_NONE: &str = "☐ ";

/// The photo context menu, shared by the grid and the filmstrip so the two cannot drift.
///
/// ⚠ **The catalog queries live in here**, which egui runs only while the menu is open — reading
/// them beside the grid would run three queries at 60 Hz, the `apply_view_demosaic` mistake in
/// another costume.
pub fn cell_menu(
    ui: &mut egui::Ui,
    catalog: Option<&catalog::db::Catalog>,
    source: &crate::LibrarySource,
    targets: &[PhotoId],
) -> Option<CellAction> {
    let mut action = None;
    let n = targets.len();
    // ⚠ Rename and Information are single-photo by nature, so they are **disabled** above one
    // rather than hidden: a menu that changes shape is harder to learn than one that greys out.
    let single = n == 1;
    if ui.button("Edit").clicked() {
        action = Some(CellAction::Edit);
        ui.close();
    }
    if ui
        .add_enabled(single, egui::Button::new("Rename"))
        .on_disabled_hover_text("One photo at a time")
        .clicked()
    {
        action = Some(CellAction::Rename);
        ui.close();
    }
    if ui
        .add_enabled(single, egui::Button::new("Information…"))
        .on_disabled_hover_text("One photo at a time")
        .clicked()
    {
        action = Some(CellAction::Info);
        ui.close();
    }
    ui.separator();
    // ⚠ The marks a mouse can reach: the digits and P/X/U do this too, and nothing in the grid
    // said so — "I cannot find how to say which photo I like" (A-12, P-16).
    if catalog.is_some() {
        ui.menu_button("Rating", |ui| {
            for stars in (0..=5u8).rev() {
                let text = if stars == 0 {
                    "None".to_string()
                } else {
                    "★".repeat(stars as usize)
                };
                if ui.button(text).clicked() {
                    action = Some(CellAction::Mark(rustroom_app::marks::MarkAction::Rating(
                        stars,
                    )));
                    ui.close();
                }
            }
        });
        ui.menu_button("Flag", |ui| {
            for (label, flag) in [
                ("Pick", catalog::db::Flag::Pick),
                ("Reject", catalog::db::Flag::Reject),
                ("Unflag", catalog::db::Flag::None),
            ] {
                if ui.button(label).clicked() {
                    action = Some(CellAction::Mark(rustroom_app::marks::MarkAction::Flag(
                        flag,
                    )));
                    ui.close();
                }
            }
        });
        ui.menu_button("Colour", |ui| {
            if ui.button("None").clicked() {
                action = Some(CellAction::Mark(rustroom_app::marks::MarkAction::Label(0)));
                ui.close();
            }
            for (code, name) in rustroom_app::xmp_sync::LABELS {
                if ui.button(name).clicked() {
                    action = Some(CellAction::Mark(rustroom_app::marks::MarkAction::Label(
                        code,
                    )));
                    ui.close();
                }
            }
        });
        ui.separator();
    }
    if ui.button("Export…").clicked() {
        action = Some(CellAction::Export);
        ui.close();
    }
    if ui
        .add_enabled(single, egui::Button::new("Create virtual copy"))
        .on_disabled_hover_text("One photo at a time")
        .clicked()
    {
        action = Some(CellAction::VirtualCopy);
        ui.close();
    }
    // ⚠ Only when a target IS a virtual copy — an original is the file, and deleting files is not
    // what this menu does.
    let copies = targets.iter().filter(|t| t.copy != 0).count();
    if copies > 0 {
        let label = match copies {
            1 => "Delete virtual copy".to_string(),
            n => format!("Delete {n} virtual copies"),
        };
        if ui.button(label).clicked() {
            action = Some(CellAction::DeleteVirtualCopies);
            ui.close();
        }
    }
    if ui
        .add_enabled(single, egui::Button::new("Show in Finder"))
        .on_disabled_hover_text("One photo at a time")
        .clicked()
    {
        action = Some(CellAction::Reveal);
        ui.close();
    }
    let Some(cat) = catalog else {
        return action;
    };
    ui.separator();

    let label = if single {
        "Add to collection".to_string()
    } else {
        format!("Add {n} to collection")
    };
    ui.menu_button(label, |ui| {
        let rows = cat.collections().unwrap_or_default();
        if rows.is_empty() {
            crate::theme::hint(ui, "No collections yet.");
        }
        for c in &rows {
            if ui.button(&c.name).clicked() {
                action = Some(CellAction::AddToCollection(c.id));
                ui.close();
            }
        }
    });

    // ⚠ Only while a collection is on screen: "remove from" needs one to mean anything, and the
    // rail's `−` reaches every other collection from anywhere.
    if let crate::LibrarySource::Collection { id, name } = source {
        if ui.button(format!("Remove from “{name}”")).clicked() {
            action = Some(CellAction::RemoveFromCollection(*id));
            ui.close();
        }
    }

    ui.menu_button("Keywords", |ui| {
        if let Some(a) = keyword_menu(ui, cat, targets) {
            action = Some(a);
            ui.close();
        }
    });
    action
}

/// The Keywords submenu: the targets' suggestions first (M10.8a), then the whole tree, every
/// entry with its tri-state mark.
///
/// ⚠ The suggestions are queried here, uncached: a menu's body runs only while it is open, the
/// rule this file already follows for its other queries.
fn keyword_menu(
    ui: &mut egui::Ui,
    cat: &catalog::db::Catalog,
    targets: &[PhotoId],
) -> Option<CellAction> {
    let kws = cat.keywords().unwrap_or_default();
    if kws.is_empty() {
        crate::theme::hint(ui, "No keywords yet.");
        return None;
    }
    let files = distinct_files(targets);
    let counts = cat.keyword_counts(&files).unwrap_or_default();
    let mut action = None;
    let mut item = |ui: &mut egui::Ui, k: &catalog::db::KeywordRow, depth: usize| {
        let have = counts.get(&k.id).copied().unwrap_or(0);
        let all = have == files.len() && have > 0;
        // ⚠ Three states, not two: filled when every file carries it, a dash when only some
        // do. A two-state tick on a mixed selection claims something false about half of it.
        let mark = if all {
            MARK_ALL
        } else if have > 0 {
            MARK_SOME
        } else {
            MARK_NONE
        };
        let text = format!("{mark}{}{}", "  ".repeat(depth), k.name);
        if ui.button(text).clicked() {
            // Clicking makes the selection uniform: on unless everything already has it.
            action = Some(CellAction::SetKeyword { id: k.id, on: !all });
        }
    };
    let suggested =
        rustroom_app::keyword_suggest::suggest(cat, &files, rustroom_app::keyword_suggest::LIMIT);
    if !suggested.is_empty() {
        crate::theme::sub_label(ui, "Suggested");
        for k in &suggested {
            item(ui, k, 0);
        }
        ui.separator();
    }
    for k in &kws {
        item(ui, k, k.depth);
    }
    action
}

/// Apply a context-menu choice. Shared by the grid and the filmstrip so the two cannot drift.
pub fn apply_cell_action(
    app: &mut RustroomApp,
    ctx: &egui::Context,
    index: usize,
    action: CellAction,
) {
    let Some(id) = app
        .library
        .as_ref()
        .and_then(|l| l.entries.get(index).map(|e| e.id.clone()))
    else {
        return;
    };
    match action {
        CellAction::Edit => app.open_entry(ctx, index),
        // ⚠ Reuses `name_edit`'s draft rather than adding a second path. Two editable-name
        // mechanisms drifting is the bug that made copy names invisible in the grid.
        CellAction::Rename => {
            let shown = app
                .library
                .as_ref()
                .and_then(|l| l.entries.get(index))
                .map(|e| e.caption())
                .unwrap_or_default();
            app.rename_draft = Some((id, shown));
        }
        CellAction::Info => {
            app.modal = crate::modals::Modal::PhotoInfo {
                id,
                dims: None,
                description: None,
            };
        }
        // ⚠ These two act on the targets, not on `index` — see `menu_targets`.
        CellAction::AddToCollection(cid) => app.add_targets_to_collection(cid, index),
        CellAction::RemoveFromCollection(cid) => app.remove_targets_from_collection(cid, index),
        CellAction::SetKeyword { id: kid, on } => app.set_keyword_on_targets(kid, on, index),
        CellAction::Mark(mark) => {
            let targets = app
                .library
                .as_ref()
                .map(|l| menu_targets(l, index))
                .unwrap_or_default();
            app.apply_mark_to(&targets, mark);
        }
        CellAction::Export => {
            crate::commands::execute(app, ctx, rustroom_app::menu::Command::Export);
        }
        // ⚠ The clicked photo, never "open it, then copy the open one": that put the Library into
        // Develop (2026-10-01).
        CellAction::VirtualCopy => app.create_copy_of(ctx, &id),
        CellAction::DeleteVirtualCopies => {
            let targets = app
                .library
                .as_ref()
                .map(|l| menu_targets(l, index))
                .unwrap_or_default();
            app.delete_copies(ctx, &targets);
        }
        CellAction::Reveal => {
            if let Err(e) = rustroom_app::reveal::in_file_manager(&id.path) {
                app.status = format!("Could not show the file: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_right_click_inside_the_selection_targets_the_whole_selection() {
        let mut lib = crate::test_app::library_of(5);
        lib.selection.select_indices(&[1, 2, 3]);

        let t = menu_targets(&lib, 2);

        assert_eq!(t.len(), 3, "all three, not just the one clicked");
    }

    #[test]
    fn a_right_click_outside_the_selection_targets_only_that_photo() {
        // ⚠⚠ The reason this function exists. `menu_select` is applied *after* the frame, so
        // while the menu is drawn the selection still holds the set the click is about to
        // replace — reading it would build the menu for photos already discarded.
        let mut lib = crate::test_app::library_of(5);
        lib.selection.select_indices(&[0, 1]);

        let t = menu_targets(&lib, 4);

        assert_eq!(t.len(), 1);
        assert_eq!(t[0], lib.entries[4].id);
    }

    #[test]
    fn two_copies_of_one_photo_are_one_file_to_tag() {
        // ⚠ Keywords are per file and collections per copy, so the two lists differ by design.
        let lib = crate::test_app::library_of(2);
        let mut targets = vec![lib.entries[0].id.clone(), lib.entries[1].id.clone()];
        let mut copy = lib.entries[0].id.clone();
        copy.copy = 1;
        targets.push(copy);

        assert_eq!(targets.len(), 3, "three rows");
        assert_eq!(distinct_files(&targets).len(), 2, "but two files");
    }

    #[test]
    fn every_keyword_mark_is_renderable() {
        // ⚠ A frame must run first — `Context::default()` has not built its atlas, and without
        // `install_fonts` the answer is about egui's defaults, which production no longer uses.
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for m in [MARK_ALL, MARK_SOME, MARK_NONE] {
            for c in m.chars().filter(|c| !c.is_whitespace()) {
                assert!(
                    ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                    "{c} (U+{:04X}) is in no bundled font and would render as a tofu box",
                    c as u32
                );
            }
        }
    }

    /// The Keywords submenu lists the suggestions first: `Street` is on a photo taken the same day
    /// as the two targets, so it appears twice — above the separator and in the tree — and the
    /// unrelated `Cats` only once.
    #[test]
    fn the_keyword_menu_lists_the_suggestions_first() {
        let mut app = crate::test_app::for_test();
        app.library = Some(crate::test_app::library_of(3));
        crate::test_app::catalog_library(&mut app);
        let lib = app.library.as_ref().unwrap();
        let ids: Vec<PhotoId> = lib.entries.iter().map(|e| e.id.clone()).collect();
        let cat = app.catalog.as_ref().unwrap();
        for id in &ids {
            let capture = core_image::Capture {
                taken: Some(20_590 * 86_400),
                ..Default::default()
            };
            cat.set_capture(&id.path, &capture).unwrap();
        }
        let street = cat.create_keyword("Street", None).unwrap();
        cat.create_keyword("Cats", None).unwrap();
        cat.assign_keyword(street, &ids[2].path).unwrap();
        let targets = &ids[..2];
        let ctx = crate::test_ui::ctx();
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            let _ = keyword_menu(ui, cat, targets);
        });
        let ys = |name: &str| -> Vec<f32> {
            found
                .iter()
                .filter(|(l, _)| l.ends_with(name))
                .map(|(_, p)| p.y)
                .collect()
        };
        assert_eq!(
            ys("Cats").len(),
            1,
            "the sample: the tree is drawn — {found:?}"
        );
        let street_ys = ys("Street");
        assert_eq!(street_ys.len(), 2, "{found:?}");
        // The tree is by name, Cats before Street: only a suggestion can put a Street above Cats.
        let cats = ys("Cats")[0];
        let (top, bottom) = (
            street_ys.iter().copied().fold(f32::MAX, f32::min),
            street_ys.iter().copied().fold(f32::MIN, f32::max),
        );
        assert!(
            top < cats && cats < bottom,
            "the suggestion leads: {found:?}"
        );
    }

    /// "Remove from" appears while a collection is on screen, and nowhere else.
    #[test]
    fn remove_from_is_offered_only_in_a_collection() {
        let cat = catalog::db::Catalog::open_in_memory().unwrap();
        let lib = crate::test_app::library_of(1);
        let targets = vec![lib.entries[0].id.clone()];
        let labels = |source: &crate::LibrarySource| -> Vec<String> {
            crate::test_ui::nodes(&crate::test_ui::ctx(), |ui| {
                let _ = cell_menu(ui, Some(&cat), source, &targets);
            })
            .into_iter()
            .map(|n| n.label)
            .collect()
        };
        let trips = crate::LibrarySource::Collection {
            id: 7,
            name: "Trips".into(),
        };
        let shown = labels(&trips);
        assert!(shown.iter().any(|l| l == "Edit"), "the sample: {shown:?}");
        assert!(
            shown.iter().any(|l| l == "Remove from “Trips”"),
            "{shown:?}"
        );
        let folder = labels(&lib.source);
        assert!(
            !folder.iter().any(|l| l.starts_with("Remove from")),
            "{folder:?}"
        );
    }
}
