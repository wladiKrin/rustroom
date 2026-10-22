//! The outlines of a plane (M10.16c3: the border each person found is drawn with — the author wanted it to follow
//! the tinted area, not a box): marching squares at a level, the segments chained into closed loops.

use std::collections::HashMap;

/// The outlines of `alpha` (row-major `w × h`, 0..1) at `level`: closed loops in the plane's own units — pixel `i`'s
/// centre at `i + 0.5` — with outside the plane taken as empty, so every loop closes. A hole is a loop of its own.
pub fn loops(alpha: &[f32], w: usize, h: usize, level: f32) -> Vec<Vec<[f32; 2]>> {
    if w == 0 || h == 0 || alpha.len() < w * h {
        return Vec::new();
    }
    // The plane padded by one empty pixel all round: grid points (x, y) for x in 0..=w + 1, y in 0..=h + 1.
    let (gw, gh) = (w + 2, h + 2);
    let v = |x: usize, y: usize| -> f32 {
        if x == 0 || y == 0 || x > w || y > h {
            0.0
        } else {
            alpha[(y - 1) * w + (x - 1)]
        }
    };
    // An edge's key: horizontal from grid point (x, y) to (x + 1, y), or vertical to (x, y + 1).
    let key = |x: usize, y: usize, vertical: bool| ((y * gw + x) * 2 + vertical as usize) as u64;
    // Where the level crosses an edge, in the plane's units (grid point (x, y) is pixel centre (x − 0.5, y − 0.5)).
    let cross = |x: usize, y: usize, vertical: bool| -> [f32; 2] {
        let (a, b) = if vertical {
            (v(x, y), v(x, y + 1))
        } else {
            (v(x, y), v(x + 1, y))
        };
        let t = if (b - a).abs() > 1e-9 {
            ((level - a) / (b - a)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let (px, py) = (x as f32 - 0.5, y as f32 - 0.5);
        if vertical {
            [px, py + t]
        } else {
            [px + t, py]
        }
    };
    let mut segs: Vec<(u64, u64)> = Vec::new();
    let mut at: HashMap<u64, [f32; 2]> = HashMap::new();
    for y in 0..gh - 1 {
        for x in 0..gw - 1 {
            let on = |vx, vy| v(vx, vy) >= level;
            let (tl, tr, br, bl) = (on(x, y), on(x + 1, y), on(x + 1, y + 1), on(x, y + 1));
            let case = (tl as u8) << 3 | (tr as u8) << 2 | (br as u8) << 1 | bl as u8;
            // The cell's edges: top, right, bottom, left.
            let edges = [
                (x, y, false),
                (x + 1, y, true),
                (x, y + 1, false),
                (x, y, true),
            ];
            let centre = (v(x, y) + v(x + 1, y) + v(x + 1, y + 1) + v(x, y + 1)) / 4.0 >= level;
            let (top, right, bottom, left) = (0, 1, 2, 3);
            let pairs: &[(usize, usize)] = match case {
                1 | 14 => &[(left, bottom)],
                2 | 13 => &[(bottom, right)],
                3 | 12 => &[(left, right)],
                4 | 11 => &[(top, right)],
                6 | 9 => &[(top, bottom)],
                7 | 8 => &[(left, top)],
                // The saddles: the centre decides which corners join.
                5 if centre => &[(left, top), (right, bottom)],
                5 => &[(top, right), (left, bottom)],
                10 if centre => &[(top, right), (left, bottom)],
                10 => &[(left, top), (right, bottom)],
                _ => &[],
            };
            for &(a, b) in pairs {
                let (ka, kb) = (
                    key(edges[a].0, edges[a].1, edges[a].2),
                    key(edges[b].0, edges[b].1, edges[b].2),
                );
                at.entry(ka)
                    .or_insert_with(|| cross(edges[a].0, edges[a].1, edges[a].2));
                at.entry(kb)
                    .or_insert_with(|| cross(edges[b].0, edges[b].1, edges[b].2));
                segs.push((ka, kb));
            }
        }
    }
    // Chain: every edge crossed belongs to exactly two segments, so each walk returns to where it began.
    let mut by_edge: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, &(a, b)) in segs.iter().enumerate() {
        by_edge.entry(a).or_default().push(i);
        by_edge.entry(b).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (first, mut cur) = segs[start];
        let mut pts = vec![at[&first]];
        while cur != first {
            pts.push(at[&cur]);
            let next = by_edge[&cur].iter().copied().find(|&j| !used[j]);
            let Some(j) = next else {
                break;
            };
            used[j] = true;
            cur = if segs[j].0 == cur {
                segs[j].1
            } else {
                segs[j].0
            };
        }
        if pts.len() >= 3 {
            out.push(pts);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: usize, h: usize, on: impl Fn(usize, usize) -> bool) -> Vec<f32> {
        (0..w * h).map(|i| on(i % w, i / w) as u8 as f32).collect()
    }

    fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
        let mut c = false;
        for (i, a) in poly.iter().enumerate() {
            let b = poly[(i + 1) % poly.len()];
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
            {
                c = !c;
            }
        }
        c
    }

    /// A filled square has one outline, around it — the blob's pixels inside, the empty ones outside.
    #[test]
    fn a_square_has_one_loop_around_it() {
        let a = plane(10, 8, |x, y| (3..7).contains(&x) && (2..6).contains(&y));
        let l = loops(&a, 10, 8, 0.5);
        assert_eq!(l.len(), 1, "{l:?}");
        assert!(inside(&l[0], [5.0, 4.0]), "the middle is inside");
        assert!(!inside(&l[0], [1.0, 1.0]), "a corner is outside");
        for p in &l[0] {
            assert!(
                (2.9..=7.1).contains(&p[0]) && (1.9..=6.1).contains(&p[1]),
                "{p:?}"
            );
        }
    }

    /// A blob touching the plane's edge closes along it — outside the plane is empty.
    #[test]
    fn a_blob_on_the_edge_closes() {
        let a = plane(6, 6, |x, _| x < 3);
        let l = loops(&a, 6, 6, 0.5);
        assert_eq!(l.len(), 1);
        assert!(inside(&l[0], [1.0, 3.0]));
    }

    #[test]
    fn two_blobs_two_loops_and_none_for_nothing() {
        let a = plane(12, 6, |x, y| {
            (1..4).contains(&x) && (1..4).contains(&y) || (7..11).contains(&x) && y >= 2
        });
        assert_eq!(loops(&a, 12, 6, 0.5).len(), 2);
        assert!(loops(&[0.0; 36], 6, 6, 0.5).is_empty());
    }

    /// A hole is a loop of its own.
    #[test]
    fn a_ring_has_an_inner_loop() {
        let a = plane(9, 9, |x, y| {
            (1..8).contains(&x)
                && (1..8).contains(&y)
                && !((3..6).contains(&x) && (3..6).contains(&y))
        });
        assert_eq!(loops(&a, 9, 9, 0.5).len(), 2);
    }
}
