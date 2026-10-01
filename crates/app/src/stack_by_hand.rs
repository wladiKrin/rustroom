//! Stacks made by hand (M10.8c): what Stack, Unstack, reordering and Stack by capture time change.
//!
//! ⚠ Pure and egui-free: each takes the current memberships of the files on view and returns the
//! **changes** — a file's new membership, `None` for out — which the app writes to the catalog and
//! records as one undo step. Every result goes through [`tidy`], so no command leaves a stack of one.
//!
//! ⚠⚠ **Two kinds of hand stack** (the author, after use): the ones *Stack by capture time* made
//! (`by_time`), which that menu manages — its *None* takes them apart and another gap replaces them —
//! and the user's own (⌘G, or any edit of a stack's order), which that menu never touches.

use crate::stack::HandStacks;
use catalog::db::HandMember;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A file's membership, or `None`.
pub type Membership = Option<HandMember>;

/// A file's new membership.
pub type Change = (PathBuf, Membership);

fn mine(stack: i64, ord: usize) -> Membership {
    Some(HandMember {
        stack,
        ord: ord as i64,
        by_time: false,
    })
}

/// Stack `files`, **per folder** (two or more each), `top` first where it is one of them — else the
/// first. A file already in a hand stack brings its whole stack along, after the files given: the
/// stacks merge. The result is the user's.
pub fn stack(
    files: &[PathBuf],
    top: Option<&Path>,
    current: &HandStacks,
    next_id: i64,
) -> Vec<Change> {
    let mut id = next_id;
    let mut changes = Vec::new();
    for (dir, group) in by_folder(files) {
        let joined: HashSet<i64> = group
            .iter()
            .filter_map(|f| current.get(f).map(|m| m.stack))
            .collect();
        let mut members = group.clone();
        if let Some(t) = top.filter(|t| members.iter().any(|m| m == t)) {
            members.retain(|m| m != t);
            members.insert(0, t.to_path_buf());
        }
        for (p, _) in in_order(current, |m| joined.contains(&m.stack)) {
            if p.parent() == Some(dir.as_path()) && !members.contains(p) {
                members.push(p.clone());
            }
        }
        if members.len() < 2 {
            continue;
        }
        for (k, m) in members.iter().enumerate() {
            changes.push((m.clone(), mine(id, k)));
        }
        id += 1;
    }
    tidy(current, changes)
}

/// Take apart every hand stack any of `files` belongs to.
pub fn unstack(files: &[PathBuf], current: &HandStacks) -> Vec<Change> {
    let ids: HashSet<i64> = files
        .iter()
        .filter_map(|f| current.get(f).map(|m| m.stack))
        .collect();
    let changes = in_order(current, |m| ids.contains(&m.stack))
        .into_iter()
        .map(|(p, _)| (p.clone(), None))
        .collect();
    tidy(current, changes)
}

/// Put `files` in this order, the first on top — a reorder by drag, *Make top of stack*, or a name
/// stack adopted by being reordered. Their shared stack keeps its id; files that share none get a new
/// one. ⚠ The result is the **user's** (`by_time: false`): editing a stack's order makes it theirs.
pub fn set_order(files: &[PathBuf], current: &HandStacks, next_id: i64) -> Vec<Change> {
    let ids: HashSet<i64> = files
        .iter()
        .filter_map(|f| current.get(f).map(|m| m.stack))
        .collect();
    let shared = files.iter().all(|f| current.contains_key(f)) && ids.len() == 1;
    let id = if shared {
        *ids.iter().next().expect("one id")
    } else {
        next_id
    };
    let changes = files
        .iter()
        .enumerate()
        .map(|(k, f)| (f.clone(), mine(id, k)))
        .collect();
    tidy(current, changes)
}

