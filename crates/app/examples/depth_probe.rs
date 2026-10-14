//! M10.16a's spike, kept: Depth Anything V2 Small through `tract` on real photos — the time to build a
//! size's plan, the time to run it, and the map as a PNG (near white) beside a copy of the photo.
//!
//! ```text
//! cargo run --release --example depth_probe -- <out dir> <long side> <photo>…
//! cargo run --release --example depth_probe -- <out dir> sheet <photo>…
//! cargo run --release --example depth_probe -- <out dir> edges <photo>…
//! ```
//! `edges` (the edge spike, 2026-10-14, after the author saw the edges bleed) writes `<stem>_edges.png`:
//! five recipes side by side, zoomed on the photo's strongest depth edge — A today's (770, luminance-guided r8 eps 1e-3),
//! B colour-guided, C luminance eps 1e-4, D inference at 1036, E 1036 + colour — on top the photo with a
//! cyan overlay where a HARD split (one Otsu threshold, from A, for all five) calls it background, so a
//! bleed is cyan crossing an outline; below, the depth map itself.
//! `sheet` (Task 11) writes `<stem>_sheet.png` per photo, for the author's choices — row A the scale
//! (the photo, linear disparity, equalised), row B the edges (a crop of the photo's centre, then the
//! map there unsnapped, snapped at radius 8 and 16), row C the stored size (1024, 1536, 2048, each
//! snapped at a radius in proportion) — all from one inference at `depth_map::RECIPE.infer_long`.
//! One long side per process, so `/usr/bin/time -l` reports that size's peak memory. The photos are
//! read upright (their EXIF orientation applied), as the app shows the network every photo.

use image::ImageDecoder;

fn upright(path: &str) -> image::RgbImage {
    let mut dec = image::ImageReader::open(path)
        .expect("open")
        .into_decoder()
        .expect("decoder");
    let o = dec.orientation().expect("orientation");
    let mut img = image::DynamicImage::from_decoder(dec).expect("decode");
    img.apply_orientation(o);
    img.to_rgb8()
}

/// The map as a grey image, near white.
fn grey(m: &ai_mask::ProbMap) -> image::RgbImage {
    image::RgbImage::from_fn(m.w, m.h, |x, y| {
        let g = 255 - m.data[(y * m.w + x) as usize];
        image::Rgb([g, g, g])
    })
}

/// `img`'s central crop — a third of each side — scaled to `side` wide.
fn centre_crop(img: &image::RgbImage, side: u32) -> image::RgbImage {
    let (w, h) = (img.width() / 3, img.height() / 3);
    let crop = image::imageops::crop_imm(img, w, h, w, h).to_image();
    let tall = (side as f32 * h as f32 / w as f32).round() as u32;
    image::imageops::resize(&crop, side, tall, image::imageops::FilterType::Nearest)
}

fn fit(img: &image::RgbImage, side: u32) -> image::RgbImage {
    let k = side as f32 / img.width().max(img.height()) as f32;
    let (w, h) = (
        (img.width() as f32 * k) as u32,
        (img.height() as f32 * k) as u32,
    );
    image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle)
}

fn sheet(out: &std::path::Path, photos: &[String], depth: &mut ai_mask::depth::Depth) {
    use ai_mask::depth::DepthScale;
    use image::GenericImage;
    let r = rustroom_app::depth_map::RECIPE;
    for photo in photos {
        let img = upright(photo);
        let at = |long: u32| fit(&img, long);
        let raw = depth.run(&at(r.store_long), r.infer_long).expect("run");
        let finish = |scale, store: u32, radius| {
            let pos = ai_mask::depth::normalise(&raw, scale);
            let recipe = rustroom_app::depth_map::Recipe {
                store_long: store,
                radius,
                scale,
                ..r
            };
            grey(&rustroom_app::depth_map::finish(&pos, &at(store), &recipe))
        };
        let cell = 420u32;
        let row_a = [
            fit(&img, cell),
            fit(&finish(DepthScale::Linear, 1536, 8), cell),
            fit(&finish(DepthScale::Equalised, 1536, 8), cell),
        ];
        let row_b = [
            centre_crop(&at(1536), cell),
            centre_crop(&finish(r.scale, 1536, 0), cell),
            centre_crop(&finish(r.scale, 1536, 8), cell),
            centre_crop(&finish(r.scale, 1536, 16), cell),
        ];
        let row_c = [
            centre_crop(&finish(r.scale, 1024, 5), cell),
            centre_crop(&finish(r.scale, 1536, 8), cell),
            centre_crop(&finish(r.scale, 2048, 11), cell),
        ];
        let rows = [&row_a[..], &row_b[..], &row_c[..]];
        let height: u32 = rows
            .iter()
            .map(|r| r.iter().map(|i| i.height()).max().unwrap_or(0) + 8)
            .sum();
        let mut sheet =
            image::RgbImage::from_pixel(cell * 4 + 24, height, image::Rgb([40, 40, 40]));
        let mut y = 0;
        for row in rows {
            for (i, tile) in row.iter().enumerate() {
                sheet
                    .copy_from(tile, i as u32 * (cell + 8), y)
                    .expect("fits");
            }
            y += row.iter().map(|i| i.height()).max().unwrap_or(0) + 8;
        }
        let stem = std::path::Path::new(photo)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        sheet
            .save(out.join(format!("{stem}_sheet.png")))
            .expect("save");
        println!("{photo}: sheet written");
    }
}

