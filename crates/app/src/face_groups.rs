//! The People view's "Who is this?" (M10.18c): unnamed faces grouped by likeness, and a group offered as someone
//! already named when it looks like them. No egui.
//!
//! ⚠ Grouping only ever SUGGESTS. A named face is never in the input (the caller passes the unnamed ones), so no
//! grouping can move a name; and a person any face of a group was answered "No" for is never offered for it.
//!
//! The method (the builder's call, recorded in the journal): each face joins the group whose running mean it is
//! most like when that likeness clears [`THRESHOLD`], else starts a group — linear in the faces, deterministic in
//! their order. SFace's own published threshold for "the same person" is the cosine 0.363.

use std::collections::{HashMap, HashSet};

/// SFace's cosine for "the same person" (OpenCV's `FaceRecognizerSF`).
pub const THRESHOLD: f32 = 0.363;

/// One group of look-alike unnamed faces.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub faces: Vec<i64>,
    /// The person it looks like, when one clears the threshold and was not rejected for it.
    pub suggest: Option<i64>,
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn unit(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= n);
}

/// Group `unnamed` (`(face, embedding)`); `named` (`(person, embedding)`) are the confirmed faces suggestions come
/// from; `rejected` holds every `(face, person)` "No". Largest groups first.
pub fn group(
    unnamed: &[(i64, Vec<f32>)],
    named: &[(i64, Vec<f32>)],
    rejected: &HashSet<(i64, i64)>,
    threshold: f32,
) -> Vec<Group> {
    // The groups: their faces and the running sum of their embeddings (normalised for each comparison).
    let mut groups: Vec<(Vec<i64>, Vec<f32>)> = Vec::new();
    for (id, e) in unnamed {
        let best = groups
            .iter()
            .enumerate()
            .map(|(i, (_, sum))| {
                let mut m = sum.clone();
                unit(&mut m);
                (i, dot(&m, e))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, s)) if s >= threshold => {
                groups[i].0.push(*id);
                groups[i].1.iter_mut().zip(e).for_each(|(a, b)| *a += b);
            }
            _ => groups.push((vec![*id], e.clone())),
        }
    }
    // Each person's mean confirmed face.
    let mut people: HashMap<i64, Vec<f32>> = HashMap::new();
    for (p, e) in named {
        let m = people.entry(*p).or_insert_with(|| vec![0.0; e.len()]);
        m.iter_mut().zip(e).for_each(|(a, b)| *a += b);
    }
    people.values_mut().for_each(|m| unit(m));
    let mut out: Vec<Group> = groups
        .into_iter()
        .map(|(faces, mut sum)| {
            unit(&mut sum);
            let suggest = people
                .iter()
                .filter(|(p, _)| !faces.iter().any(|f| rejected.contains(&(*f, **p))))
                .map(|(p, m)| (*p, dot(&sum, m)))
                .filter(|(_, s)| *s >= threshold)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(p, _)| p);
            Group { faces, suggest }
        })
        .collect();
    out.sort_by(|a, b| {
        b.faces
            .len()
            .cmp(&a.faces.len())
            .then(a.faces[0].cmp(&b.faces[0]))
    });
    out
}

/// [`group`], remembered for one key — the view's `(instance, change count, view)` folded together.
#[derive(Default)]
pub struct GroupMemo {
    key: Option<u64>,
    groups: std::sync::Arc<Vec<Group>>,
    /// How many times it actually grouped — for tests.
    pub computed: usize,
}

impl GroupMemo {
    pub fn get(
        &mut self,
        key: u64,
        compute: impl FnOnce() -> Vec<Group>,
    ) -> std::sync::Arc<Vec<Group>> {
        if self.key != Some(key) {
            self.groups = std::sync::Arc::new(compute());
            self.key = Some(key);
            self.computed += 1;
        }
        self.groups.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit vector near axis `k`, nudged by `j` so the faces of one person are alike but not equal.
    fn near(k: usize, j: f32) -> Vec<f32> {
        let mut v = vec![0.0; 128];
        v[k] = 1.0;
        v[(k + 1) % 128] = j;
        unit(&mut v);
        v
    }

    #[test]
    fn two_tight_groups_and_an_outlier() {
        let faces = vec![
            (1, near(0, 0.1)),
            (2, near(10, 0.1)),
            (3, near(0, 0.2)),
            (4, near(10, 0.0)),
            (5, near(0, 0.05)),
            (6, near(50, 0.0)),
        ];
        let g = group(&faces, &[], &HashSet::new(), THRESHOLD);
        assert_eq!(g.len(), 3, "{g:?}");
        assert_eq!(g[0].faces, vec![1, 3, 5], "the largest first");
        assert_eq!(g[1].faces, vec![2, 4]);
        assert_eq!(g[2].faces, vec![6]);
        assert!(g.iter().all(|g| g.suggest.is_none()));
    }

    #[test]
    fn a_group_like_anna_is_offered_as_anna() {
        let anna = 7;
        let g = group(
            &[(1, near(0, 0.1)), (2, near(0, 0.2))],
            &[(anna, near(0, 0.15))],
            &HashSet::new(),
            THRESHOLD,
        );
        assert_eq!(g[0].suggest, Some(anna));
        let g = group(
            &[(1, near(20, 0.0))],
            &[(anna, near(0, 0.15))],
            &HashSet::new(),
            THRESHOLD,
        );
        assert_eq!(g[0].suggest, None, "a stranger is offered as nobody");
    }

    /// Review focus 2: "No" is final — even after more faces join the group.
    #[test]
    fn a_rejected_person_is_not_offered_again() {
        let anna = 7;
        let rejected = HashSet::from([(1, anna)]);
        let more = [(1, near(0, 0.1)), (2, near(0, 0.2)), (3, near(0, 0.0))];
        let g = group(&more, &[(anna, near(0, 0.15))], &rejected, THRESHOLD);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].suggest, None);
    }

    /// Review focus 1: a named face is not an input, so grouping cannot carry it anywhere — the caller's
    /// contract, pinned: every face out is a face in.
    #[test]
    fn grouping_never_moves_a_named_face() {
        let unnamed = [(1, near(0, 0.1)), (2, near(0, 0.2))];
        let named = [(7, near(0, 0.15))];
        let g = group(&unnamed, &named, &HashSet::new(), THRESHOLD);
        let out: Vec<i64> = g.iter().flat_map(|g| g.faces.clone()).collect();
        assert_eq!(out, vec![1, 2], "a named face appeared in a group");
    }

    #[test]
    fn the_memo_keeps_an_unchanged_answer() {
        let mut m = GroupMemo::default();
        m.get(1, Vec::new);
        m.get(1, || panic!("an unchanged view grouped again"));
        m.get(2, Vec::new);
        assert_eq!(m.computed, 2);
    }
}
