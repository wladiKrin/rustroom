//! Web Mercator for the Map view (M10.18b): latitude/longitude ↔ world pixels, and which 256-px tiles
//! cover a view. No egui.
//!
//! ⚠ The world at zoom `z` is `256 · 2^z` pixels square; x grows east from the antimeridian, y grows south
//! from 85.05° N. Tiles wrap in x and stop at the poles in y.

use std::path::{Path, PathBuf};

/// A tile's edge in pixels.
pub const TILE: f64 = 256.0;
/// Web Mercator's latitude limit — the square world's edge.
pub const MAX_LAT: f64 = 85.051_128_78;

/// One tile: zoom, column, row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// The world's edge in pixels at `zoom`.
pub fn world_size(zoom: f64) -> f64 {
    TILE * zoom.exp2()
}

/// `(lat, lon)` → world pixels at `zoom`. ⚠ The latitude is clamped to the square world.
pub fn to_world(lat: f64, lon: f64, zoom: f64) -> (f64, f64) {
    let n = world_size(zoom);
    let lat = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

/// World pixels at `zoom` → `(lat, lon)`, the longitude wrapped into −180…180.
pub fn from_world(x: f64, y: f64, zoom: f64) -> (f64, f64) {
    let n = world_size(zoom);
    let lon = (x / n * 360.0 - 180.0).rem_euclid(360.0);
    let lon = if lon > 180.0 { lon - 360.0 } else { lon };
    let m = std::f64::consts::PI * (1.0 - 2.0 * y / n);
    (m.sinh().atan().to_degrees(), lon)
}

/// The tiles a view of `size` pixels centred on `center` (world pixels) needs at integer `zoom`, each
/// with its top-left in view pixels. x wraps across the antimeridian; rows outside the world are left out.
pub fn tiles_for(center: (f64, f64), zoom: u8, size: (f32, f32)) -> Vec<(TileId, (f32, f32))> {
    let count = 1i64 << zoom;
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let left = center.0 - w / 2.0;
    let top = center.1 - h / 2.0;
    let x0 = (left / TILE).floor() as i64;
    let x1 = ((left + w) / TILE).ceil() as i64;
    let y0 = ((top / TILE).floor() as i64).max(0);
    let y1 = (((top + h) / TILE).ceil() as i64).min(count);
    let mut out = Vec::new();
    for ty in y0..y1 {
        for tx in x0..x1 {
            let id = TileId {
                z: zoom,
                x: tx.rem_euclid(count) as u32,
                y: ty as u32,
            };
            let at = (
                (tx as f64 * TILE - left) as f32,
                (ty as f64 * TILE - top) as f32,
            );
            out.push((id, at));
        }
    }
    out
}

/// A tile's file in the disk cache.
pub fn cache_path(root: &Path, t: TileId) -> PathBuf {
    root.join(t.z.to_string())
        .join(t.x.to_string())
        .join(format!("{}.png", t.y))
}

/// The tile server's URL for `t`, from a `{z}/{x}/{y}` template.
pub fn tile_url(template: &str, t: TileId) -> String {
    template
        .replace("{z}", &t.z.to_string())
        .replace("{x}", &t.x.to_string())
        .replace("{y}", &t.y.to_string())
}

/// Where a tile server's tiles are cached: `<cache>/rustroom/tiles/<host>`.
pub fn tile_cache_root(template: &str) -> Option<PathBuf> {
    Some(
        dirs::cache_dir()?
            .join("rustroom")
            .join("tiles")
            .join(crate::immich_source::host_slug(template)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_projection_round_trips() {
        for (la, lo) in [
            (38.7139, -9.1394),
            (-33.86, 151.2),
            (0.0, 0.0),
            (70.0, 179.9),
        ] {
            for z in [0.0, 5.5, 18.0] {
                let (x, y) = to_world(la, lo, z);
                let (b, c) = from_world(x, y, z);
                assert!(
                    (b - la).abs() < 1e-9 && (c - lo).abs() < 1e-9,
                    "{la},{lo} z{z} → {b},{c}"
                );
            }
        }
        assert_eq!(to_world(0.0, -180.0, 0.0), (0.0, 128.0));
    }

    #[test]
    fn tiles_for_covers_the_view_at_zoom_0_5_18() {
        let world = |z: u8| world_size(f64::from(z));
        // Zoom 0: the one tile, whatever the view's size.
        let t = tiles_for((128.0, 128.0), 0, (256.0, 256.0));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0], (TileId { z: 0, x: 0, y: 0 }, (0.0, 0.0)));
        // Zoom 5, a 512×256 view centred on a tile corner: exactly 2 columns × 2 rows; nudged off the
        // corner it straddles a third column.
        let c = (10.0 * TILE, 10.0 * TILE);
        let t = tiles_for(c, 5, (512.0, 256.0));
        assert_eq!(t.len(), 2 * 2, "{t:?}");
        assert!(t
            .iter()
            .all(|(id, _)| (9..=10).contains(&id.x) && (9..=10).contains(&id.y)));
        assert_eq!(tiles_for((c.0 + 10.0, c.1), 5, (512.0, 256.0)).len(), 3 * 2);
        // Zoom 18 near Lisbon: every tile is inside the world.
        let (x, y) = to_world(38.7, -9.1, 18.0);
        let t = tiles_for((x, y), 18, (800.0, 600.0));
        assert!(t.len() >= 12 && t.iter().all(|(id, _)| f64::from(id.x) < world(18) / TILE));
    }

    /// Review focus 4: a view across the antimeridian wraps its columns; nothing beyond a pole.
    #[test]
    fn tiles_wrap_at_the_antimeridian() {
        let t = tiles_for((0.0, 128.0), 1, (512.0, 256.0));
        let mut xs: Vec<u32> = t.iter().map(|(id, _)| id.x).collect();
        xs.sort();
        xs.dedup();
        assert_eq!(xs, vec![0, 1], "{t:?}");
        let top = tiles_for((256.0, 0.0), 1, (256.0, 512.0));
        assert!(top.iter().all(|(id, _)| id.y < 2), "{top:?}");
    }

    #[test]
    fn the_url_and_the_cache_path() {
        let t = TileId { z: 3, x: 4, y: 5 };
        assert_eq!(
            tile_url("https://tile.openstreetmap.org/{z}/{x}/{y}.png", t),
            "https://tile.openstreetmap.org/3/4/5.png"
        );
        assert_eq!(
            cache_path(Path::new("/c"), t),
            PathBuf::from("/c/3/4/5.png")
        );
    }
}