/// The guided filter with a COLOUR guide (He et al. 2010, §3.5): a local linear model of `p` in the
/// guide's three channels, so equal brightness of different colours still separates. Spike code.
fn colour_guided(p: &[f32], img: &image::RgbImage, r: usize, eps: f32) -> Vec<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let ch: Vec<Vec<f32>> = (0..3)
        .map(|c| img.pixels().map(|px| px[c] as f32 / 255.0).collect())
        .collect();
    let bx = |v: &[f32]| pipeline::spatial::box_blur(v, w, h, r);
    let prod = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).collect::<Vec<f32>>();
    let mi: Vec<Vec<f32>> = ch.iter().map(|c| bx(c)).collect();
    let mp = bx(p);
    let mip: Vec<Vec<f32>> = ch.iter().map(|c| bx(&prod(c, p))).collect();
    let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let mii: Vec<Vec<f32>> = pairs
        .iter()
        .map(|&(i, j)| bx(&prod(&ch[i], &ch[j])))
        .collect();
    let n = w * h;
    let (mut a, mut b) = (vec![[0f32; 3]; n], vec![0f32; n]);
    for k in 0..n {
        let m = [mi[0][k], mi[1][k], mi[2][k]];
        let s = |q: usize, i: usize, j: usize| mii[q][k] - m[i] * m[j];
        let (rr, rg, rb, gg, gb, bb) = (
            s(0, 0, 0) + eps,
            s(1, 0, 1),
            s(2, 0, 2),
            s(3, 1, 1) + eps,
            s(4, 1, 2),
            s(5, 2, 2) + eps,
        );
        // The inverse of the symmetric 3 × 3 covariance, by its adjugate.
        let inv = [
            gg * bb - gb * gb,
            gb * rb - rg * bb,
            rg * gb - gg * rb,
            rr * bb - rb * rb,
            rb * rg - rr * gb,
            rr * gg - rg * rg,
        ];
        let det = rr * inv[0] + rg * inv[1] + rb * inv[2];
        let cov = [
            mip[0][k] - m[0] * mp[k],
            mip[1][k] - m[1] * mp[k],
            mip[2][k] - m[2] * mp[k],
        ];
        let ak = [
            (inv[0] * cov[0] + inv[1] * cov[1] + inv[2] * cov[2]) / det,
            (inv[1] * cov[0] + inv[3] * cov[1] + inv[4] * cov[2]) / det,
            (inv[2] * cov[0] + inv[4] * cov[1] + inv[5] * cov[2]) / det,
        ];
        b[k] = mp[k] - ak[0] * m[0] - ak[1] * m[1] - ak[2] * m[2];
        a[k] = ak;
    }
    let ma: Vec<Vec<f32>> = (0..3)
        .map(|c| bx(&a.iter().map(|v| v[c]).collect::<Vec<f32>>()))
        .collect();
    let mb = bx(&b);
    (0..n)
        .map(|k| {
            (ma[0][k] * ch[0][k] + ma[1][k] * ch[1][k] + ma[2][k] * ch[2][k] + mb[k])
                .clamp(0.0, 1.0)
        })
        .collect()
}

