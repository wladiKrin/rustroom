//! The Folders panel's tree (M9.9): the catalog's flat folder rows arranged by their paths.
//!
//! ⚠ No egui — the panel draws what this returns. Each folder nests under its NEAREST known
//! ancestor, labelled by the path relative to it: an unscanned folder between two known ones (one
//! holding no photos) does not become a row, so `Pictures` over `fuji/2026-09` reads as such rather
//! than inventing an empty `fuji` row that clicks to nothing.

use catalog::db::FolderRow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One row of the tree, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: i64,
    pub path: PathBuf,
    /// The path relative to its parent node, or the full path's last component for a root.
    pub label: String,
    pub depth: usize,
    /// Photos in this folder and every folder beneath it.
    pub count: i64,
    pub has_children: bool,
}

fn is_under(child: &Path, parent: &Path) -> bool {
    child != parent && child.starts_with(parent)
}

/// The nearest row that is a strict ancestor of `path`.
fn parent_of<'a>(rows: &'a [FolderRow], path: &Path) -> Option<&'a FolderRow> {
    rows.iter()
        .filter(|r| is_under(path, &r.path))
        .max_by_key(|r| r.path.components().count())
}

/// The tree in display order: depth-first, children sorted by label, case-insensitively. A node
/// whose path is in `collapsed` is listed, its descendants are not.
pub fn build(rows: &[FolderRow], collapsed: &HashSet<PathBuf>) -> Vec<Node> {
    let mut out = Vec::new();
    walk(rows, None, 0, collapsed, &mut out);
    out
}

fn walk(
    rows: &[FolderRow],
    parent: Option<&Path>,
    depth: usize,
    collapsed: &HashSet<PathBuf>,
    out: &mut Vec<Node>,
) {
    let mut kids: Vec<&FolderRow> = rows
        .iter()
        .filter(|r| parent_of(rows, &r.path).map(|p| p.path.as_path()) == parent)
        .collect();
    let label = |r: &FolderRow| match parent {
        Some(p) => r
            .path
            .strip_prefix(p)
            .unwrap_or(&r.path)
            .display()
            .to_string(),
        None => r
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| r.path.display().to_string()),
    };
    kids.sort_by_key(|r| (label(r).to_lowercase(), label(r)));
    for r in kids {
        let has_children = rows.iter().any(|c| is_under(&c.path, &r.path));
        out.push(Node {
            id: r.id,
            path: r.path.clone(),
            label: label(r),
            depth,
            count: subtree(rows, &r.path).map(|f| f.count).sum(),
            has_children,
        });
        if !collapsed.contains(&r.path) {
            walk(rows, Some(&r.path), depth + 1, collapsed, out);
        }
    }
}

/// `path`'s row and every row beneath it — what clicking it shows, and what forgetting it forgets.
pub fn subtree<'a>(rows: &'a [FolderRow], path: &'a Path) -> impl Iterator<Item = &'a FolderRow> {
    rows.iter()
        .filter(move |r| r.path == path || is_under(&r.path, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, path: &str, count: i64) -> FolderRow {
        FolderRow {
            id,
            path: PathBuf::from(path),
            count,
        }
    }

    /// The author's catalog, plus a subfolder two levels under `fuji` whose middle folder holds
    /// no photos (so has no row), and a second root elsewhere.
    fn rows() -> Vec<FolderRow> {
        vec![
            row(1, "/Users/w/Pictures/fuji", 400),
            row(2, "/Users/w/Pictures/fuji/2026/09-27", 30),
            row(3, "/Users/w/Pictures/fuji_new_new", 800),
            row(4, "/Users/w/Desktop/512724-521236", 37),
            row(5, "/Users/w/Pictures/fuji/Zeta", 5),
        ]
    }

    fn shown(nodes: &[Node]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| format!("{}{} {}", "  ".repeat(n.depth), n.label, n.count))
            .collect()
    }

    #[test]
    fn folders_nest_under_their_nearest_known_ancestor() {
        let t = build(&rows(), &HashSet::new());
        assert_eq!(
            shown(&t),
            vec![
                "512724-521236 37",
                "fuji 435",
                "  2026/09-27 30",
                "  Zeta 5",
                "fuji_new_new 800",
            ]
        );
        assert!(t[1].has_children && !t[2].has_children);
    }

    #[test]
    fn a_collapsed_folder_hides_its_descendants_but_keeps_its_count() {
        let collapsed: HashSet<PathBuf> = [PathBuf::from("/Users/w/Pictures/fuji")].into();
        let t = build(&rows(), &collapsed);
        assert_eq!(
            shown(&t),
            vec!["512724-521236 37", "fuji 435", "fuji_new_new 800"]
        );
    }

    /// ⚠ `fuji_new_new` starts with the TEXT `fuji` but is not under the folder `fuji` — a string
    /// prefix test would nest it there. Paths compare by component.
    #[test]
    fn a_sibling_sharing_a_name_prefix_is_not_a_child() {
        let ids: Vec<i64> = subtree(&rows(), Path::new("/Users/w/Pictures/fuji"))
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![1, 2, 5]);
    }
}
