//! M10.16c4's spike — **throwaway**: can a reflection-removal network run here well enough to be worth building?
//! Nothing here ships; a build would be its own plan, test first.
//!
//! `cargo run --release -p ai-mask --example reflection_probe -- contact OUT DIR…` — every JPEG in the folders as
//! numbered contact sheets (`OUT/sheet_NNN.png`, `OUT/sheet_NNN.txt` naming each cell), to find photos with glass.
//!
//! `… -- remove OUT KIND MODEL SIDE JPG…` — each photo through a reflection-removal network (`KIND` `pinto`: a
//! squashed 640² square; `dsr`: the photo's own aspect at `SIDE` long, both sides to a multiple of 32), the removed
//! layer (input − output) scaled up and subtracted from the photo at `WORK` long, so it keeps its own detail.
//! `OUT/<photo>_<kind>.png`: the original, the result and the removed layer (×3); `OUT/<photo>_<kind>_crop.png`: a
//! 1:1 crop where the most was removed, before and after. Times printed.

use image::imageops::{self, FilterType};
use image::{Rgb, Rgb32FImage, RgbImage};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tract_onnx::prelude::*;

/// The long side the result is made at.
const WORK: u32 = 2048;
/// Each panel of a sheet, long side.
const PANEL: u32 = 720;
/// The 1:1 crop's side, at `WORK`.
const CROP: u32 = 512;

const COLS: u32 = 6;
const ROWS: u32 = 5;
const CELL: u32 = 260;

fn load_jpeg(path: &Path) -> Option<RgbImage> {
    let mut dec = image::ImageReader::open(path).ok()?.into_decoder().ok()?;
    let o = image::ImageDecoder::orientation(&mut dec).ok()?;
    let mut img = image::DynamicImage::from_decoder(dec).ok()?;
    img.apply_orientation(o);
    Some(img.to_rgb8())
}

fn jpegs(dirs: &[String]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).unwrap().flatten().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
        })
        .collect();
    out.sort();
    out
}

fn contact(out: &Path, dirs: &[String]) {
    std::fs::create_dir_all(out).unwrap();
    let files = jpegs(dirs);
    let per = (COLS * ROWS) as usize;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let thumbs: Vec<Option<RgbImage>> = {
        let mut slots: Vec<Option<RgbImage>> = vec![None; files.len()];
        let chunk = files.len().div_ceil(threads).max(1);
        std::thread::scope(|sc| {
            for (fs, ss) in files.chunks(chunk).zip(slots.chunks_mut(chunk)) {
                sc.spawn(move || {
                    for (f, s) in fs.iter().zip(ss.iter_mut()) {
                        *s = load_jpeg(f).map(|i| {
                            let (w, h) = (i.width(), i.height());
                            let k = CELL as f32 / w.max(h) as f32;
                            imageops::resize(
                                &i,
                                ((w as f32 * k) as u32).max(1),
                                ((h as f32 * k) as u32).max(1),
                                FilterType::Triangle,
                            )
                        });
                    }
                });
            }
        });
        slots
    };
    for (n, (fs, ts)) in files.chunks(per).zip(thumbs.chunks(per)).enumerate() {
        let mut sheet = RgbImage::from_pixel(COLS * CELL, ROWS * CELL, Rgb([20, 20, 20]));
        let mut names = String::new();
        for (i, (f, t)) in fs.iter().zip(ts).enumerate() {
            let (c, r) = (i as u32 % COLS, i as u32 / COLS);
            if let Some(t) = t {
                let x = c * CELL + (CELL - t.width()) / 2;
                let y = r * CELL + (CELL - t.height()) / 2;
                imageops::overlay(&mut sheet, t, x as i64, y as i64);
            }
            names.push_str(&format!("r{} c{} {}\n", r + 1, c + 1, f.display()));
        }
        sheet.save(out.join(format!("sheet_{n:03}.png"))).unwrap();
        std::fs::write(out.join(format!("sheet_{n:03}.txt")), names).unwrap();
    }
    println!(
        "{} photos on {} sheets",
        files.len(),
        files.len().div_ceil(per)
    );
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

/// The network's input size for a `w × h` photo.
fn net_size(kind: &str, side: u32, w: u32, h: u32) -> (u32, u32) {
    match kind {
        "pinto" => (640, 640),
        _ => {
            let r = side as f32 / w.max(h) as f32;
            let to32 = |v: u32| (((v as f32 * r) / 32.0).round() as u32).max(2) * 32;
            (to32(w), to32(h))
        }
    }
}

fn to_f32(img: &RgbImage) -> Rgb32FImage {
    Rgb32FImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        Rgb([
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
        ])
    })
}