/// Make `file` the top of its hand stack, the rest in their order — no change when it is in none.
pub fn make_top(file: &Path, current: &HandStacks) -> Vec<Change> {
    let Some(m) = current.get(file).copied() else {
        return Vec::new();
    };
    let mut order: Vec<PathBuf> = in_order(current, |o| o.stack == m.stack)
        .into_iter()
        .map(|(p, _)| p.clone())
        .collect();
    order.retain(|p| p != file);
    order.insert(0, file.to_path_buf());
    set_order(&order, current, m.stack)
}

/// *Stack by capture time* over `files`: take apart the time stacks they are in, then — with a gap —
/// stack runs of photos taken close together (Lightroom's Auto-Stack): per folder, sorted by capture
/// time, a run continues while the gap to the previous photo is **at most** `gap` seconds; a run of
/// two or more becomes a time stack, earliest on top. `None` is the menu's *None*: only the taking
/// apart.
///
/// ⚠⚠ A file in a stack of the **user's** is left out entirely — this menu never touches those. A file
/// with no capture time stacks nowhere.
pub fn by_capture_time(
    files: &[(PathBuf, Option<i64>)],
    gap: Option<i64>,
    current: &HandStacks,
    next_id: i64,
) -> Vec<Change> {
    let theirs = |p: &PathBuf| current.get(p).is_some_and(|m| !m.by_time);
    let scope: Vec<&(PathBuf, Option<i64>)> = files.iter().filter(|(p, _)| !theirs(p)).collect();
    let time_ids: HashSet<i64> = scope
        .iter()
        .filter_map(|(p, _)| current.get(p).filter(|m| m.by_time).map(|m| m.stack))
        .collect();
    let mut changes: Vec<Change> = in_order(current, |m| time_ids.contains(&m.stack))
        .into_iter()
        .map(|(p, _)| (p.clone(), None))
        .collect();
    if let Some(gap) = gap {
        let mut id = next_id;
        let paths: Vec<PathBuf> = scope.iter().map(|(p, _)| p.clone()).collect();
        let taken: HashMap<&PathBuf, i64> = scope
            .iter()
            .filter_map(|(p, t)| t.map(|t| (p, t)))
            .collect();
        for (_, group) in by_folder(&paths) {
            let mut timed: Vec<(i64, PathBuf)> = group
                .into_iter()
                .filter_map(|p| taken.get(&p).map(|t| (*t, p)))
                .collect();
            timed.sort();
            let mut runs: Vec<Vec<PathBuf>> = Vec::new();
            let mut last: Option<i64> = None;
            for (t, p) in timed {
                match (last, runs.last_mut()) {
                    (Some(prev), Some(run)) if t - prev <= gap => run.push(p),
                    _ => runs.push(vec![p]),
                }
                last = Some(t);
            }
            for run in runs.into_iter().filter(|r| r.len() > 1) {
                for (k, p) in run.iter().enumerate() {
                    changes.push((
                        p.clone(),
                        Some(HandMember {
                            stack: id,
                            ord: k as i64,
                            by_time: true,
                        }),
                    ));
                }
                id += 1;
            }
        }
    }
    tidy(current, changes)
}

/// The changes, with every stack they leave under two members dissolved, and nothing that would not
/// change a file.
pub fn tidy(current: &HandStacks, changes: Vec<Change>) -> Vec<Change> {
    let mut finals: HashMap<PathBuf, Membership> =
        current.iter().map(|(p, m)| (p.clone(), Some(*m))).collect();
    let mut order: Vec<PathBuf> = Vec::new();
    for (p, m) in changes {
        if !order.contains(&p) {
            order.push(p.clone());
        }
        finals.insert(p, m);
    }
    let mut count: HashMap<i64, usize> = HashMap::new();
    for m in finals.values().flatten() {
        *count.entry(m.stack).or_default() += 1;
    }
    let mut lonely: Vec<PathBuf> = finals
        .iter()
        .filter(|(_, m)| m.is_some_and(|m| count[&m.stack] < 2))
        .map(|(p, _)| p.clone())
        .collect();
    lonely.sort();
    for p in lonely {
        if !order.contains(&p) {
            order.push(p.clone());
        }
        finals.insert(p, None);
    }
    order
        .into_iter()
        .filter(|p| finals[p] != current.get(p).copied())
        .map(|p| {
            let m = finals[&p];
            (p, m)
        })
        .collect()
}

