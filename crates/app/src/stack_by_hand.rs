//! Stacks made by hand (M10.8c): what Stack, Unstack, Make top of stack and Stack by capture time
//! change.
//!
//! ⚠ Pure and egui-free: each takes the current memberships of the files on view and returns the
//! **changes** — a file's new membership, `None` for out — which the app writes to the catalog and
//! records as one undo step. Every result goes through [`tidy`], so no command leaves a stack of one.

use crate::stack::HandStacks;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A file's membership: its stack id and whether it is the top, or `None`.
pub type Membership = Option<(i64, bool)>;

/// A file's new membership.
pub type Change = (PathBuf, Membership);

/// Stack `files`, **per folder** (two or more each), with `top` on top where it is one of them — else
/// the first. A file already in a hand stack brings its whole stack along: the stacks merge.
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
            .filter_map(|f| current.get(f).map(|(i, _)| *i))
            .collect();
        let mut members = group.clone();
        for (p, (i, _)) in sorted(current) {
            if joined.contains(i) && p.parent() == Some(dir.as_path()) && !members.contains(p) {
                members.push(p.clone());
            }
        }
        if members.len() < 2 {
            continue;
        }
        let top = top
            .filter(|t| members.iter().any(|m| m == t))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| members[0].clone());
        for m in &members {
            changes.push((m.clone(), Some((id, *m == top))));
        }
        id += 1;
    }
    tidy(current, changes)
}

/// Take apart every hand stack any of `files` belongs to.
pub fn unstack(files: &[PathBuf], current: &HandStacks) -> Vec<Change> {
    let ids: HashSet<i64> = files
        .iter()
        .filter_map(|f| current.get(f).map(|(i, _)| *i))
        .collect();
    let changes = sorted(current)
        .into_iter()
        .filter(|(_, (i, _))| ids.contains(i))
        .map(|(p, _)| (p.clone(), None))
        .collect();
    tidy(current, changes)
}

/// Make `file` the top of its hand stack — no change when it is in none.
pub fn make_top(file: &Path, current: &HandStacks) -> Vec<Change> {
    let Some((id, _)) = current.get(file).copied() else {
        return Vec::new();
    };
    let changes = sorted(current)
        .into_iter()
        .filter(|(_, (i, _))| *i == id)
        .map(|(p, _)| (p.clone(), Some((id, p == file))))
        .collect();
    tidy(current, changes)
}

/// Stack runs of photos taken close together (Lightroom's Auto-Stack): per folder, sorted by capture
/// time, a run continues while the gap to the previous photo is **at most** `gap` seconds; a run of two
/// or more becomes a hand stack with its earliest on top. A file with no capture time stacks nowhere,
/// and a file it re-stacks leaves its old stack.
pub fn by_capture_time(
    files: &[(PathBuf, Option<i64>)],
    gap: i64,
    current: &HandStacks,
    next_id: i64,
) -> Vec<Change> {
    let mut id = next_id;
    let mut changes = Vec::new();
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
    let taken: HashMap<&PathBuf, i64> = files
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
                changes.push((p.clone(), Some((id, k == 0))));
            }
            id += 1;
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
    for (id, _) in finals.values().flatten() {
        *count.entry(*id).or_default() += 1;
    }
    let mut lonely: Vec<PathBuf> = finals
        .iter()
        .filter(|(_, m)| m.is_some_and(|(id, _)| count[&id] < 2))
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

/// `current` in path order, so every result is deterministic.
fn sorted(current: &HandStacks) -> Vec<(&PathBuf, &(i64, bool))> {
    let mut v: Vec<_> = current.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn hand(of: &[(&str, i64, bool)]) -> HandStacks {
        of.iter().map(|(f, i, t)| (p(f), (*i, *t))).collect()
    }

    fn as_map(changes: Vec<Change>) -> HashMap<PathBuf, Membership> {
        changes.into_iter().collect()
    }

    #[test]
    fn stacking_is_per_folder_with_the_top_given() {
        let files = [p("/a/1"), p("/a/2"), p("/b/3")];
        let got = as_map(stack(
            &files,
            Some(Path::new("/a/2")),
            &HandStacks::new(),
            5,
        ));
        assert_eq!(got.len(), 2, "a lone file in /b stacks nowhere: {got:?}");
        assert_eq!(got[&p("/a/1")], Some((5, false)));
        assert_eq!(got[&p("/a/2")], Some((5, true)));
    }

    /// A file already in a hand stack brings its stack: the two merge into the new one.
    #[test]
    fn stacking_merges_a_stack_a_file_is_in() {
        let current = hand(&[("/a/3", 2, true), ("/a/4", 2, false)]);
        let got = as_map(stack(&[p("/a/1"), p("/a/3")], None, &current, 9));
        assert_eq!(got.len(), 3, "{got:?}");
        assert!(got.values().all(|m| m.map(|(i, _)| i) == Some(9)));
        assert_eq!(got[&p("/a/1")], Some((9, true)), "the first is the top");
    }

    #[test]
    fn unstack_takes_the_whole_stack_apart_and_make_top_moves_the_top() {
        let current = hand(&[
            ("/a/3", 2, true),
            ("/a/4", 2, false),
            ("/a/5", 6, true),
            ("/a/6", 6, false),
        ]);
        let got = as_map(unstack(&[p("/a/4")], &current));
        assert_eq!(got.len(), 2);
        assert_eq!(got[&p("/a/3")], None);
        assert_eq!(got[&p("/a/4")], None);
        let top = as_map(make_top(Path::new("/a/6"), &current));
        assert_eq!(top[&p("/a/5")], Some((6, false)));
        assert_eq!(top[&p("/a/6")], Some((6, true)));
        assert!(make_top(Path::new("/a/9"), &current).is_empty());
    }

    /// Runs at the gap stay together, one over it splits; a file with no time stacks nowhere.
    #[test]
    fn capture_runs_become_stacks() {
        let files = [
            (p("/a/1"), Some(100)),
            (p("/a/2"), Some(102)),
            (p("/a/3"), Some(104)),
            (p("/a/4"), Some(107)),
            (p("/a/5"), Some(108)),
            (p("/a/6"), Some(200)),
            (p("/a/7"), None),
        ];
        let got = as_map(by_capture_time(&files, 2, &HandStacks::new(), 1));
        assert_eq!(got[&p("/a/1")], Some((1, true)), "the earliest on top");
        assert_eq!(got[&p("/a/2")], Some((1, false)));
        assert_eq!(
            got[&p("/a/3")],
            Some((1, false)),
            "exactly the gap stays in the run"
        );
        assert_eq!(
            got[&p("/a/4")],
            Some((2, true)),
            "one over it starts another"
        );
        assert_eq!(got[&p("/a/5")], Some((2, false)));
        assert!(!got.contains_key(&p("/a/6")), "alone");
        assert!(!got.contains_key(&p("/a/7")), "no time");
    }

    /// ⚠ No stack of one: re-stacking a member elsewhere dissolves what it leaves behind.
    #[test]
    fn a_stack_left_with_one_member_dissolves() {
        let current = hand(&[("/a/3", 2, true), ("/a/4", 2, false)]);
        let files = [(p("/a/3"), Some(10)), (p("/a/8"), Some(11))];
        let got = as_map(by_capture_time(&files, 2, &current, 7));
        assert_eq!(got[&p("/a/3")], Some((7, true)));
        assert_eq!(got[&p("/a/4")], None, "left alone in stack 2, so out");
    }
}