fn to_u8(img: &Rgb32FImage) -> RgbImage {
    RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        Rgb(p.0.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8))
    })
}

/// `img` (0…1) as the NCHW tensor both networks take.
fn tensor_of(img: &Rgb32FImage) -> Vec<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let n = w * h;
    let mut v = vec![0f32; 3 * n];
    for (x, y, p) in img.enumerate_pixels() {
        let i = y as usize * w + x as usize;
        for c in 0..3 {
            v[c * n + i] = p[c];
        }
    }
    v
}

fn image_of(v: &[f32], w: u32, h: u32) -> Rgb32FImage {
    let n = (w * h) as usize;
    Rgb32FImage::from_fn(w, h, |x, y| {
        let i = (y * w + x) as usize;
        Rgb([v[i], v[n + i], v[2 * n + i]])
    })
}

fn write_f32(path: &Path, v: &[f32]) {
    let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    std::fs::write(path, bytes).unwrap();
}

/// The panel of a sheet: `img` at `PANEL` long.
fn panel(img: &RgbImage) -> RgbImage {
    resize_long(img, PANEL)
}

fn row(imgs: &[RgbImage]) -> RgbImage {
    let gap = 8;
    let w = imgs.iter().map(|i| i.width()).sum::<u32>() + gap * (imgs.len() as u32 - 1);
    let h = imgs.iter().map(|i| i.height()).max().unwrap();
    let mut out = RgbImage::from_pixel(w, h, Rgb([20, 20, 20]));
    let mut x = 0;
    for i in imgs {
        imageops::overlay(&mut out, i, x as i64, 0);
        x += i.width() + gap;
    }
    out
}

