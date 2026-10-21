//! M10.16c3's spike — **throwaway**: people found by YOLOX (Apache-2.0, Megvii's own exports) and outlined by
//! MobileSAM from each box, then every one removed by LaMa, on the author's photos. Nothing here ships; the
//! build is its own plan, test first.
//!
//! `cargo run --release -p ai-mask --example people_probe -- scan YOLOX DIR…` — every JPEG's people (YOLOX on
//! the whole frame at 640), to pick photos with people in them.
//!
//! `cargo run --release -p ai-mask --example people_probe -- sheet OUT MODELS YOLOX[,YOLOX…] JPG…` — per photo
//! and model `OUT/<photo>_<model>.png`: left the people found (green on the whole frame, yellow only in the
//! tiles, blue the subject — kept) with MobileSAM's outline of each (red), right every bystander removed by LaMa;
//! and
//! `OUT/<photo>_<model>_people.png`, each person magnified, before and after. Times printed.

use ai_mask::inpaint::{Inpaint, SIDE as LAMA};
use ai_mask::{ModelId, ProbMap, Sam};
use image::imageops::{self, FilterType};
use image::{Rgb, RgbImage};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tract_onnx::prelude::*;

/// YOLOX-s and -m take a 640² square.
const SIDE: u32 = 640;
/// A person below this score is not drawn — the demo's own visual threshold is 0.3.
const SCORE: f32 = 0.35;
const NMS: f32 = 0.45;
/// The tiled pass: the frame at this long side, cut into 640² tiles overlapping by `OVERLAP`.
const TILED_LONG: u32 = 1920;
const OVERLAP: u32 = 160;
/// The removal works on the photo at this long side.
const WORK_LONG: u32 = 2048;
/// A person this tall (a share of the frame's height) or taller is the photo's subject: outlined, never
/// removed — the sheets show what removing the bystanders gives.
const SUBJECT: f32 = 0.35;

fn bystander(p: &Person) -> bool {
    p.b[3] - p.b[1] < SUBJECT
}

type Plan = std::sync::Arc<TypedRunnableModel>;

#[derive(Clone, Copy, Debug)]
struct Person {
    /// `[x0, y0, x1, y1]`, normalised over the photo.
    b: [f32; 4],
    score: f32,
    /// Found only by the tiled pass.
    tiled_only: bool,
}

fn load_jpeg(path: &Path) -> RgbImage {
    let mut dec = image::ImageReader::open(path)
        .unwrap()
        .into_decoder()
        .unwrap();
    let o = image::ImageDecoder::orientation(&mut dec).unwrap();
    let mut img = image::DynamicImage::from_decoder(dec).unwrap();
    img.apply_orientation(o);
    img.to_rgb8()
}

fn load_yolox(path: &Path) -> Plan {
    tract_onnx::onnx()
        .model_for_path(path)
        .unwrap()
        .with_input_fact(0, f32::fact([1, 3, SIDE as usize, SIDE as usize]).into())
        .unwrap()
        .into_optimized()
        .unwrap()
        .into_runnable()
        .unwrap()
}

fn resize_long(img: &RgbImage, long: u32) -> RgbImage {
    let r = long as f32 / img.width().max(img.height()) as f32;
    imageops::resize(
        img,
        ((img.width() as f32 * r).round() as u32).max(1),
        ((img.height() as f32 * r).round() as u32).max(1),
        FilterType::Triangle,
    )
}

