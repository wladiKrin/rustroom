//! The Folders section's tree rows (M9.9): each catalog folder nested under its nearest known
//! ancestor, with a disclosure triangle for the ones that have folders beneath.
//!
//! ⚠ Its own module for `keyword_rows`' reason: `collections_panel.rs` is past 850 lines of code.
//! The tree itself is `rustroom_app::folder_tree`, pure and tested; this draws it.
//!
//! The triangle is `list_row::disclosure` — painted, not a glyph; its doc says why.

use crate::list_row::{self, RowAction, RowSpec};
use catalog::db::FolderRow;
use eframe::egui;
use rustroom_app::folder_marks::FolderMarks;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// What a row asked for, applied after the panel closes.
pub(crate) enum Out {
    Open(PathBuf),
    /// Forget these folders (each with everything beneath it) — after the confirmation.
    Forget(Vec<PathBuf>),
}

pub(crate) struct View<'a> {
    pub folders: &'a [FolderRow],
    /// The Immich cache root, whose row is named and counted differently — see the caller.
    pub immich_root: Option<&'a Path>,
    pub immich_count: i64,
    /// The folder the Library shows, if it is one.
    pub open: Option<&'a Path>,
}

/// Draw the tree. A plain click opens a folder; ⌘-click marks it, ⇧-click marks the visible range,
/// and a right-click forgets the marks — or that row alone (M10.19c, `FolderMarks`).
pub(crate) fn rows(
    ui: &mut egui::Ui,
    view: View<'_>,
    collapsed: &mut HashSet<PathBuf>,
    marks: &mut FolderMarks,
) -> Option<Out> {
    let mut out = None;
    marks.follow(view.open);
    // ⚠ Against EVERY catalog folder, not the visible rows: a collapse hides a mark, it does not drop
    // it. A set, because this runs every frame (`AGENTS.md`: nothing in the rail may grow per frame).
    let exists: HashSet<&Path> = view.folders.iter().map(|f| f.path.as_path()).collect();
    marks.retain(|p| exists.contains(p));
    let nodes = rustroom_app::folder_tree::build(view.folders, collapsed);
    // The rail's visible order — what a ⇧-click's range runs over.
    let order: Vec<PathBuf> = nodes.iter().map(|n| n.path.clone()).collect();
    for node in nodes {
        let is_immich = view.immich_root == Some(node.path.as_path());
        // ⚠ The basename is what fits and what identifies; the full path is the hover. ⚠ Except
        // the Immich cache root, whose basename is a host slug that says nothing — see
        // `folder_display_name`. (M9.9: a nested node shows its path from its parent instead.)
        let shown = if node.depth == 0 {
            rustroom_app::immich_catalog::folder_display_name(&node.path, view.immich_root)
        } else {
            node.label.clone()
        };
        // ⚠⚠ A catalog row exists for every asset **browsed**, while the count beside a folder is a
        // statement about that folder's *contents* — it read 981 against 17 files on disk.
        // ⚠ Only the Immich folder is overridden; a folder on an unplugged drive keeps its own
        // count, the same line slice 3 drew for `row_is_browsable`.
        let count = if is_immich {
            view.immich_count
        } else {
            node.count
        };
        let path = node.path.to_string_lossy().to_string();
        let indent = list_row::tree_indent(node.depth);
        let r = list_row::row(
            ui,
            RowSpec {
                name: &shown,
                count: Some(count),
                meta: "",
                // ⚠ The one accent for every marked row, and for the open folder (DESIGN §2).
                selected: marks.contains(&node.path) || view.open == Some(node.path.as_path()),
                indent,
                hover: &path,
                actions: &[RowAction {
                    glyph: '×',
                    verb: "Forget",
                    hover: "Removes this folder and everything under it from the catalog",
                    enabled: true,
                }],
            },
        );
        let mut toggled = false;
        if node.has_children {
            let open = !collapsed.contains(&node.path);
            let id = ui.id().with(("folder_disclosure", &node.path));
            if list_row::disclosure(ui, r.rect, indent, open, id) {
                toggled = true;
                if open {
                    collapsed.insert(node.path.clone());
                } else {
                    collapsed.remove(&node.path);
                }
            }
        }
        // ⚠ The marks only from a marked row: a right-click elsewhere acts on that row alone.
        r.response.context_menu(|ui| {
            let targets = marks.targets(&node.path);
            let label = match targets.len() {
                1 => "Forget folder".to_string(),
                n => format!("Forget {n} folders"),
            };
            if ui.button(label).clicked() {
                out = Some(Out::Forget(targets));
                ui.close();
            }
        });
        // The Forget total covers the subtree, as does the forget itself (`ask_forget_folders`).
        if r.action == Some(0) {
            out = Some(Out::Forget(vec![node.path.clone()]));
        } else if r.clicked && !toggled {
            // ⚠ The Dates section's gestures (`smart_sources::click_period`); ⌘ and ⇧ never open.
            let mods = ui.input(|i| i.modifiers);
            if mods.command {
                marks.toggle(&node.path);
            } else if mods.shift {
                marks.extend_to(&node.path, &order);
            } else {
                marks.click(&node.path);
                out = Some(Out::Open(node.path.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    /// The rail over [`folders`] with nothing open, keeping its marks and the last thing it asked.
    struct Rail {
        ctx: egui::Context,
        f: Vec<FolderRow>,
        collapsed: HashSet<PathBuf>,
        marks: FolderMarks,
        out: Option<Out>,
    }

    impl Rail {
        fn new() -> Self {
            Self {
                ctx: test_ui::ctx(),
                f: folders(),
                collapsed: HashSet::new(),
                marks: FolderMarks::default(),
                out: None,
            }
        }

        fn body(&mut self, ui: &mut egui::Ui) {
            if let Some(o) = rows(
                ui,
                View {
                    folders: &self.f,
                    immich_root: None,
                    immich_count: 0,
                    open: None,
                },
                &mut self.collapsed,
                &mut self.marks,
            ) {
                self.out = Some(o);
            }
        }

        fn click_with(&mut self, label: &str, mods: egui::Modifiers) {
            let ctx = self.ctx.clone();
            test_ui::click_with(&ctx, label, mods, |ui| self.body(ui));
        }

        /// Right-click the row labelled `row`, then choose `item` from its menu.
        fn menu(&mut self, row: &str, item: &str) {
            let ctx = self.ctx.clone();
            test_ui::right_click(&ctx, row, |ui| self.body(ui));
            test_ui::click(&ctx, item, |ui| self.body(ui));
        }

        fn forgot(&self) -> Option<Vec<PathBuf>> {
            match &self.out {
                Some(Out::Forget(paths)) => Some(paths.clone()),
                _ => None,
            }
        }
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn a_command_click_marks_without_opening() {
        let mut r = Rail::new();
        r.click_with("fuji", egui::Modifiers::COMMAND);
        assert!(r.out.is_none(), "a ⌘-click must not open the folder");
        assert!(r.marks.contains(&p("/p/fuji")));
    }

    #[test]
    fn a_shift_click_marks_the_visible_range() {
        let mut r = Rail::new();
        r.click_with("fuji", egui::Modifiers::COMMAND);
        r.click_with("other", egui::Modifiers::SHIFT);
        assert!(r.out.is_none(), "a ⇧-click must not open the folder");
        assert_eq!(
            r.marks.targets(&p("/p/other")),
            vec![p("/p/fuji"), p("/p/fuji/2026"), p("/p/other")]
        );
    }

    #[test]
    fn the_menu_forgets_every_mark_from_a_marked_row() {
        let mut r = Rail::new();
        r.click_with("fuji", egui::Modifiers::COMMAND);
        r.click_with("other", egui::Modifiers::COMMAND);
        r.menu("other", "Forget 2 folders");
        assert_eq!(r.forgot(), Some(vec![p("/p/fuji"), p("/p/other")]));
    }

    /// Review Focus 1: a right-click on an unmarked row never acts on the marks.
    #[test]
    fn a_right_click_on_an_unmarked_row_forgets_only_it() {
        let mut r = Rail::new();
        r.click_with("fuji", egui::Modifiers::COMMAND);
        r.click_with("other", egui::Modifiers::COMMAND);
        r.menu("2026", "Forget folder");
        assert_eq!(r.forgot(), Some(vec![p("/p/fuji/2026")]));
    }

    fn folders() -> Vec<FolderRow> {
        let row = |id, p: &str, count| FolderRow {
            id,
            path: PathBuf::from(p),
            count,
        };
        vec![
            row(1, "/p/fuji", 10),
            row(2, "/p/fuji/2026", 3),
            row(3, "/p/other", 4),
        ]
    }

    fn labels(collapsed: &mut HashSet<PathBuf>) -> Vec<String> {
        let f = folders();
        test_ui::labelled_centres(&test_ui::ctx(), Vec::new(), |ui| {
            let _ = rows(
                ui,
                View {
                    folders: &f,
                    immich_root: None,
                    immich_count: 0,
                    open: None,
                },
                collapsed,
                &mut FolderMarks::default(),
            );
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect()
    }

    #[test]
    fn the_tree_shows_nesting_and_a_click_on_the_triangle_collapses_it() {
        let mut collapsed = HashSet::new();
        let shown = labels(&mut collapsed);
        assert!(
            shown.iter().any(|l| l == "2026"),
            "the child is listed: {shown:?}"
        );

        // The triangle sits in the gutter left of "fuji"'s name: click there.
        let ctx = test_ui::ctx();
        let f = folders();
        let mut body = |ui: &mut egui::Ui| {
            let _ = rows(
                ui,
                View {
                    folders: &f,
                    immich_root: None,
                    immich_count: 0,
                    open: None,
                },
                &mut collapsed,
                &mut FolderMarks::default(),
            );
        };
        let fuji = test_ui::find(&ctx, "fuji", &mut body).expect("fuji is drawn");
        // The row's rect starts at the panel's left edge; the triangle is DISCLOSURE/2 in.
        let triangle = egui::pos2(list_row::DISCLOSURE / 2.0 + 1.0, fuji.y);
        test_ui::click_at(&ctx, triangle, false, &mut body);
        assert!(
            collapsed.contains(Path::new("/p/fuji")),
            "the triangle click must collapse fuji"
        );
        let shown = labels(&mut collapsed);
        assert!(
            !shown.iter().any(|l| l == "2026"),
            "the child is hidden: {shown:?}"
        );
    }

    #[test]
    fn a_click_on_the_row_opens_it_and_not_the_triangle() {
        let ctx = test_ui::ctx();
        let f = folders();
        let mut collapsed = HashSet::new();
        let mut got = None;
        let mut body = |ui: &mut egui::Ui| {
            if let Some(o) = rows(
                ui,
                View {
                    folders: &f,
                    immich_root: None,
                    immich_count: 0,
                    open: None,
                },
                &mut collapsed,
                &mut FolderMarks::default(),
            ) {
                got = Some(match o {
                    Out::Open(p) => format!("open {}", p.display()),
                    Out::Forget(..) => "forget".to_string(),
                });
            }
        };
        test_ui::click(&ctx, "fuji", &mut body);
        assert_eq!(got.as_deref(), Some("open /p/fuji"));
        assert!(collapsed.is_empty(), "a row click must not collapse");
    }
}