/// `files` by folder, in first-appearance order, each file once.
fn by_folder(files: &[PathBuf]) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut out: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for f in files {
        let dir = f.parent().map(Path::to_path_buf).unwrap_or_default();
        match out.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, g)) if !g.contains(f) => g.push(f.clone()),
            Some(_) => {}
            None => out.push((dir, vec![f.clone()])),
        }
    }
    out
}

/// The members of `current` that `keep` says, in stack then place order (path last), so every result
/// is deterministic.
fn in_order(
    current: &HandStacks,
    keep: impl Fn(&HandMember) -> bool,
) -> Vec<(&PathBuf, &HandMember)> {
    let mut v: Vec<_> = current.iter().filter(|(_, m)| keep(m)).collect();
    v.sort_by(|a, b| (a.1.stack, a.1.ord, a.0).cmp(&(b.1.stack, b.1.ord, b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// `(path, stack, ord, by_time)`.
    fn hand(of: &[(&str, i64, i64, bool)]) -> HandStacks {
        of.iter()
            .map(|(f, stack, ord, by_time)| {
                (
                    p(f),
                    HandMember {
                        stack: *stack,
                        ord: *ord,
                        by_time: *by_time,
                    },
                )
            })
            .collect()
    }

    fn as_map(changes: Vec<Change>) -> HashMap<PathBuf, Option<(i64, i64, bool)>> {
        changes
            .into_iter()
            .map(|(f, m)| (f, m.map(|m| (m.stack, m.ord, m.by_time))))
            .collect()
    }

    #[test]
    fn stacking_is_per_folder_with_the_top_first() {
        let files = [p("/a/1"), p("/a/2"), p("/b/3")];
        let got = as_map(stack(
            &files,
            Some(Path::new("/a/2")),
            &HandStacks::new(),
            5,
        ));
        assert_eq!(got.len(), 2, "a lone file in /b stacks nowhere: {got:?}");
        assert_eq!(got[&p("/a/2")], Some((5, 0, false)), "the top first");
        assert_eq!(got[&p("/a/1")], Some((5, 1, false)));
    }

    /// A file already in a hand stack brings its stack: the two merge into the new one.
    #[test]
    fn stacking_merges_a_stack_a_file_is_in() {
        let current = hand(&[("/a/3", 2, 0, false), ("/a/4", 2, 1, false)]);
        let got = as_map(stack(&[p("/a/1"), p("/a/3")], None, &current, 9));
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[&p("/a/1")], Some((9, 0, false)));
        assert_eq!(got[&p("/a/3")], Some((9, 1, false)));
        assert_eq!(
            got[&p("/a/4")],
            Some((9, 2, false)),
            "the merged member after"
        );
    }

    #[test]
    fn unstack_takes_the_whole_stack_apart_and_make_top_reorders() {
        let current = hand(&[
            ("/a/3", 2, 0, false),
            ("/a/4", 2, 1, false),
            ("/a/5", 6, 0, true),
            ("/a/6", 6, 1, true),
        ]);
        let got = as_map(unstack(&[p("/a/4")], &current));
        assert_eq!(got.len(), 2);
        assert_eq!(got[&p("/a/3")], None);
        let top = as_map(make_top(Path::new("/a/6"), &current));
        assert_eq!(
            top[&p("/a/6")],
            Some((6, 0, false)),
            "on top, and now the user's"
        );
        assert_eq!(top[&p("/a/5")], Some((6, 1, false)));
        assert!(make_top(Path::new("/a/9"), &current).is_empty());
    }

    /// Reordering keeps a stack's id; reordering files that share none — a name pair — makes a new
    /// hand stack of the user's.
    #[test]
    fn set_order_keeps_the_id_or_adopts() {
        let current = hand(&[("/a/1", 4, 0, true), ("/a/2", 4, 1, true)]);
        let got = as_map(set_order(&[p("/a/2"), p("/a/1")], &current, 9));
        assert_eq!(got[&p("/a/2")], Some((4, 0, false)));
        assert_eq!(got[&p("/a/1")], Some((4, 1, false)));
        let pair = as_map(set_order(&[p("/a/x.JPG"), p("/a/x.RAF")], &current, 9));
        assert_eq!(pair[&p("/a/x.JPG")], Some((9, 0, false)), "adopted");
        assert_eq!(pair[&p("/a/x.RAF")], Some((9, 1, false)));
    }

    /// Runs at the gap stay together, one over it splits; a file with no time stacks nowhere.
    #[test]
    fn capture_runs_become_time_stacks() {
        let files = [
            (p("/a/1"), Some(100)),
            (p("/a/2"), Some(102)),
            (p("/a/3"), Some(104)),
            (p("/a/4"), Some(107)),
            (p("/a/5"), Some(108)),
            (p("/a/6"), Some(200)),
            (p("/a/7"), None),
        ];
        let got = as_map(by_capture_time(&files, Some(2), &HandStacks::new(), 1));
        assert_eq!(got[&p("/a/1")], Some((1, 0, true)), "the earliest on top");
        assert_eq!(
            got[&p("/a/3")],
            Some((1, 2, true)),
            "exactly the gap stays in the run"
        );
        assert_eq!(
            got[&p("/a/4")],
            Some((2, 0, true)),
            "one over it starts another"
        );
        assert!(!got.contains_key(&p("/a/6")), "alone");
        assert!(!got.contains_key(&p("/a/7")), "no time");
    }

    /// ⚠⚠ After use: *None* takes the time stacks apart and leaves the user's; another gap replaces
    /// the time stacks; a file in a stack of the user's is never time-stacked.
    #[test]
    fn none_and_another_gap_touch_only_time_stacks() {
        let current = hand(&[
            ("/a/1", 1, 0, true),
            ("/a/2", 1, 1, true),
            ("/a/3", 7, 0, false),
            ("/a/4", 7, 1, false),
        ]);
        let files: Vec<(PathBuf, Option<i64>)> = (1..=5)
            .map(|k| (p(&format!("/a/{k}")), Some(100 + k as i64)))
            .collect();
        let none = as_map(by_capture_time(&files, None, &current, 20));
        assert_eq!(none.len(), 2, "{none:?}");
        assert_eq!(none[&p("/a/1")], None);
        assert_eq!(none[&p("/a/2")], None);
        let wide = as_map(by_capture_time(&files, Some(10), &current, 20));
        assert_eq!(
            wide[&p("/a/1")],
            Some((20, 0, true)),
            "replaced by the new run"
        );
        assert_eq!(
            wide[&p("/a/5")],
            Some((20, 2, true)),
            "3 and 4 skipped: theirs"
        );
        assert!(!wide.contains_key(&p("/a/3")) && !wide.contains_key(&p("/a/4")));
    }

    /// ⚠ No stack of one: re-stacking a member elsewhere dissolves what it leaves behind.
    #[test]
    fn a_stack_left_with_one_member_dissolves() {
        let current = hand(&[("/a/3", 2, 0, false), ("/a/4", 2, 1, false)]);
        let got = as_map(stack(&[p("/a/3"), p("/a/8")], None, &HandStacks::new(), 7));
        assert_eq!(got.len(), 2);
        let moved = as_map(set_order(&[p("/a/3"), p("/a/8")], &current, 7));
        assert_eq!(moved[&p("/a/4")], None, "left alone in stack 2, so out");
    }
}
