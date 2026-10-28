//! Faces (M10.18c): found by YuNet 2023mar (MIT), told apart by SFace 2021dec (Apache-2.0) — both OpenCV Zoo's.
//!
//! ⚠ The recipes are OpenCV's own `FaceDetectorYN` / `FaceRecognizerSF`, measured in the spike
//! (`examples/probe_faces.rs`): YuNet takes BGR 0–255 unnormalised at 640², padded at the right and bottom, and
//! answers per stride 8, 16, 32 four tensors — class `[N,1]`, objectness `[N,1]`, box `[N,4]` (centre offsets and
//! log sizes in cells), five landmarks `[N,10]` (offsets in cells) — in that order, cls×3 then obj×3, box×3, kps×3.
//! SFace takes the face warped onto ArcFace's 112² five-point template, RGB 0–255, and answers 128 numbers.

use crate::infer::InferError;
use image::imageops::{self, FilterType};
use image::RgbImage;
use tract_onnx::prelude::*;

/// YuNet's square.
pub const SIDE: u32 = 640;
/// A face below this score is not kept — OpenCV's default.
pub const SCORE: f32 = 0.6;
const NMS: f32 = 0.3;
const STRIDES: [u32; 3] = [8, 16, 32];
/// SFace's input edge.
const ALIGNED: u32 = 112;
/// ArcFace's five points in the 112² crop: the eyes (image left, image right), the nose, the mouth's corners.
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// One face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    /// `[x0, y0, x1, y1]`, normalised over the image handed to [`Faces::detect`].
    pub bbox: [f32; 4],
    /// The eyes, the nose and the mouth's corners, normalised — YuNet's order, which is the template's.
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

type Plan = std::sync::Arc<TypedRunnableModel>;

fn err(e: impl std::fmt::Display) -> InferError {
    InferError::Model(e.to_string())
}

fn load(path: &std::path::Path, shape: [usize; 4]) -> Result<Plan, InferError> {
    tract_onnx::onnx()
        .model_for_path(path)
        .map_err(err)?
        .with_input_fact(0, f32::fact(shape).into())
        .map_err(err)?
        .into_optimized()
        .map_err(err)?
        .into_runnable()
        .map_err(err)
}

/// Both networks, loaded and optimised once.
pub struct Faces {
    detector: Plan,
    recogniser: Plan,
}

impl Faces {
    pub fn load(yunet: &std::path::Path, sface: &std::path::Path) -> Result<Self, InferError> {
        Ok(Self {
            detector: load(yunet, [1, 3, SIDE as usize, SIDE as usize])?,
            recogniser: load(sface, [1, 3, ALIGNED as usize, ALIGNED as usize])?,
        })
    }

    /// The faces in `img` (upright), strongest first.
    pub fn detect(&self, img: &RgbImage) -> Result<Vec<Face>, InferError> {
        let scale = SIDE as f32 / img.width().max(img.height()).max(1) as f32;
        let (w, h) = (
            ((img.width() as f32 * scale).round() as u32).max(1),
            ((img.height() as f32 * scale).round() as u32).max(1),
        );
        let small = imageops::resize(img, w, h, FilterType::Triangle);
        let n = (SIDE * SIDE) as usize;
        let mut data = vec![0.0f32; 3 * n];
        for (x, y, p) in small.enumerate_pixels() {
            let i = (y * SIDE + x) as usize;
            // BGR, 0–255.
            data[i] = f32::from(p[2]);
            data[n + i] = f32::from(p[1]);
            data[2 * n + i] = f32::from(p[0]);
        }
        let t = Tensor::from_shape(&[1, 3, SIDE as usize, SIDE as usize], &data).map_err(err)?;
        let out = self.detector.run(tvec!(t.into())).map_err(err)?;
        if out.len() != 12 {
            return Err(InferError::Model(format!(
                "YuNet answered {} tensors, not 12",
                out.len()
            )));
        }
        let mut views = Vec::with_capacity(12);
        for o in out.iter() {
            let v = o.to_plain_array_view::<f32>().map_err(err)?;
            views.push(
                v.as_slice()
                    .ok_or_else(|| err("a non-contiguous output"))?
                    .to_vec(),
            );
        }
        let found = decode(&views, SIDE);
        // Back to the image's own frame: the square's pixels over the scaled image's size.
        let (sw, sh) = (w as f32, h as f32);
        Ok(nms(found
            .into_iter()
            .map(|f| Face {
                bbox: [
                    f.bbox[0] / sw,
                    f.bbox[1] / sh,
                    f.bbox[2] / sw,
                    f.bbox[3] / sh,
                ]
                .map(|v| v.clamp(0.0, 1.0)),
                landmarks: f.landmarks.map(|[x, y]| [x / sw, y / sh]),
                score: f.score,
            })
            .filter(|f| f.bbox[2] > f.bbox[0] && f.bbox[3] > f.bbox[1])
            .collect()))
    }