/// `sq` (at most 640², top-left, the rest padded with 114 — YOLOX's own `preproc`) through YOLOX: the people,
/// in `sq`'s pixels.
///
/// ⚠ The 0.1.1rc0 exports' recipe (its `demo/ONNXRuntime`): BGR, 0…255, no normalisation; the output
/// `[1, 8400, 85]` raw — box offsets on each stride's grid (8, 16, 32), log sizes, objectness and the 80 class
/// scores already through a sigmoid. Person is class 0.
fn detect(plan: &Plan, sq: &RgbImage) -> Vec<([f32; 4], f32)> {
    let s = SIDE as usize;
    let n = s * s;
    let mut input = vec![114f32; 3 * n];
    for (x, y, p) in sq.enumerate_pixels() {
        let i = y as usize * s + x as usize;
        input[i] = p[2] as f32;
        input[n + i] = p[1] as f32;
        input[2 * n + i] = p[0] as f32;
    }
    let t = Tensor::from_shape(&[1, 3, s, s], &input).unwrap();
    let out = plan.run(tvec!(t.into())).unwrap();
    let o = out[0].to_plain_array_view::<f32>().unwrap();
    assert_eq!(o.shape(), &[1, 8400, 85], "YOLOX's output");
    let o = o.as_slice().unwrap();
    if std::env::var_os("PEOPLE_DEBUG").is_some() {
        let best = (0..8400)
            .max_by(|&a, &b| o[a * 85 + 4].total_cmp(&o[b * 85 + 4]))
            .unwrap();
        let r = &o[best * 85..(best + 1) * 85];
        let class = (0..80)
            .max_by(|&a, &b| r[5 + a].total_cmp(&r[5 + b]))
            .unwrap();
        eprintln!(
            "  debug: best objectness {:.3} at anchor {best}, its class {class} ({:.3}), person {:.3}; raw {:?}",
            r[4],
            r[5 + class],
            r[5],
            &r[..4]
        );
    }
    let mut found = Vec::new();
    let mut k = 0;
    for stride in [8usize, 16, 32] {
        let g = s / stride;
        for gy in 0..g {
            for gx in 0..g {
                let r = &o[k * 85..(k + 1) * 85];
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

fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    area([
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ])
}

/// Greedy NMS by score: a box overlapping a kept one by `NMS` IoU, or lying mostly inside it, goes.
fn nms(mut v: Vec<Person>) -> Vec<Person> {
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Person> = Vec::new();
    for p in v {
        let dup = keep.iter().any(|k| {
            let i = overlap(k.b, p.b);
            i / (area(k.b) + area(p.b) - i) > NMS || i / area(k.b).min(area(p.b)) > 0.7
        });
        if !dup {
            keep.push(p);
        }
    }
    keep
}

/// The whole frame at 640 long.
fn whole(plan: &Plan, img: &RgbImage) -> Vec<Person> {
    let sq = resize_long(img, SIDE);
    let (w, h) = (sq.width() as f32, sq.height() as f32);
    nms(detect(plan, &sq)
        .into_iter()
        .map(|(b, score)| Person {
            b: [b[0] / w, b[1] / h, b[2] / w, b[3] / h].map(|v| v.clamp(0.0, 1.0)),
            score,
            tiled_only: false,
        })
        .collect())
}

fn offsets(len: u32) -> Vec<u32> {
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

/// The frame at `TILED_LONG`, in overlapping 640² tiles; a box cut by an inner tile edge is left to the tile
/// that holds it whole.
fn tiled(plan: &Plan, img: &RgbImage) -> Vec<Person> {
    let work = resize_long(img, TILED_LONG);
    let (w, h) = (work.width(), work.height());
    let mut found = Vec::new();
    for &oy in &offsets(h) {
        for &ox in &offsets(w) {
            let (tw, th) = (SIDE.min(w - ox), SIDE.min(h - oy));
            let tile = imageops::crop_imm(&work, ox, oy, tw, th).to_image();
            for (b, score) in detect(plan, &tile) {
                let cut = (b[0] < 3.0 && ox > 0)
                    || (b[1] < 3.0 && oy > 0)
                    || (b[2] > tw as f32 - 3.0 && ox + tw < w)
                    || (b[3] > th as f32 - 3.0 && oy + th < h);
                if cut {
                    continue;
                }
                found.push(Person {
                    b: [
                        (b[0] + ox as f32) / w as f32,
                        (b[1] + oy as f32) / h as f32,
                        (b[2] + ox as f32) / w as f32,
                        (b[3] + oy as f32) / h as f32,
                    ]
                    .map(|v| v.clamp(0.0, 1.0)),
                    score,
                    tiled_only: true,
                });
            }
        }
    }
    nms(found)
}

/// Whole-frame finds, plus the tiled finds no whole-frame find covers.
fn merged(whole: &[Person], tiled: &[Person]) -> Vec<Person> {
    let mut all = whole.to_vec();
    for t in tiled {
        let seen = whole.iter().any(|p| {
            let i = overlap(p.b, t.b);
            i / (area(p.b) + area(t.b) - i) > 0.3 || i / area(p.b).min(area(t.b)) > 0.6
        });
        if !seen {
            all.push(*t);
        }
    }
    all
}

fn at(m: &ProbMap, x: f32, y: f32) -> bool {
    let (mx, my) = (
        ((x * m.w as f32) as u32).min(m.w - 1),
        ((y * m.h as f32) as u32).min(m.h - 1),
    );
    m.data[(my * m.w + mx) as usize] >= 128
}

/// `img` with each person's box (green, yellow when tiled only) and the outline of its mask (red).
fn draw(img: &RgbImage, people: &[Person], masks: &[ProbMap]) -> RgbImage {
    let mut out = img.clone();
    let (w, h) = (out.width(), out.height());
    for (p, m) in people.iter().zip(masks) {
        let [x0, y0, x1, y1] = [
            p.b[0] * w as f32,
            p.b[1] * h as f32,
            p.b[2] * w as f32,
            p.b[3] * h as f32,
        ]
        .map(|v| v as i64);
        let c = if !bystander(p) {
            Rgb([60, 140, 255])
        } else if p.tiled_only {
            Rgb([255, 220, 0])
        } else {
            Rgb([40, 220, 60])
        };
        for x in x0..=x1 {
            for y in [y0, y1] {
                if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    out.put_pixel(x as u32, y as u32, c);
                }
            }
        }
        for y in y0..=y1 {
            for x in [x0, x1] {
                if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    out.put_pixel(x as u32, y as u32, c);
                }
            }
        }
        let pad = 4;
        for y in (y0 - pad).max(1)..(y1 + pad).min(h as i64 - 1) {
            for x in (x0 - pad).max(1)..(x1 + pad).min(w as i64 - 1) {
                let f = |dx: i64, dy: i64| {
                    at(m, (x + dx) as f32 / w as f32, (y + dy) as f32 / h as f32)
                };
                if f(0, 0) && (!f(1, 0) || !f(-1, 0) || !f(0, 1) || !f(0, -1)) {
                    out.put_pixel(x as u32, y as u32, Rgb([255, 40, 40]));
                }
            }
        }
    }
    out
}

/// Every person removed from `img` (its own size) by LaMa, one after another: the hole the mask grown by 4 % of
/// the person's height (at least 4 px), a square window 2.5 × the box (at least 512), the fill pasted inside the
/// hole's softened edge. Returns the seconds each took.
fn remove(
    img: &mut RgbImage,
    people: &[Person],
    masks: &[ProbMap],
    lama: &mut Inpaint,
) -> Vec<f64> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut times = Vec::new();
    for (p, m) in people.iter().zip(masks) {
        let t = Instant::now();
        let (bx0, by0, bx1, by1) = (
            p.b[0] * w as f32,
            p.b[1] * h as f32,
            p.b[2] * w as f32,
            p.b[3] * h as f32,
        );
        let grow = (0.04 * (by1 - by0)).max(4.0);
        let side = ((2.5 * (bx1 - bx0).max(by1 - by0)).max(LAMA as f32) as usize).min(w.min(h));
        let (cx, cy) = ((bx0 + bx1) / 2.0, (by0 + by1) / 2.0);
        let x0 = ((cx - side as f32 / 2.0).max(0.0) as usize).min(w - side);
        let y0 = ((cy - side as f32 / 2.0).max(0.0) as usize).min(h - side);
        let crop =
            imageops::crop_imm(img, x0 as u32, y0 as u32, side as u32, side as u32).to_image();
        let small = imageops::resize(&crop, LAMA as u32, LAMA as u32, FilterType::Triangle);
        let rgb: Vec<f32> = small
            .pixels()
            .flat_map(|p| p.0.map(|v| v as f32 / 255.0))
            .collect();
        // The hole at LaMa's size: the mask, grown.
        let g = (grow * LAMA as f32 / side as f32).ceil() as i64;
        let inside: Vec<bool> = (0..LAMA * LAMA)
            .map(|i| {
                let (u, v) = ((i % LAMA) as f32 + 0.5, (i / LAMA) as f32 + 0.5);
                at(
                    m,
                    (x0 as f32 + u * side as f32 / LAMA as f32) / w as f32,
                    (y0 as f32 + v * side as f32 / LAMA as f32) / h as f32,
                )
            })
            .collect();
        let mut hole = vec![0f32; LAMA * LAMA];
        for y in 0..LAMA as i64 {
            for x in 0..LAMA as i64 {
                let mut any = false;
                'n: for dy in -g..=g {
                    for dx in -g..=g {
                        let (sx, sy) = (x + dx, y + dy);
                        if dx * dx + dy * dy <= g * g
                            && sx >= 0
                            && sy >= 0
                            && sx < LAMA as i64
                            && sy < LAMA as i64
                            && inside[(sy * LAMA as i64 + sx) as usize]
                        {
                            any = true;
                            break 'n;
                        }
                    }
                }
                hole[(y * LAMA as i64 + x) as usize] = any as u8 as f32;
            }
        }
        let filled = lama.run(&rgb, &hole).expect("LaMa");
        // Paste at the window's own size, inside the hole's edge softened over 2 px of LaMa's square.
        let soft = |u: f32, v: f32| {
            let (x, y) = (u as i64, v as i64);
            let mut s = 0.0;
            let mut n = 0.0;
            for dy in -2..=2i64 {
                for dx in -2..=2i64 {
                    let (sx, sy) = (
                        (x + dx).clamp(0, LAMA as i64 - 1),
                        (y + dy).clamp(0, LAMA as i64 - 1),
                    );
                    s += hole[(sy * LAMA as i64 + sx) as usize];
                    n += 1.0;
                }
            }
            s / n
        };
        for y in 0..side {
            for x in 0..side {
                let (u, v) = (
                    x as f32 * LAMA as f32 / side as f32,
                    y as f32 * LAMA as f32 / side as f32,
                );
                let a = soft(u, v);
                if a <= 0.0 {
                    continue;
                }
                let k = (v as usize).min(LAMA - 1) * LAMA + (u as usize).min(LAMA - 1);
                let px = img.get_pixel_mut((x0 + x) as u32, (y0 + y) as u32);
                for c in 0..3 {
                    let f = filled[k * 3 + c] * 255.0;
                    px[c] = (px[c] as f32 * (1.0 - a) + f * a).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        times.push(t.elapsed().as_secs_f64());
    }
    times
}

/// Each person's surroundings (the box grown 1.6×), before with its outline and after, in cells of 280.
fn people_sheet(before: &RgbImage, after: &RgbImage, people: &[Person]) -> RgbImage {
    let cell = 280u32;
    let n = people.len().min(16) as u32;
    let mut out =
        RgbImage::from_pixel(2 * cell * 2, n.div_ceil(2).max(1) * cell, Rgb([30, 30, 30]));
    let (w, h) = (before.width() as f32, before.height() as f32);
    for (k, p) in people.iter().take(16).enumerate() {
        let (bw, bh) = ((p.b[2] - p.b[0]) * w, (p.b[3] - p.b[1]) * h);
        let s = (1.6 * bw.max(bh)).max(48.0).min(w.min(h));
        let (cx, cy) = ((p.b[0] + p.b[2]) / 2.0 * w, (p.b[1] + p.b[3]) / 2.0 * h);
        let x0 = (cx - s / 2.0).clamp(0.0, w - s) as u32;
        let y0 = (cy - s / 2.0).clamp(0.0, h - s) as u32;
        let (col, row) = (k as u32 % 2, k as u32 / 2);
        for (j, src) in [before, after].iter().enumerate() {
            let c = imageops::crop_imm(*src, x0, y0, s as u32, s as u32).to_image();
            let c = imageops::resize(&c, cell, cell, FilterType::Triangle);
            imageops::replace(
                &mut out,
                &c,
                ((col * 2 + j as u32) * cell) as i64,
                (row * cell) as i64,
            );
        }
    }
    out
}

fn jpegs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            jpegs(&p, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
        {
            out.push(p);
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("scan") => {
            let plan = load_yolox(Path::new(&a[2]));
            let mut files = Vec::new();
            for d in &a[3..] {
                jpegs(Path::new(d), &mut files);
            }
            eprintln!("{} photos", files.len());
            for f in files {
                let img = load_jpeg(&f);
                let people = whole(&plan, &img);
                if people.is_empty() {
                    continue;
                }
                let small = people.iter().filter(|p| p.b[3] - p.b[1] < 0.15).count();
                let best = people.iter().map(|p| p.score).fold(0.0, f32::max);
                println!(
                    "{:3} people  {:3} small  best {:.2}  {}",
                    people.len(),
                    small,
                    best,
                    f.display()
                );
            }
        }
        Some("sheet") => {
            let out = PathBuf::from(&a[2]);
            let models = PathBuf::from(&a[3]);
            std::fs::create_dir_all(&out).unwrap();
            let yolox: Vec<(String, Plan)> = a[4]
                .split(',')
                .map(|p| {
                    let t = Instant::now();
                    let plan = load_yolox(Path::new(p));
                    let name = Path::new(p)
                        .file_stem()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    println!("{name}: loaded in {:.1} s", t.elapsed().as_secs_f64());
                    (name, plan)
                })
                .collect();
            let mut sam = Sam::new(
                &models.join(ModelId::SamEncoder.spec().file),
                &models.join(ModelId::SamDecoder.spec().file),
            );
            let t = Instant::now();
            let mut lama = Inpaint::load(&models.join(ModelId::Lama.spec().file)).unwrap();
            println!("LaMa: loaded in {:.1} s", t.elapsed().as_secs_f64());
            for photo in &a[5..] {
                let img = load_jpeg(Path::new(photo));
                let stem = Path::new(photo)
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                println!("\n{photo} ({}×{})", img.width(), img.height());
                let t = Instant::now();
                let embedding = sam.embed(&img).unwrap();
                println!("  MobileSAM embed {:.2} s", t.elapsed().as_secs_f64());
                for (name, plan) in &yolox {
                    let t = Instant::now();
                    let w = whole(plan, &img);
                    let tw = t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let ti = tiled(plan, &img);
                    let tt = t.elapsed().as_secs_f64();
                    let people = merged(&w, &ti);
                    let extra = people.iter().filter(|p| p.tiled_only).count();
                    let t = Instant::now();
                    let masks: Vec<ProbMap> = people
                        .iter()
                        .map(|p| sam.prompt(&embedding, &[], Some(p.b)).unwrap())
                        .collect();
                    let ts = t.elapsed().as_secs_f64();
                    println!(
                        "  {name}: whole frame {} people in {:.0} ms · tiled {} more in {:.0} ms · {} outlines in {:.2} s",
                        w.len(),
                        tw * 1000.0,
                        extra,
                        tt * 1000.0,
                        masks.len(),
                        ts
                    );
                    for (i, p) in people.iter().enumerate() {
                        println!(
                            "    [{i}] score {:.2}{}  box {:.3} {:.3} {:.3} {:.3}  ({:.1} % of the frame's height)",
                            p.score,
                            if p.tiled_only { " (tiled)" } else { "" },
                            p.b[0],
                            p.b[1],
                            p.b[2],
                            p.b[3],
                            100.0 * (p.b[3] - p.b[1])
                        );
                    }
                    let mut work = resize_long(&img, WORK_LONG);
                    let before = draw(&work, &people, &masks);
                    let (gone, gone_masks): (Vec<Person>, Vec<ProbMap>) = people
                        .iter()
                        .zip(masks.iter().cloned())
                        .filter(|(p, _)| bystander(p))
                        .map(|(p, m)| (*p, m))
                        .unzip();
                    let times = remove(&mut work, &gone, &gone_masks, &mut lama);
                    println!(
                        "  LaMa: {} removals, {:.1} s ({:.2} s each)",
                        times.len(),
                        times.iter().sum::<f64>(),
                        times.iter().sum::<f64>() / times.len().max(1) as f64
                    );
                    let (l, r) = (resize_long(&before, 1600), resize_long(&work, 1600));
                    let mut sheet = RgbImage::new(l.width() * 2 + 8, l.height());
                    imageops::replace(&mut sheet, &l, 0, 0);
                    imageops::replace(&mut sheet, &r, (l.width() + 8) as i64, 0);
                    sheet.save(out.join(format!("{stem}_{name}.png"))).unwrap();
                    people_sheet(&before, &work, &people)
                        .save(out.join(format!("{stem}_{name}_people.png")))
                        .unwrap();
                }
            }
        }
        _ => eprintln!("people_probe scan YOLOX DIR… | sheet OUT MODELS YOLOX[,YOLOX…] JPG…"),
    }
}