/// Otsu's threshold over `v` (0..1): the split maximising the between-class variance.
fn otsu(v: &[f32]) -> f32 {
    let mut hist = [0f64; 256];
    for &x in v {
        hist[(x.clamp(0.0, 1.0) * 255.0) as usize] += 1.0;
    }
    let total: f64 = hist.iter().sum();
    let sum: f64 = hist.iter().enumerate().map(|(i, h)| i as f64 * h).sum();
    let (mut w0, mut s0, mut best, mut at) = (0.0, 0.0, -1.0, 128usize);
    for (i, h) in hist.iter().enumerate() {
        w0 += h;
        s0 += i as f64 * h;
        let w1 = total - w0;
        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }
        let between = w0 * w1 * (s0 / w0 - (sum - s0) / w1).powi(2);
        if between > best {
            best = between;
            at = i;
        }
    }
    (at as f32 + 0.5) / 255.0
}

fn edges(out: &std::path::Path, photos: &[String], depth: &mut ai_mask::depth::Depth) {
    use image::GenericImage;
    let r = rustroom_app::depth_map::RECIPE;
    for photo in photos {
        let img = upright(photo);
        let guide = fit(&img, r.store_long);
        let (gw, gh) = (guide.width() as usize, guide.height() as usize);
        let up = |raw: &ai_mask::depth::Plane| {
            let pos = ai_mask::depth::normalise(raw, r.scale);
            // Unsnapped, at the guide's size — `finish` with radius 0 is exactly the upsample.
            let m = rustroom_app::depth_map::finish(
                &pos,
                &guide,
                &rustroom_app::depth_map::Recipe { radius: 0, ..r },
            );
            m.data
                .iter()
                .map(|&v| v as f32 / 255.0)
                .collect::<Vec<f32>>()
        };
        let t = std::time::Instant::now();
        let p770 = up(&depth.run(&guide, 770).expect("run"));
        let t770 = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let p1036 = up(&depth.run(&guide, 1036).expect("run"));
        let t1036 = t.elapsed().as_secs_f64();
        let luma: Vec<f32> = guide
            .pixels()
            .map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0)
            .collect();
        let lum = |p: &[f32], eps| pipeline::spatial::guided_filter(p, &luma, gw, gh, 8, eps);
        let t = std::time::Instant::now();
        let colour_770 = colour_guided(&p770, &guide, 8, 1e-3);
        let t_colour = t.elapsed().as_secs_f64();
        let variants: [(&str, Vec<f32>); 5] = [
            ("A", lum(&p770, 1e-3)),
            ("B", colour_770),
            ("C", lum(&p770, 1e-4)),
            ("D", lum(&p1036, 1e-3)),
            ("E", colour_guided(&p1036, &guide, 8, 1e-3)),
        ];
        println!(
            "{photo}: infer 770 {t770:.2} s, 1036 {t1036:.2} s; colour filter {t_colour:.2} s"
        );
        // The crop: a sixth of the frame centred on the strongest depth edge in A — the subject's
        // outline, where bleeding shows — searched in the middle two thirds; the split: Otsu there.
        let (cw, ch) = (gw / 6, gh / 6);
        let grad = {
            let a = &variants[0].1;
            let g: Vec<f32> = (0..gw * gh)
                .map(|k| {
                    let (x, y) = (k % gw, k / gw);
                    let dx = a[y * gw + (x + 1).min(gw - 1)] - a[y * gw + x.saturating_sub(1)];
                    let dy = a[(y + 1).min(gh - 1) * gw + x] - a[y.saturating_sub(1) * gw + x];
                    (dx * dx + dy * dy).sqrt()
                })
                .collect();
            pipeline::spatial::box_blur(&g, gw, gh, 12)
        };
        let (mut best, mut at) = (-1.0f32, (gw / 2, gh / 2));
        for y in gh / 6..gh * 5 / 6 {
            for x in gw / 6..gw * 5 / 6 {
                if grad[y * gw + x] > best {
                    best = grad[y * gw + x];
                    at = (x, y);
                }
            }
        }
        let cx = at.0.saturating_sub(cw / 2).min(gw - cw);
        let cy = at.1.saturating_sub(ch / 2).min(gh - ch);
        let crop = |v: &[f32]| -> Vec<f32> {
            (cy..cy + ch)
                .flat_map(|y| (cx..cx + cw).map(move |x| (x, y)))
                .map(|(x, y)| v[y * gw + x])
                .collect()
        };
        let split = otsu(&crop(&variants[0].1));
        let tile = 340u32;
        let scale = |im: image::RgbImage| {
            let th = (tile as f32 * im.height() as f32 / im.width() as f32).round() as u32;
            image::imageops::resize(&im, tile, th, image::imageops::FilterType::Nearest)
        };
        let photo_crop =
            image::imageops::crop_imm(&guide, cx as u32, cy as u32, cw as u32, ch as u32)
                .to_image();
        let th = (tile as f32 * ch as f32 / cw as f32).round() as u32;
        let mut sheet =
            image::RgbImage::from_pixel(5 * (tile + 6), 2 * (th + 6), image::Rgb([40, 40, 40]));
        // The halo: pixels whose depth sits in the middle 60 % between the crop's two Otsu classes
        // (A's split) — neither subject nor background, i.e. a bleeding edge. Fewer is tighter.
        let halo = |c: &[f32]| {
            let (lo, hi): (Vec<f32>, Vec<f32>) = c.iter().partition(|&&v| v <= split);
            let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
            let (a, b) = (mean(&lo), mean(&hi));
            let (from, to) = (a + 0.2 * (b - a), a + 0.8 * (b - a));
            c.iter().filter(|&&v| v > from && v < to).count()
        };
        let counts: Vec<String> = variants
            .iter()
            .map(|(name, v)| format!("{name} {}", halo(&crop(v))))
            .collect();
        println!("  halo pixels at the edge: {}", counts.join(", "));
        for (i, (_, v)) in variants.iter().enumerate() {
            let c = crop(v);
            let over = image::RgbImage::from_fn(cw as u32, ch as u32, |x, y| {
                let px = photo_crop.get_pixel(x, y);
                if c[y as usize * cw + x as usize] > split {
                    // Cyan — a red overlay vanished on the portrait's red wall.
                    image::Rgb([
                        px[0] / 3,
                        (px[1] as u16 / 3 + 150) as u8,
                        (px[2] as u16 / 3 + 170) as u8,
                    ])
                } else {
                    *px
                }
            });
            let grey_tile = image::RgbImage::from_fn(cw as u32, ch as u32, |x, y| {
                let g = 255 - (c[y as usize * cw + x as usize] * 255.0) as u8;
                image::Rgb([g, g, g])
            });
            sheet
                .copy_from(&scale(over), i as u32 * (tile + 6), 0)
                .expect("fits");
            sheet
                .copy_from(&scale(grey_tile), i as u32 * (tile + 6), th + 6)
                .expect("fits");
        }
        let stem = std::path::Path::new(photo)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        sheet
            .save(out.join(format!("{stem}_edges.png")))
            .expect("save");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(2).map(String::as_str) == Some("edges") {
        let models = rustroom_app::ai_cache::models_dir().expect("models dir");
        let file = models.join(ai_mask::ModelId::DepthAnythingV2Small.spec().file);
        let mut depth = ai_mask::depth::Depth::load(&file).expect("load");
        return edges(std::path::Path::new(&args[1]), &args[3..], &mut depth);
    }
    if args.get(2).map(String::as_str) == Some("sheet") {
        let models = rustroom_app::ai_cache::models_dir().expect("models dir");
        let file = models.join(ai_mask::ModelId::DepthAnythingV2Small.spec().file);
        let mut depth = ai_mask::depth::Depth::load(&file).expect("load");
        return sheet(std::path::Path::new(&args[1]), &args[3..], &mut depth);
    }
    let (out, long) = (
        std::path::PathBuf::from(&args[1]),
        args[2].parse::<u32>().expect("long"),
    );
    let models = rustroom_app::ai_cache::models_dir().expect("models dir");
    let file = models.join(ai_mask::ModelId::DepthAnythingV2Small.spec().file);
    let t = std::time::Instant::now();
    let mut depth = ai_mask::depth::Depth::load(&file).expect("load");
    println!("load {:.2} s", t.elapsed().as_secs_f64());
    for photo in &args[3..] {
        let img = upright(photo);
        // The first run builds the size's plan; the second is the steady state.
        let t = std::time::Instant::now();
        let raw = depth.run(&img, long).expect("run");
        let first = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let _ = depth.run(&img, long).expect("run");
        let second = t.elapsed().as_secs_f64();
        println!(
            "{photo}: {}×{} first {first:.2} s, again {second:.2} s",
            raw.w, raw.h
        );
        let p = ai_mask::depth::normalise(&raw, ai_mask::depth::DepthScale::Linear);
        let stem = std::path::Path::new(photo)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        image::GrayImage::from_fn(p.w, p.h, |x, y| {
            image::Luma([((1.0 - p.data[(y * p.w + x) as usize]) * 255.0).round() as u8])
        })
        .save(out.join(format!("{stem}_depth_{long}.png")))
        .expect("save");
        image::imageops::resize(&img, p.w, p.h, image::imageops::FilterType::Triangle)
            .save(out.join(format!("{stem}_{long}.png")))
            .expect("save");
    }
}
