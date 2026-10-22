//! People found by YOLOX-m (M10.16c3) — Apache-2.0, Megvii's own `0.1.1rc0` export: the whole frame at 640 and
//! 640² tiles over it, merged. The boxes only; the app outlines each with MobileSAM and maps it to the decode.
//!
//! ⚠ The export's recipe, measured in the spike (`examples/people_probe.rs`): BGR 0–255, no normalisation, padded
//! with 114 at the right and bottom; the output `[1, 8400, 85]` raw — per stride (8, 16, 32) a grid of offsets and
//! log sizes, then objectness and the 80 class scores already through a sigmoid. Person is class 0.

use crate::infer::InferError;
use image::imageops::{self, FilterType};
use image::RgbImage;
use tract_onnx::prelude::*;

/// The network's square.
pub const SIDE: u32 = 640;
/// The tiled pass: the image at this long side, in squares overlapping by [`OVERLAP`] — the small, distant
/// people the whole frame at 640 misses (44 more on the spike's seven photos).
pub const TILED_LONG: u32 = 1920;
pub const OVERLAP: u32 = 160;
/// A person below this score is not offered — the demo's own drawing threshold is 0.3.
pub const SCORE: f32 = 0.35;
const NMS: f32 = 0.45;
/// A box this much inside a stronger one is the same person — a part of them found again.
const CONTAINED: f32 = 0.7;
/// A tile's find within this many pixels of an edge the image goes on past is cut — the next tile holds it.
const EDGE: f32 = 3.0;
const ANCHORS: usize = 8400;
const ROW: usize = 85;

/// One person.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Found {
    /// `[x0, y0, x1, y1]`, normalised over the image handed to [`People::find`].
    pub bbox: [f32; 4],
    pub score: f32,
}

type Plan = std::sync::Arc<TypedRunnableModel>;

fn err(e: impl std::fmt::Display) -> InferError {
    InferError::Model(e.to_string())
}

/// YOLOX-m, loaded and optimised once.
pub struct People {
    plan: Plan,
}

impl People {
    pub fn load(path: &std::path::Path) -> Result<Self, InferError> {
        let plan = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(err)?
            .with_input_fact(0, f32::fact([1, 3, SIDE as usize, SIDE as usize]).into())
            .map_err(err)?
            .into_optimized()
            .map_err(err)?
            .into_runnable()
            .map_err(err)?;
        Ok(Self { plan })
    }

    /// The people in `img` (upright) at 640 long, without the tiles — a small area the author drew over, already
    /// close enough for its people to be many pixels tall.
    pub fn find_whole(&self, img: &RgbImage) -> Result<Vec<Found>, InferError> {
        let sq = resize_long(img, SIDE);
        let (sw, sh) = (sq.width() as f32, sq.height() as f32);
        Ok(nms(self
            .square(&sq)?
            .into_iter()
            .map(|(b, score)| Found {
                bbox: norm(b, [0.0, 0.0], sw, sh),
                score,
            })
            .collect()))
    }

    /// The people in `img` (upright): the whole frame, then the tiles — `progress` told after each square, 1 at
    /// the end.
    pub fn find(&self, img: &RgbImage, progress: &dyn Fn(f32)) -> Result<Vec<Found>, InferError> {
        let whole = self.find_whole(img)?;
        let work = resize_long(img, TILED_LONG);
        let (w, h) = (work.width(), work.height());
        let (xs, ys) = (offsets(w), offsets(h));
        let total = (1 + xs.len() * ys.len()) as f32;
        let mut done = 1.0;
        progress(done / total);
        let mut tiles = Vec::new();
        for &oy in &ys {
            for &ox in &xs {
                let size = [SIDE.min(w - ox), SIDE.min(h - oy)];
                let tile = imageops::crop_imm(&work, ox, oy, size[0], size[1]).to_image();
                for (b, score) in self.square(&tile)? {
                    if !cut(b, [ox, oy], size, w, h) {
                        tiles.push(Found {
                            bbox: norm(b, [ox as f32, oy as f32], w as f32, h as f32),
                            score,
                        });
                    }
                }
                done += 1.0;
                progress(done / total);
            }
        }
        Ok(merge(whole, nms(tiles)))
    }

