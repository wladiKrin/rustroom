//! The Folders panel's tree (M9.9): the catalog's flat folder rows arranged by their paths.
//!
//! ⚠ No egui — the panel draws what this returns. Each folder nests under its NEAREST known
//! ancestor, labelled by the path relative to it: an unscanned folder between two known ones (one
//! holding no photos) does not become a row, so `Pictures` over `fuji/2026-09` reads as such rather
//! than inventing an empty `fuji` row that clicks to nothing.

use catalog::db::FolderRow;
use std::collections::{HashMap, HashSet};
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

/// The rows arranged once: each row's children and its subtree's photo count.
struct Forest {
    roots: Vec<usize>,
    kids: Vec<Vec<usize>>,
    total: Vec<i64>,
}

impl Forest {
    /// Each row's parent is its NEAREST known ancestor — the first of its own path's ancestors that
    /// is a row — found by walking up the path through an index, `O(rows × depth)`.
    fn new(rows: &[FolderRow]) -> Self {
        let index: HashMap<&Path, usize> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.path.as_path(), i))
            .collect();
        let parent: Vec<Option<usize>> = rows
            .iter()
            .map(|r| {
                r.path
                    .ancestors()
                    .skip(1)
                    .find_map(|a| index.get(a).copied())
            })
            .collect();
        let mut roots = Vec::new();
        let mut kids = vec![Vec::new(); rows.len()];
        for (i, p) in parent.iter().enumerate() {
            match p {
                Some(p) => kids[*p].push(i),
                None => roots.push(i),
            }
        }
        // A parent is a strict ancestor, so it has fewer components than any row beneath it:
        // deepest first, every subtree is complete before it is added to its parent's.
        let mut total: Vec<i64> = rows.iter().map(|r| r.count).collect();
        let mut deepest_first: Vec<usize> = (0..rows.len()).collect();
        deepest_first.sort_by_key(|&i| std::cmp::Reverse(rows[i].path.components().count()));
        for i in deepest_first {
            if let Some(p) = parent[i] {
                total[p] += total[i];
            }
        }
        Self { roots, kids, total }
    }
}

/// The tree in display order: depth-first, children sorted by label, case-insensitively. A node
/// whose path is in `collapsed` is listed, its descendants are not.
///
/// ⚠⚠ **Called on EVERY FRAME** — the Library's Folders rail draws it — so it must stay
/// near-linear. It was cubic (every expanded node filtered every row, and each row's parent
/// scanned every row again): 211 folders, 189 of them another app's thumbnail shards scanned in
/// under `Pictures`, took 93% of the Library's main thread while Develop, which draws no rail,
/// stayed fast (sampled, 2026-10-12). `a_large_catalog_builds_in_well_under_a_frame` pins it;
/// the cubic version is kept in the tests as the reference the output must match.
pub fn build(rows: &[FolderRow], collapsed: &HashSet<PathBuf>) -> Vec<Node> {
    let forest = Forest::new(rows);
    let mut out = Vec::new();
    walk(rows, &forest, &forest.roots, None, 0, collapsed, &mut out);
    out
}

fn walk(
    rows: &[FolderRow],
    forest: &Forest,
    ids: &[usize],
    parent: Option<&Path>,
    depth: usize,
    collapsed: &HashSet<PathBuf>,
    out: &mut Vec<Node>,
) {
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
    // ⚠ A stable sort over rows in catalog order, so equal labels keep the order they always had.
    let mut kids: Vec<(String, String, usize)> = ids
        .iter()
        .map(|&i| {
            let l = label(&rows[i]);
            (l.to_lowercase(), l, i)
        })
        .collect();
    kids.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (_, label, i) in kids {
        let r = &rows[i];
        out.push(Node {
            id: r.id,
            path: r.path.clone(),
            label,
            depth,
            count: forest.total[i],
            has_children: !forest.kids[i].is_empty(),
        });
        if !collapsed.contains(&r.path) {
            walk(
                rows,
                forest,
                &forest.kids[i],
                Some(&r.path),
                depth + 1,
                collapsed,
                out,
            );
        }
    }
}

/// `path`'s row and every row beneath it — what clicking it shows, and what forgetting it forgets.
pub fn subtree<'a>(rows: &'a [FolderRow], path: &'a Path) -> impl Iterator<Item = &'a FolderRow> {
    rows.iter()
        .filter(move |r| r.path == path || is_under(&r.path, path))
}

