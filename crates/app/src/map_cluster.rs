//! The Map view's clusters (M10.18b): photos close together on screen become one pin with a count. No egui.
//!
//! ⚠⚠ Memoised — the map draws every frame, and nothing drawn per frame may grow with the catalog
//! (CLAUDE.md, the rail warning). [`ClusterMemo`] recomputes only when the view's points or the zoom change.

/// One pin: its world position at the clustering zoom (the members' mean) and which points it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub at: (f64, f64),
    pub members: Vec<usize>,
}

/// Bucket `world` points (pixels at zoom `zoom`) into `cell_px` squares; one cluster per non-empty cell,
/// in first-seen order.
pub fn cluster(world: &[(f64, f64)], cell_px: f64) -> Vec<Cluster> {
    let mut cells: std::collections::HashMap<(i64, i64), usize> = std::collections::HashMap::new();
    let mut out: Vec<Cluster> = Vec::new();
    let mut sums: Vec<(f64, f64)> = Vec::new();
    for (i, &(x, y)) in world.iter().enumerate() {
        let key = ((x / cell_px).floor() as i64, (y / cell_px).floor() as i64);
        let k = *cells.entry(key).or_insert_with(|| {
            out.push(Cluster {
                at: (0.0, 0.0),
                members: Vec::new(),
            });
            sums.push((0.0, 0.0));
            out.len() - 1
        });
        out[k].members.push(i);
        sums[k].0 += x;
        sums[k].1 += y;
    }
    for (c, s) in out.iter_mut().zip(sums) {
        let n = c.members.len() as f64;
        c.at = (s.0 / n, s.1 / n);
    }
    out
}

/// [`cluster`], remembered for one `(view, zoom)`.
#[derive(Default)]
pub struct ClusterMemo {
    key: Option<(u64, u8)>,
    clusters: std::sync::Arc<Vec<Cluster>>,
    /// How many times it actually clustered — for tests.
    pub computed: usize,
}

impl ClusterMemo {
    /// The clusters for `view_key`'s points at integer `zoom`; `points` (lat, lon) is called only on a miss.
    pub fn get(
        &mut self,
        view_key: u64,
        zoom: u8,
        cell_px: f64,
        points: impl FnOnce() -> Vec<(f64, f64)>,
    ) -> std::sync::Arc<Vec<Cluster>> {
        if self.key != Some((view_key, zoom)) {
            let world: Vec<(f64, f64)> = points()
                .into_iter()
                .map(|(la, lo)| crate::geo::to_world(la, lo, f64::from(zoom)))
                .collect();
            self.clusters = std::sync::Arc::new(cluster(&world, cell_px));
            self.key = Some((view_key, zoom));
            self.computed += 1;
        }
        self.clusters.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_counts_sum_to_the_input() {
        let pts: Vec<(f64, f64)> = (0..1000)
            .map(|i| ((i % 37) as f64 * 13.0, (i / 37) as f64 * 7.0))
            .collect();
        let c = cluster(&pts, 60.0);
        assert_eq!(c.iter().map(|c| c.members.len()).sum::<usize>(), pts.len());
        assert!(c.len() > 1 && c.len() < pts.len());
        let two = cluster(&[(1.0, 1.0), (3.0, 5.0)], 60.0);
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].at, (2.0, 3.0), "the members' mean");
    }

    #[test]
    fn the_memo_does_not_recompute_an_unchanged_view() {
        let mut m = ClusterMemo::default();
        let pts = || vec![(38.7, -9.1), (38.8, -9.4)];
        m.get(1, 5, 60.0, pts);
        m.get(1, 5, 60.0, || panic!("an unchanged view clustered again"));
        assert_eq!(m.computed, 1);
        m.get(1, 6, 60.0, pts);
        m.get(2, 6, 60.0, pts);
        assert_eq!(m.computed, 3);
    }

    /// The anti-goal: a 100k-photo view clusters well inside a frame's budget. Release only.
    #[test]
    #[ignore]
    fn cluster_100k_points_is_fast() {
        let pts: Vec<(f64, f64)> = (0..100_000)
            .map(|i| ((i * 7919 % 100_000) as f64, (i * 104_729 % 100_000) as f64))
            .collect();
        let t = std::time::Instant::now();
        let c = cluster(&pts, 60.0);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        println!("100k points → {} clusters in {ms:.1} ms", c.len());
        assert!(ms < 50.0, "{ms:.1} ms");
    }
}