fn remove(out: &Path, kind: &str, model: &Path, side: u32, photos: &[String]) {
    std::fs::create_dir_all(out).unwrap();
    let dump = std::env::var_os("REFL_DUMP").map(PathBuf::from);
    let t0 = Instant::now();
    // ⚠ The DSRNet export is dynamic (`32·h32` × `32·w32`): its symbols are BOUND per size and its shape
    // annotations ignored — the depth network's recipe (`ai_mask::depth`).
    let base = tract_onnx::onnx()
        .with_ignore_value_info(true)
        .with_ignore_output_shapes(true)
        .model_for_path(model)
        .unwrap();
    let typed = (kind == "dsr").then(|| base.clone().into_typed().unwrap());
    println!("{kind}: model read in {:.1} s", t0.elapsed().as_secs_f32());
    let mut plans: Vec<((u32, u32), std::sync::Arc<TypedRunnableModel>)> = Vec::new();
    for photo in photos {
        let path = Path::new(photo);
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let full = load_jpeg(path).unwrap();
        let work = to_f32(&resize_long(&full, WORK));
        let (nw, nh) = net_size(kind, side, work.width(), work.height());
        let plan = match plans.iter().find(|(s, _)| *s == (nw, nh)) {
            Some((_, p)) => p.clone(),
            None => {
                let t = Instant::now();
                let p = match &typed {
                    Some(m) => {
                        let subs: std::collections::HashMap<Symbol, TDim> = [
                            (m.sym("h32"), ((nh / 32) as usize).to_dim()),
                            (m.sym("w32"), ((nw / 32) as usize).to_dim()),
                        ]
                        .into_iter()
                        .collect();
                        m.set_symbols(&subs)
                            .unwrap()
                            .into_optimized()
                            .unwrap()
                            .into_runnable()
                            .unwrap()
                    }
                    None => base
                        .clone()
                        .with_input_fact(0, f32::fact([1, 3, nh as usize, nw as usize]).into())
                        .unwrap()
                        .into_optimized()
                        .unwrap()
                        .into_runnable()
                        .unwrap(),
                };
                println!("  plan for {nw}×{nh} in {:.1} s", t.elapsed().as_secs_f32());
                plans.push(((nw, nh), p.clone()));
                p
            }
        };
        let input_img = imageops::resize(&work, nw, nh, FilterType::Triangle);
        let input = tensor_of(&input_img);
        let t = Instant::now();
        let res = plan
            .run(tvec!(Tensor::from_shape(
                &[1, 3, nh as usize, nw as usize],
                &input
            )
            .unwrap()
            .into()))
            .unwrap();
        let ran = t.elapsed().as_secs_f32();
        let tr = res[0].to_plain_array_view::<f32>().unwrap();
        let tr = tr.as_slice().unwrap();
        let bad = tr.iter().filter(|v| !v.is_finite()).count();
        if let Some(d) = &dump {
            std::fs::create_dir_all(d).unwrap();
            write_f32(&d.join(format!("{stem}_{kind}_{nw}x{nh}_in.f32")), &input);
            write_f32(&d.join(format!("{stem}_{kind}_{nw}x{nh}_t.f32")), tr);
        }
        // The removed layer at the network's size, scaled up and taken from the photo at `WORK`.
        let t_net = image_of(tr, nw, nh);
        let r_net = Rgb32FImage::from_fn(nw, nh, |x, y| {
            let (a, b) = (input_img.get_pixel(x, y), t_net.get_pixel(x, y));
            Rgb([a[0] - b[0], a[1] - b[1], a[2] - b[2]])
        });
        let r_up = imageops::resize(&r_net, work.width(), work.height(), FilterType::Triangle);
        let result = Rgb32FImage::from_fn(work.width(), work.height(), |x, y| {
            let (a, r) = (work.get_pixel(x, y), r_up.get_pixel(x, y));
            Rgb([a[0] - r[0], a[1] - r[1], a[2] - r[2]])
        });
        let layer = Rgb32FImage::from_fn(work.width(), work.height(), |x, y| {
            Rgb(r_up.get_pixel(x, y).0.map(|v| (v * 3.0).abs()))
        });
        let mean_r: f32 = r_up
            .pixels()
            .map(|p| (p[0] + p[1] + p[2]).abs() / 3.0)
            .sum::<f32>()
            / (work.width() * work.height()) as f32;
        println!(
            "  {stem}: net {nw}×{nh} in {ran:.2} s, non-finite {bad}, mean |removed| {:.4}",
            mean_r
        );
        let (o8, r8, l8) = (to_u8(&work), to_u8(&result), to_u8(&layer));
        row(&[panel(&o8), panel(&r8), panel(&l8)])
            .save(out.join(format!("{stem}_{kind}.png")))
            .unwrap();
        // The 1:1 crop where the most was removed, on a coarse grid.
        let (cw, ch) = (CROP.min(work.width()), CROP.min(work.height()));
        let mut best = (0, 0, -1f32);
        for y in (0..=work.height() - ch).step_by(64) {
            for x in (0..=work.width() - cw).step_by(64) {
                let mut s = 0f32;
                for yy in (y..y + ch).step_by(8) {
                    for xx in (x..x + cw).step_by(8) {
                        let p = r_up.get_pixel(xx, yy);
                        s += (p[0] + p[1] + p[2]).abs();
                    }
                }
                if s > best.2 {
                    best = (x, y, s);
                }
            }
        }
        let crop = |i: &RgbImage| imageops::crop_imm(i, best.0, best.1, cw, ch).to_image();
        row(&[crop(&o8), crop(&r8)])
            .save(out.join(format!("{stem}_{kind}_crop.png")))
            .unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("contact") => contact(Path::new(&args[1]), &args[2..]),
        Some("remove") => remove(
            Path::new(&args[1]),
            &args[2],
            Path::new(&args[3]),
            args[4].parse().unwrap(),
            &args[5..],
        ),
        Some("stack") => {
            let imgs: Vec<RgbImage> = args[2..]
                .iter()
                .map(|p| image::open(p).unwrap().to_rgb8())
                .collect();
            let gap = 12;
            let w = imgs.iter().map(|i| i.width()).max().unwrap();
            let h = imgs.iter().map(|i| i.height()).sum::<u32>() + gap * (imgs.len() as u32 - 1);
            let mut out = RgbImage::from_pixel(w, h, Rgb([20, 20, 20]));
            let mut y = 0;
            for i in &imgs {
                imageops::overlay(&mut out, i, 0, y as i64);
                y += i.height() + gap;
            }
            out.save(&args[1]).unwrap();
        }
        _ => eprintln!("usage: reflection_probe contact OUT DIR… | remove OUT KIND MODEL SIDE JPG… | stack OUT PNG…"),
    }
}