/// Every row in any of `paths`' subtrees, each ONCE, in `rows` order — what forgetting `paths`
/// removes (M10.19c).
///
/// ⚠ A marked folder beneath another marked one is in both subtrees; counting it twice would
/// inflate the confirmation's totals, and forgetting it twice would fail the second delete.
pub fn subtrees<'a>(rows: &'a [FolderRow], paths: &[PathBuf]) -> Vec<&'a FolderRow> {
    rows.iter()
        .filter(|r| paths.iter().any(|p| r.path == *p || is_under(&r.path, p)))
        .collect()
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

    #[test]
    fn subtrees_counts_a_nested_path_once() {
        let ps = [
            PathBuf::from("/Users/w/Pictures/fuji"),
            PathBuf::from("/Users/w/Pictures/fuji/2026/09-27"),
        ];
        let ids: Vec<i64> = subtrees(&rows(), &ps).iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 2, 5]);
    }

    /// The shape that stalled the Library (2026-10-12): another app's sharded thumbnail cache
    /// scanned in under `Pictures` — a cache root, a `thumbs` folder, `shards` shard folders, each
    /// with `per` subfolders. Labels mix case so the case-insensitive sort is exercised.
    fn sharded(shards: usize, per: usize) -> Vec<FolderRow> {
        let root = "/Users/w/Pictures/LightCraft Library";
        let mut out = vec![row(100, root, 0), row(101, &format!("{root}/thumbs"), 2)];
        let mut id = 102;
        for s in 0..shards {
            let shard = format!("{root}/thumbs/{s:02X}");
            out.push(row(id, &shard, 1));
            id += 1;
            for k in 0..per {
                let name = if k % 2 == 0 { "a" } else { "B" };
                out.push(row(id, &format!("{shard}/{name}{k}"), 1));
                id += 1;
            }
        }
        out
    }

    /// ⚠⚠ **The rail rebuilds this tree on EVERY FRAME** (`folder_rows::rows`), and it was cubic:
    /// each expanded node filtered every row, and each row's parent scanned every row again. With
    /// 211 folders — 189 of them another app's thumbnail shards — it took 93% of the main thread's
    /// time in the Library while Develop, which draws no rail, stayed fast (sampled, 2026-10-12).
    /// ⚠ The bound is ~50× what a near-linear build costs in a debug build and orders of magnitude
    /// below the cubic one's, so it measures the algorithm, not the machine.
    #[test]
    fn a_large_catalog_builds_in_well_under_a_frame() {
        let rows = sharded(30, 15);
        assert!(rows.len() > 450, "fixture: {} rows", rows.len());
        let t = std::time::Instant::now();
        let tree = build(&rows, &HashSet::new());
        let ms = t.elapsed().as_secs_f64() * 1e3;
        assert_eq!(tree.len(), rows.len(), "fixture: every folder is a node");
        assert!(
            ms < 100.0,
            "building {} folders took {ms:.1} ms",
            rows.len()
        );
    }

    /// The build before 2026-10-12, kept as the reference: obviously correct, and cubic.
    fn reference(rows: &[FolderRow], collapsed: &HashSet<PathBuf>) -> Vec<Node> {
        fn parent_of<'a>(rows: &'a [FolderRow], path: &Path) -> Option<&'a FolderRow> {
            rows.iter()
                .filter(|r| is_under(path, &r.path))
                .max_by_key(|r| r.path.components().count())
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
                out.push(Node {
                    id: r.id,
                    path: r.path.clone(),
                    label: label(r),
                    depth,
                    count: subtree(rows, &r.path).map(|f| f.count).sum(),
                    has_children: rows.iter().any(|c| is_under(&c.path, &r.path)),
                });
                if !collapsed.contains(&r.path) {
                    walk(rows, Some(&r.path), depth + 1, collapsed, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(rows, None, 0, collapsed, &mut out);
        out
    }

    /// Every shape the tree knows — nearest-known-ancestor nesting across an unscanned folder,
    /// name-prefix siblings, two roots, mixed-case labels, deep shards — and every kind of
    /// collapse, against the reference. ⚠ Small enough that the cubic reference stays quick.
    #[test]
    fn the_build_matches_the_reference_on_every_shape() {
        let mut all = rows();
        all.extend(sharded(4, 5));
        all.push(row(200, "/Users/w/Pictures/fuji/2026/09-27/raw", 3));
        all.push(row(201, "/Volumes/Card/DCIM", 9));
        let paths = |s: &[&str]| -> HashSet<PathBuf> { s.iter().map(PathBuf::from).collect() };
        let collapses = [
            HashSet::new(),
            paths(&["/Users/w/Pictures/fuji"]),
            paths(&["/Users/w/Pictures/LightCraft Library/thumbs"]),
            paths(&[
                "/Users/w/Pictures/LightCraft Library/thumbs/02",
                "/Users/w/Pictures/fuji/2026/09-27",
            ]),
        ];
        assert!(
            reference(&all, &collapses[0]).iter().any(|n| n.depth >= 3),
            "fixture: the tree is deep"
        );
        for c in &collapses {
            assert_eq!(build(&all, c), reference(&all, c), "collapsed: {c:?}");
        }
        assert!(
            reference(&all, &collapses[2]).len() < reference(&all, &collapses[0]).len(),
            "fixture: a collapse hides something"
        );
    }
}