    /// One square (at most 640², at the top left, the rest padded) through the network: the people in its pixels.
    fn square(&self, sq: &RgbImage) -> Result<Vec<([f32; 4], f32)>, InferError> {
        let s = SIDE as usize;
        let n = s * s;
        let mut input = vec![114f32; 3 * n];
        for (x, y, p) in sq.enumerate_pixels() {
            let i = y as usize * s + x as usize;
            input[i] = p[2] as f32;
            input[n + i] = p[1] as f32;
            input[2 * n + i] = p[0] as f32;
        }
        let t = Tensor::from_shape(&[1, 3, s, s], &input).map_err(err)?;
        let out = self.plan.run(tvec!(t.into())).map_err(err)?;
        let o = out[0].to_plain_array_view::<f32>().map_err(err)?;
        if o.shape() != [1, ANCHORS, ROW] {
            return Err(err(format!("YOLOX answered {:?}", o.shape())));
        }
        let o = o.as_slice().ok_or_else(|| err("a non-contiguous output"))?;
        if let Some(bad) = o.iter().find(|v| !v.is_finite()) {
            return Err(err(format!("a non-finite output ({bad})")));
        }
        Ok(decode(o))
    }
}

/// The people in one output, in its square's pixels, scoring at least [`SCORE`].
pub fn decode(o: &[f32]) -> Vec<([f32; 4], f32)> {
    let mut found = Vec::new();
    let mut k = 0;
    for stride in [8usize, 16, 32] {
        let g = SIDE as usize / stride;
        for gy in 0..g {
            for gx in 0..g {
                let r = &o[k * ROW..(k + 1) * ROW];
                k += 1;
                let score = r[4] * r[5];
                if score < SCORE {
                    continue;
                }
                let st = stride as f32;
                let (cx, cy) = ((r[0] + gx as f32) * st, (r[1] + gy as f32) * st);
                let (w, h) = (r[2].exp() * st, r[3].exp() * st);
                found.push((
                    [cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0],
                    score,
                ));
            }
        }
    }
    found
}

fn area(b: [f32; 4]) -> f32 {
    (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0)
}

/// Intersection over union, and the share of the smaller box inside the other.
fn iou_and_containment(a: [f32; 4], b: [f32; 4]) -> (f32, f32) {
    let i = area([
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]);
    let u = area(a) + area(b) - i;
    (i / u.max(1e-9), i / area(a).min(area(b)).max(1e-9))
}

/// Strongest first; a box overlapping a kept one by IoU over 0.45, or mostly inside it, goes.
pub fn nms(mut v: Vec<Found>) -> Vec<Found> {
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Found> = Vec::new();
    for f in v {
        let fresh = keep.iter().all(|k| {
            let (iou, inside) = iou_and_containment(k.bbox, f.bbox);
            iou <= NMS && inside <= CONTAINED
        });
        if fresh {
            keep.push(f);
        }
    }
    keep
}

/// The squares' left (or top) edges along `len`: from 0, one every `SIDE − OVERLAP`, the last flush with the end.
pub fn offsets(len: u32) -> Vec<u32> {
    if len <= SIDE {
        return vec![0];
    }
    let mut v: Vec<u32> = (0..)
        .map(|k| k * (SIDE - OVERLAP))
        .take_while(|&o| o + SIDE < len)
        .collect();
    v.push(len - SIDE);
    v
}

/// Whether box `b` (the tile's pixels) of the tile at `at`, `size` big, in a `w × h` image, touches an edge the
/// image goes on past.
fn cut(b: [f32; 4], at: [u32; 2], size: [u32; 2], w: u32, h: u32) -> bool {
    (b[0] < EDGE && at[0] > 0)
        || (b[1] < EDGE && at[1] > 0)
        || (b[2] > size[0] as f32 - EDGE && at[0] + size[0] < w)
        || (b[3] > size[1] as f32 - EDGE && at[1] + size[1] < h)
}