    /// `face`'s 128 numbers, of unit length — two of the same person score near 1 in [`similarity`].
    pub fn embed(&self, img: &RgbImage, face: &Face) -> Result<[f32; 128], InferError> {
        let pts = face
            .landmarks
            .map(|[x, y]| [x * img.width() as f32, y * img.height() as f32]);
        let crop = align(img, &pts);
        let n = (ALIGNED * ALIGNED) as usize;
        let mut data = vec![0.0f32; 3 * n];
        for (x, y, p) in crop.enumerate_pixels() {
            let i = (y * ALIGNED + x) as usize;
            // RGB, 0–255 (OpenCV's `swapRB = true`).
            for c in 0..3 {
                data[c * n + i] = f32::from(p[c]);
            }
        }
        let t =
            Tensor::from_shape(&[1, 3, ALIGNED as usize, ALIGNED as usize], &data).map_err(err)?;
        let out = self.recogniser.run(tvec!(t.into())).map_err(err)?;
        let v = out[0].to_plain_array_view::<f32>().map_err(err)?;
        let s = v.as_slice().ok_or_else(|| err("a non-contiguous output"))?;
        if s.len() != 128 || s.iter().any(|v| !v.is_finite()) {
            return Err(err("SFace answered no 128 finite numbers"));
        }
        let norm = s.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
        let mut e = [0.0f32; 128];
        for (o, v) in e.iter_mut().zip(s) {
            *o = v / norm;
        }
        Ok(e)
    }
}

/// How alike two faces are: the cosine of two unit vectors, 1 for the same.
pub fn similarity(a: &[f32; 128], b: &[f32; 128]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// YuNet's raw tensors (cls×3, obj×3, box×3, kps×3, flattened) as faces in the `side`² square's pixels.
pub fn decode(t: &[Vec<f32>], side: u32) -> Vec<Face> {
    let mut out = Vec::new();
    for (k, &stride) in STRIDES.iter().enumerate() {
        let cols = (side / stride) as usize;
        let (cls, obj, bx, kps) = (&t[k], &t[3 + k], &t[6 + k], &t[9 + k]);
        for i in 0..cls.len() {
            let score = (cls[i].clamp(0.0, 1.0)
                * obj.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0))
            .sqrt();
            if score < SCORE {
                continue;
            }
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            let s = stride as f32;
            let b = &bx[4 * i..4 * i + 4];
            let (cx, cy) = ((c + b[0]) * s, (r + b[1]) * s);
            let (w, h) = (b[2].exp() * s, b[3].exp() * s);
            let mut landmarks = [[0.0f32; 2]; 5];
            for (n, l) in landmarks.iter_mut().enumerate() {
                *l = [
                    (kps[10 * i + 2 * n] + c) * s,
                    (kps[10 * i + 2 * n + 1] + r) * s,
                ];
            }
            out.push(Face {
                bbox: [cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0],
                landmarks,
                score,
            });
        }
    }
    out
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    inter / (area(a) + area(b) - inter).max(1e-12)
}

