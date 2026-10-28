//! The Map view's camera (M10.18b): where it looks and how close, panned and zoomed in view pixels.
//! No egui — the bin hands in plain sizes and offsets.

use crate::geo::{from_world, to_world};

/// The closest and farthest zooms the map allows (OSM serves 0–19).
pub const ZOOM_RANGE: (f64, f64) = (1.0, 19.0);

/// What the map shows: its centre and a fractional zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapCamera {
    /// `(lat, lon)` at the view's centre.
    pub center: (f64, f64),
    pub zoom: f64,
}

impl Default for MapCamera {
    /// The whole world.
    fn default() -> Self {
        Self {
            center: (20.0, 0.0),
            zoom: 2.0,
        }
    }
}

impl MapCamera {
    /// `(lat, lon)` → view pixels, the view `size` with its origin top-left.
    pub fn to_view(&self, at: (f64, f64), size: (f32, f32)) -> (f32, f32) {
        let (cx, cy) = to_world(self.center.0, self.center.1, self.zoom);
        let (x, y) = to_world(at.0, at.1, self.zoom);
        (
            (x - cx + f64::from(size.0) / 2.0) as f32,
            (y - cy + f64::from(size.1) / 2.0) as f32,
        )
    }

    /// View pixels → `(lat, lon)`.
    pub fn from_view(&self, p: (f32, f32), size: (f32, f32)) -> (f64, f64) {
        let (cx, cy) = to_world(self.center.0, self.center.1, self.zoom);
        from_world(
            cx + f64::from(p.0) - f64::from(size.0) / 2.0,
            cy + f64::from(p.1) - f64::from(size.1) / 2.0,
            self.zoom,
        )
    }

    /// Move the map with a drag of `delta` view pixels.
    pub fn pan(&mut self, delta: (f32, f32), size: (f32, f32)) {
        let mid = (size.0 / 2.0 - delta.0, size.1 / 2.0 - delta.1);
        self.center = self.from_view(mid, size);
    }

    /// Zoom by `steps` (one step doubles) keeping the place under `pointer` where it is.
    pub fn zoom_about(&mut self, steps: f64, pointer: (f32, f32), size: (f32, f32)) {
        let under = self.from_view(pointer, size);
        self.zoom = (self.zoom + steps).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let now = self.to_view(under, size);
        self.pan((pointer.0 - now.0, pointer.1 - now.1), size);
    }

    /// The camera that shows every point in `points` inside `size`, with a margin; one point is shown
    /// close (zoom 14), none the whole world.
    pub fn fit(points: &[(f64, f64)], size: (f32, f32)) -> Self {
        let Some(&first) = points.first() else {
            return Self::default();
        };
        if points.len() == 1 {
            return Self {
                center: first,
                zoom: 14.0,
            };
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(la, lo) in points {
            let (x, y) = to_world(la, lo, 0.0);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let margin = 0.8;
        let span = |a: f64, b: f64, px: f32| (f64::from(px) * margin / (b - a).max(1e-9)).log2();
        let zoom = span(x0, x1, size.0)
            .min(span(y0, y1, size.1))
            .clamp(ZOOM_RANGE.0, 14.0);
        Self {
            center: from_world((x0 + x1) / 2.0, (y0 + y1) / 2.0, 0.0),
            zoom,
        }
    }
}

/// When the Map fits its camera to the photos (asked for in use, 2026-10-28: "when moving to a different
/// folder … the map also moved"): on a new SOURCE, and when that source's first located photos arrive
/// (the startup pass may land them after the view opened). ⚠ Never on a filter or a rating — the map
/// would jump while you cull.
#[derive(Debug)]
pub struct Refit<K> {
    source: Option<K>,
    had_points: bool,
}

/// ⚠ By hand: a derive would ask `K: Default`, which a source has no need of.
impl<K> Default for Refit<K> {
    fn default() -> Self {
        Self {
            source: None,
            had_points: false,
        }
    }
}

impl<K: PartialEq + Clone> Refit<K> {
    /// Whether to fit now, for `source` with or without located photos; remembers the answer.
    pub fn check(&mut self, source: &K, has_points: bool) -> bool {
        let new_source = self.source.as_ref() != Some(source);
        let first_points = has_points && !self.had_points;
        if new_source {
            self.source = Some(source.clone());
        }
        self.had_points = has_points;
        new_source || first_points
    }

    /// Forget the source, so the next check fits — "Show all photos".
    pub fn reset(&mut self) {
        self.source = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (800.0, 600.0);

    #[test]
    fn the_centre_is_the_middle_of_the_view() {
        let c = MapCamera {
            center: (38.7, -9.1),
            zoom: 10.0,
        };
        let (x, y) = c.to_view((38.7, -9.1), SIZE);
        assert!((x - 400.0).abs() < 1e-3 && (y - 300.0).abs() < 1e-3);
        let (la, lo) = c.from_view((400.0, 300.0), SIZE);
        assert!((la - 38.7).abs() < 1e-9 && (lo + 9.1).abs() < 1e-9);
    }

    #[test]
    fn map_camera_zooms_about_the_pointer() {
        let mut c = MapCamera {
            center: (38.7, -9.1),
            zoom: 8.0,
        };
        let p = (650.0, 120.0);
        let under = c.from_view(p, SIZE);
        c.zoom_about(1.5, p, SIZE);
        assert!((c.zoom - 9.5).abs() < 1e-12);
        let (x, y) = c.to_view(under, SIZE);
        assert!((x - p.0).abs() < 0.01 && (y - p.1).abs() < 0.01, "{x},{y}");
        c.zoom_about(100.0, p, SIZE);
        assert_eq!(c.zoom, ZOOM_RANGE.1);
    }

    #[test]
    fn a_drag_moves_the_map_with_the_pointer() {
        let mut c = MapCamera {
            center: (38.7, -9.1),
            zoom: 12.0,
        };
        let at = c.to_view((38.71, -9.12), SIZE);
        c.pan((30.0, -20.0), SIZE);
        let now = c.to_view((38.71, -9.12), SIZE);
        assert!((now.0 - at.0 - 30.0).abs() < 0.01 && (now.1 - at.1 + 20.0).abs() < 0.01);
    }

    #[test]
    fn the_map_refits_on_a_new_source_and_on_its_first_pins_only() {
        let mut r = Refit::default();
        assert!(r.check(&"trip", true), "the first view");
        assert!(!r.check(&"trip", true), "the same view");
        assert!(r.check(&"home", false), "a new folder");
        assert!(r.check(&"home", true), "its first pins landed");
        assert!(!r.check(&"home", true));
        r.reset();
        assert!(r.check(&"home", true), "Show all photos");
    }

    #[test]
    fn fit_to_points_contains_them_all() {
        let pts = [
            (38.71, -9.14),
            (41.15, -8.61),
            (37.02, -7.93),
            (38.80, -9.38),
        ];
        let c = MapCamera::fit(&pts, SIZE);
        for p in pts {
            let (x, y) = c.to_view(p, SIZE);
            assert!(
                (0.0..=SIZE.0).contains(&x) && (0.0..=SIZE.1).contains(&y),
                "{p:?} at {x},{y}"
            );
        }
        assert!(c.zoom > 4.0, "fitted too far out: {}", c.zoom);
        assert_eq!(MapCamera::fit(&[(38.7, -9.1)], SIZE).zoom, 14.0);
        assert_eq!(MapCamera::fit(&[], SIZE), MapCamera::default());
    }
}