/// The whole frame's finds, then each tile's that none of them covers.
pub fn merge(whole: Vec<Found>, tiles: Vec<Found>) -> Vec<Found> {
    let mut all = whole.clone();
    for t in tiles {
        let seen = whole.iter().any(|p| {
            let (iou, inside) = iou_and_containment(p.bbox, t.bbox);
            iou > 0.3 || inside > 0.6
        });
        if !seen {
            all.push(t);
        }
    }
    all
}

/// A box in a square at `at` of a `w × h` image, normalised over the image.
fn norm(b: [f32; 4], at: [f32; 2], w: f32, h: f32) -> [f32; 4] {
    [
        (b[0] + at[0]) / w,
        (b[1] + at[1]) / h,
        (b[2] + at[0]) / w,
        (b[3] + at[1]) / h,
    ]
    .map(|v| v.clamp(0.0, 1.0))
}

fn resize_long(img: &RgbImage, long: u32) -> RgbImage {
    let r = long as f32 / img.width().max(img.height()).max(1) as f32;
    imageops::resize(
        img,
        ((img.width() as f32 * r).round() as u32).max(1),
        ((img.height() as f32 * r).round() as u32).max(1),
        FilterType::Triangle,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One anchor per stride, each with its own offsets: the box it stands for, in the 640 square's pixels.
    #[test]
    fn the_output_grid_decodes_to_boxes() {
        let mut out = vec![0f32; 8400 * 85];
        let set = |out: &mut Vec<f32>, k: usize, r: [f32; 4], obj: f32, person: f32| {
            out[k * 85..k * 85 + 4].copy_from_slice(&r);
            out[k * 85 + 4] = obj;
            out[k * 85 + 5] = person;
        };
        // Stride 8: anchor (gx 10, gy 20) = index 20 * 80 + 10; centre offset (0.5, 0.5), log size ln 4 → 32 px.
        set(
            &mut out,
            20 * 80 + 10,
            [0.5, 0.5, 4f32.ln(), 4f32.ln()],
            0.9,
            0.9,
        );
        // Stride 32 (after 6400 + 1600 anchors): (gx 3, gy 2) — too faint to count (0.25).
        set(&mut out, 6400 + 1600 + 2 * 20 + 3, [0.0; 4], 0.5, 0.5);
        // Stride 16 (after 6400): (gx 5, gy 7), a dog — another class — is not a person.
        set(&mut out, 6400 + 7 * 40 + 5, [0.0; 4], 0.95, 0.0);
        let got = decode(&out);
        assert_eq!(got.len(), 1, "{got:?}");
        let (b, s) = got[0];
        assert!((s - 0.81).abs() < 1e-5, "{s}");
        let want = [
            10.5 * 8.0 - 16.0,
            20.5 * 8.0 - 16.0,
            10.5 * 8.0 + 16.0,
            20.5 * 8.0 + 16.0,
        ];
        for (g, w) in b.iter().zip(want) {
            assert!((g - w).abs() < 1e-3, "{b:?} against {want:?}");
        }
    }

    #[test]
    fn nms_keeps_the_stronger_of_two_overlapping_boxes_and_a_box_inside_another_goes() {
        let f = |b: [f32; 4], score| Found { bbox: b, score };
        let kept = nms(vec![
            f([0.0, 0.0, 10.0, 10.0], 0.6),
            f([1.0, 1.0, 11.0, 11.0], 0.9),
            f([20.0, 0.0, 30.0, 10.0], 0.5),
            // Inside the third — IoU only 0.09, so only containment removes it.
            f([21.0, 1.0, 24.0, 4.0], 0.4),
        ]);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_eq!(kept[0].score, 0.9);
        assert_eq!(kept[1].score, 0.5);
    }

    #[test]
    fn the_tiles_cover_the_image() {
        for len in [641u32, 1280, 1920, 1000] {
            let o = offsets(len);
            assert_eq!(o[0], 0);
            assert_eq!(o.last().unwrap() + SIDE, len, "{len}: {o:?}");
            assert!(
                o.windows(2)
                    .all(|w| w[1] > w[0] && w[1] - w[0] <= SIDE - OVERLAP),
                "{len}: {o:?}"
            );
        }
        assert_eq!(offsets(640), vec![0]);
        assert_eq!(offsets(500), vec![0]);
    }

    /// A find within 3 px of an inner tile edge is the next tile's; at the image's own edge it stays.
    #[test]
    fn a_find_cut_by_an_inner_tile_edge_is_dropped() {
        let (w, h): (u32, u32) = (1920, 1280);
        let left = [0.0, 100.0, 50.0, 200.0];
        assert!(
            cut(left, [480, 0], [640, 640], w, h),
            "an inner tile's left edge"
        );
        assert!(
            !cut(left, [0, 0], [640, 640], w, h),
            "the image's own left edge"
        );
        let right = [600.0, 100.0, 639.0, 200.0];
        assert!(
            cut(right, [0, 0], [640, 640], w, h),
            "more image beyond the right edge"
        );
        assert!(
            !cut(right, [1280, 0], [640, 640], w, h),
            "the image's own right edge"
        );
        let inside = [100.0, 100.0, 200.0, 300.0];
        assert!(
            !cut(inside, [480, 480], [640, 640], w, h),
            "nowhere near an edge"
        );
    }

    #[test]
    fn a_tile_find_is_kept_only_where_no_whole_frame_find_covers_it() {
        let f = |b: [f32; 4]| Found {
            bbox: b,
            score: 0.8,
        };
        let whole = vec![f([0.10, 0.10, 0.30, 0.60])];
        let tiles = vec![
            // The same person, found again.
            f([0.11, 0.12, 0.29, 0.58]),
            // A part of them — their top — inside the whole-frame box at IoU 0.14: containment's case.
            f([0.12, 0.12, 0.20, 0.30]),
            // A small one only the tiles saw.
            f([0.70, 0.50, 0.72, 0.55]),
        ];
        let m = merge(whole, tiles);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m[1].bbox, [0.70, 0.50, 0.72, 0.55]);
    }

    /// The real network on one of the author's photos — `RUSTROOM_PEOPLE_PHOTO` a JPEG with a person in it,
    /// `RUSTROOM_YOLOX` the model file.
    #[test]
    #[ignore = "needs the model and a photo"]
    fn the_real_network_finds_a_person() {
        let (Some(photo), Some(model)) = (
            std::env::var_os("RUSTROOM_PEOPLE_PHOTO"),
            std::env::var_os("RUSTROOM_YOLOX"),
        ) else {
            panic!("set RUSTROOM_PEOPLE_PHOTO and RUSTROOM_YOLOX");
        };
        // ⚠ Upright, as the app hands it — `image::open` ignores the EXIF orientation, and a portrait read sideways
        // hides its people from the whole-frame pass (this test's first version, on DSCF8296).
        let mut dec = image::ImageReader::open(photo)
            .unwrap()
            .into_decoder()
            .unwrap();
        let o = image::ImageDecoder::orientation(&mut dec).unwrap();
        let mut img = image::DynamicImage::from_decoder(dec).unwrap();
        img.apply_orientation(o);
        let img = img.to_rgb8();
        let p = People::load(std::path::Path::new(&model)).unwrap();
        let steps = std::cell::RefCell::new(Vec::new());
        let found = p.find(&img, &|f| steps.borrow_mut().push(f)).unwrap();
        assert!(found.iter().any(|f| f.score > 0.8), "{found:?}");
        assert!(found
            .iter()
            .all(|f| f.bbox.iter().all(|v| (0.0..=1.0).contains(v))));
        let steps = steps.into_inner();
        assert_eq!(steps.last(), Some(&1.0), "{steps:?}");
        // The whole frame alone, as a drawn area is searched: the strongest find is the same person.
        let whole = p.find_whole(&img).unwrap();
        assert!(whole.iter().any(|f| f.score > 0.8), "{whole:?}");
    }
}