/// The strongest of overlapping faces.
pub fn nms(mut v: Vec<Face>) -> Vec<Face> {
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Face> = Vec::new();
    for f in v {
        if keep.iter().all(|k| iou(&k.bbox, &f.bbox) < NMS) {
            keep.push(f);
        }
    }
    keep
}

/// The similarity transform `[a -b; b a] + t` taking `from` onto `to` in the least-squares sense.
pub fn similarity_transform(from: &[[f32; 2]; 5], to: &[[f32; 2]; 5]) -> [f32; 4] {
    let mean = |p: &[[f32; 2]; 5]| {
        let s = p.iter().fold([0.0, 0.0], |a, q| [a[0] + q[0], a[1] + q[1]]);
        [s[0] / 5.0, s[1] / 5.0]
    };
    let (mf, mt) = (mean(from), mean(to));
    let (mut num_a, mut num_b, mut den) = (0.0f32, 0.0f32, 0.0f32);
    for (p, q) in from.iter().zip(to) {
        let (px, py) = (p[0] - mf[0], p[1] - mf[1]);
        let (qx, qy) = (q[0] - mt[0], q[1] - mt[1]);
        num_a += px * qx + py * qy;
        num_b += px * qy - py * qx;
        den += px * px + py * py;
    }
    let (a, b) = (num_a / den.max(1e-12), num_b / den.max(1e-12));
    let tx = mt[0] - (a * mf[0] - b * mf[1]);
    let ty = mt[1] - (b * mf[0] + a * mf[1]);
    [a, b, tx, ty]
}

