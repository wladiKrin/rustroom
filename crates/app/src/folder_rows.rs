//! The Folders section's tree rows (M9.9): each catalog folder nested under its nearest known
//! ancestor, with a disclosure triangle for the ones that have folders beneath.
//!
//! ⚠ Its own module for `keyword_rows`' reason: `collections_panel.rs` is past 850 lines of code.
//! The tree itself is `rustroom_app::folder_tree`, pure and tested; this draws it.
//!
//! ⚠⚠ **The triangle is PAINTED, not a glyph.** The icon font is Phosphor with a version-3 `post`
//! table — no glyph names — so a caret codepoint could only be taken from its stylesheet on trust,
//! and this project has shipped glyphs that rendered as empty boxes twice (`icons.rs` records it).
//! A painted triangle cannot be missing from a font.

use crate::list_row::{self, RowAction, RowSpec};
use catalog::db::FolderRow;
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// What a row asked for, applied after the panel closes.
pub(crate) enum Out {
    Open(PathBuf),
    Forget(i64, PathBuf, i64),
}

const INDENT: f32 = 12.0;
/// The triangle's gutter, left of the name, in points.
const DISCLOSURE: f32 = 12.0;

pub(crate) struct View<'a> {
    pub folders: &'a [FolderRow],
    /// The Immich cache root, whose row is named and counted differently — see the caller.
    pub immich_root: Option<&'a Path>,
    pub immich_count: i64,
    /// The folder the Library shows, if it is one.
    pub open: Option<&'a Path>,
}

pub(crate) fn rows(
    ui: &mut egui::Ui,
    view: View<'_>,
    collapsed: &mut HashSet<PathBuf>,
) -> Option<Out> {
    let mut out = None;
    for node in rustroom_app::folder_tree::build(view.folders, collapsed) {
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
        let indent = node.depth as f32 * INDENT + DISCLOSURE;
        let r = list_row::row(
            ui,
            RowSpec {
                name: &shown,
                count: Some(count),
                meta: "",
                selected: view.open == Some(node.path.as_path()),
                indent,
                hover: &path,
                actions: &[RowAction {
                    glyph: '×',
                    verb: "Forget",
                    hover: "Forget this folder and every folder beneath it",
                    enabled: true,
                }],
            },
        );
        let mut toggled = false;
        if node.has_children {
            let open = !collapsed.contains(&node.path);
            let centre = egui::pos2(r.rect.left() + indent - DISCLOSURE / 2.0, r.rect.center().y);
            // ⚠ Registered after the row, so it wins the click there — the row's own actions work
            // the same way.
            let hit = ui.interact(
                egui::Rect::from_center_size(centre, egui::vec2(DISCLOSURE, r.rect.height())),
                ui.id().with(("folder_disclosure", &node.path)),
                egui::Sense::click(),
            );
            let colour = ui.visuals().text_color();
            let s = 3.5;
            let pts = if open {
                vec![
                    centre + egui::vec2(-s, -s * 0.6),
                    centre + egui::vec2(s, -s * 0.6),
                    centre + egui::vec2(0.0, s * 0.8),
                ]
            } else {
                vec![
                    centre + egui::vec2(-s * 0.6, -s),
                    centre + egui::vec2(s * 0.8, 0.0),
                    centre + egui::vec2(-s * 0.6, s),
                ]
            };
            ui.painter()
                .add(egui::Shape::convex_polygon(pts, colour, egui::Stroke::NONE));
            if hit.clicked() {
                toggled = true;
                if open {
                    collapsed.insert(node.path.clone());
                } else {
                    collapsed.remove(&node.path);
                }
            }
        }
        // The Forget total covers the subtree, as does the forget itself.
        if r.action == Some(0) {
            out = Some(Out::Forget(node.id, node.path.clone(), node.count));
        } else if r.clicked && !toggled {
            out = Some(Out::Open(node.path.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

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
            );
        };
        let fuji = test_ui::find(&ctx, "fuji", &mut body).expect("fuji is drawn");
        // The row's rect starts at the panel's left edge; the triangle is DISCLOSURE/2 in.
        let triangle = egui::pos2(DISCLOSURE / 2.0 + 1.0, fuji.y);
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