/// The face at `pts` (pixels) warped onto the 112² template, sampled bilinearly.
pub fn align(img: &RgbImage, pts: &[[f32; 2]; 5]) -> RgbImage {
    let [a, b, tx, ty] = similarity_transform(pts, &TEMPLATE);
    // The inverse of the similarity: crop pixel → source pixel.
    let det = (a * a + b * b).max(1e-12);
    let mut out = RgbImage::new(ALIGNED, ALIGNED);
    let (w, h) = (img.width() as f32, img.height() as f32);
    for (x, y, px) in out.enumerate_pixels_mut() {
        let (qx, qy) = (x as f32 - tx, y as f32 - ty);
        let sx = (a * qx + b * qy) / det;
        let sy = (-b * qx + a * qy) / det;
        if sx < 0.0 || sy < 0.0 || sx > w - 1.0 || sy > h - 1.0 {
            continue;
        }
        let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
        let (x1, y1) = (
            (x0 + 1).min(img.width() - 1),
            (y0 + 1).min(img.height() - 1),
        );
        let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
        for c in 0..3 {
            let g = |x, y| f32::from(img.get_pixel(x, y)[c]);
            let v = g(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + g(x1, y0) * fx * (1.0 - fy)
                + g(x0, y1) * (1.0 - fx) * fy
                + g(x1, y1) * fx * fy;
            px[c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One strong cell at stride 32 decodes to its box and landmarks; nothing else passes the score.
    #[test]
    fn decode_reads_one_constructed_face() {
        let side = 640u32;
        let counts = STRIDES.map(|s| ((side / s) * (side / s)) as usize);
        let mut t: Vec<Vec<f32>> = Vec::new();
        for n in counts {
            t.push(vec![0.0; n]); // cls
        }
        for n in counts {
            t.push(vec![0.0; n]); // obj
        }
        for n in counts {
            t.push(vec![0.0; 4 * n]);
        }
        for n in counts {
            t.push(vec![0.0; 10 * n]);
        }
        // Stride 32 (k = 2), the cell at column 3, row 2: index 2·20 + 3.
        let i = 2 * 20 + 3;
        t[2][i] = 0.9;
        t[5][i] = 0.9;
        t[8][4 * i..4 * i + 4].copy_from_slice(&[0.5, 0.5, 0.0, 1.0f32.ln()]);
        t[11][10 * i] = 0.25;
        let f = decode(&t, side);
        assert_eq!(f.len(), 1, "{f:?}");
        let f = f[0];
        assert!((f.score - 0.9).abs() < 1e-6);
        // Centre (3.5·32, 2.5·32) = (112, 80), size 32 × 32.
        assert_eq!(f.bbox, [96.0, 64.0, 128.0, 96.0]);
        assert_eq!(f.landmarks[0], [(0.25 + 3.0) * 32.0, 2.0 * 32.0]);
    }

    #[test]
    fn the_alignment_maps_the_template_to_itself() {
        let [a, b, tx, ty] = similarity_transform(&TEMPLATE, &TEMPLATE);
        assert!((a - 1.0).abs() < 1e-5 && b.abs() < 1e-5 && tx.abs() < 1e-3 && ty.abs() < 1e-3);
        // Twice as large and shifted: the transform halves and moves back.
        let big = TEMPLATE.map(|[x, y]| [2.0 * x + 10.0, 2.0 * y + 20.0]);
        let [a, b, ..] = similarity_transform(&big, &TEMPLATE);
        assert!((a - 0.5).abs() < 1e-4 && b.abs() < 1e-4, "{a} {b}");
    }

    #[test]
    fn similarity_of_a_vector_with_itself_is_one() {
        let mut v = [0.0f32; 128];
        v[3] = 0.6;
        v[70] = 0.8;
        assert!((similarity(&v, &v) - 1.0).abs() < 1e-6);
        let mut w = [0.0f32; 128];
        w[5] = 1.0;
        assert_eq!(similarity(&v, &w), 0.0);
    }

    /// The real networks on a real photo: the Apollo 11 crew's NASA portrait (public domain, Wikimedia
    /// Commons, 960 px — `tests/fixtures/apollo11_crew.jpg`). Three faces; one face read twice, at two
    /// sizes, agrees, and two people do not. ⚠ Needs both models in the app's model directory, so ignored:
    /// `cargo test --workspace --release a_portrait -- --ignored`.
    #[test]
    #[ignore]
    fn a_portrait_has_three_faces_and_one_face_agrees_with_itself() {
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join(".local/share/rustroom/models");
        let faces = Faces::load(
            &dir.join("face_detection_yunet_2023mar.onnx"),
            &dir.join("face_recognition_sface_2021dec.onnx"),
        )
        .expect("the models");
        let img = image::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/apollo11_crew.jpg"
        ))
        .unwrap()
        .to_rgb8();
        let found = faces.detect(&img).unwrap();
        println!("{found:#?}");
        assert_eq!(found.len(), 3, "{found:?}");
        let big = image::imageops::resize(
            &img,
            img.width() * 3 / 2,
            img.height() * 3 / 2,
            FilterType::Triangle,
        );
        let again = faces.detect(&big).unwrap();
        assert_eq!(again.len(), 3);
        let e = |im: &RgbImage, f: &Face| faces.embed(im, f).unwrap();
        // The same face at both sizes: the one nearest the first's centre.
        let centre = |f: &Face| [(f.bbox[0] + f.bbox[2]) / 2.0, (f.bbox[1] + f.bbox[3]) / 2.0];
        let near = again
            .iter()
            .min_by(|a, b| {
                let d = |g: &Face| {
                    (centre(g)[0] - centre(&found[0])[0]).powi(2)
                        + (centre(g)[1] - centre(&found[0])[1]).powi(2)
                };
                d(a).total_cmp(&d(b))
            })
            .unwrap();
        let same = similarity(&e(&img, &found[0]), &e(&big, near));
        let other = similarity(&e(&img, &found[0]), &e(&img, &found[1]));
        println!("same {same:.3}, other {other:.3}");
        assert!(same > 0.7, "one face read twice: {same}");
        assert!(other < 0.363, "two people: {other}");
    }

    #[test]
    fn nms_keeps_the_stronger_of_two_overlapping() {
        let f = |x: f32, s: f32| Face {
            bbox: [x, 0.0, x + 10.0, 10.0],
            landmarks: [[0.0; 2]; 5],
            score: s,
        };
        let kept = nms(vec![f(0.0, 0.7), f(1.0, 0.9), f(50.0, 0.8)]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9);
    }
}
