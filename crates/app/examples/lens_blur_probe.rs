//! M10.16d's spike, kept: Lens Blur's CPU reference on real portraits — its time at the preview size and
//! at full resolution, and a sheet of the look (the original; Subject focus at Amount 50 with Boost 0 and
//! 50; Cat-eye; a zoomed crop at the subject's outline for each — the halo test).
//!
//! ```text
//! cargo run --release --example lens_blur_probe -- <out dir> <photo>…
//! cargo run --release --example lens_blur_probe -- gpu <photo>…
//! ```
//! `gpu` (M10.16d2): the GPU twin's frame at 1600 px — the planes through the app's real path
//! (`AiSource::plane`), the twin's construction, five warm frames with GPU timestamps, and its parity
//! with the CPU reference on the real photograph.
//! Depth and Subject are computed as the app does (`depth_map::RECIPE`; IS-Net), the photo read upright.

use image::{GenericImage, ImageDecoder};

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

fn fit(img: &image::RgbImage, long: u32) -> image::RgbImage {
    let k = long as f32 / img.width().max(img.height()) as f32;
    let (w, h) = (
        (img.width() as f32 * k).round() as u32,
        (img.height() as f32 * k).round() as u32,
    );
    image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle)
}

/// The 8-bit photo as linear light, the planes sampled from the decode-space maps.
fn frame(
    img: &image::RgbImage,
    depth: &ai_mask::ProbMap,
    subject: &ai_mask::ProbMap,
) -> (core_image::ImageBuf, Vec<f32>, Vec<f32>) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut buf = core_image::ImageBuf::new(w, h);
    let (mut d, mut s) = (vec![0.0; w * h], vec![0.0; w * h]);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x as u32, y as u32);
            buf.set_pixel(
                x,
                y,
                [0, 1, 2].map(|c| color::srgb_to_linear(p[c] as f32 / 255.0)),
            );
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            d[y * w + x] = depth.at(u, v);
            s[y * w + x] = rustroom_app::ai_coverage::refine(subject.at(u, v), 0.0, false);
        }
    }
    (buf, d, s)
}

fn to_rgb8(buf: &core_image::ImageBuf) -> image::RgbImage {
    image::RgbImage::from_fn(buf.width() as u32, buf.height() as u32, |x, y| {
        let p = buf.pixel(x as usize, y as usize);
        image::Rgb(p.map(|c| (color::linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8))
    })
}

/// The focus: the median depth under the subject (Task 6's rule, inline here).
fn focus(depth: &[f32], subject: &[f32]) -> f32 {
    let mut under: Vec<f32> = depth
        .iter()
        .zip(subject)
        .filter(|(_, &s)| s > 0.5)
        .map(|(&d, _)| d)
        .collect();
    if under.is_empty() {
        return 0.5;
    }
    under.sort_by(f32::total_cmp);
    under[under.len() / 2]
}

/// The 8-bit photo as linear light.
fn linear(img: &image::RgbImage) -> core_image::ImageBuf {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut buf = core_image::ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x as u32, y as u32);
            buf.set_pixel(
                x,
                y,
                [0, 1, 2].map(|c| color::srgb_to_linear(p[c] as f32 / 255.0)),
            );
        }
    }
    buf
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

fn gpu_mode(photos: &[String]) {
    use pipeline::PipelineStage;
    use rustroom_app::ai_coverage::{AiMaps, AiSource, MapQuery};
    let Some(ctx) = gpu_pipeline::shared().cloned() else {
        eprintln!("no GPU adapter");
        return;
    };
    let models = rustroom_app::ai_cache::models_dir().expect("models dir");
    let mut depth_net = ai_mask::depth::Depth::load(
        &models.join(ai_mask::ModelId::DepthAnythingV2Small.spec().file),
    )
    .expect("depth model");
    let subject_net = ai_mask::Subject::load(
        &models.join(ai_mask::ModelId::IsNet.spec().file),
        ai_mask::SubjectModel::IsNet,
    )
    .expect("subject model");
    let r = rustroom_app::depth_map::RECIPE;
    for photo in photos {
        let img = upright(photo);
        let guide = fit(&img, r.store_long);
        let raw = depth_net.run(&guide, r.infer_long).expect("depth");
        let depth =
            rustroom_app::depth_map::finish(&ai_mask::depth::normalise(&raw, r.scale), &guide, &r);
        let subject = subject_net.run(&fit(&img, 1024)).expect("subject");
        let mut maps = AiMaps::default();
        maps.insert(MapQuery::Depth, std::sync::Arc::new(depth));
        maps.insert(
            MapQuery::Prompt(edit_model::AiPrompt::Subject),
            std::sync::Arc::new(subject),
        );
        let edits = edit_model::Edits {
            lens_blur: edit_model::LensBlur {
                on: true,
                ..edit_model::LensBlur::default()
            },
            ..Default::default()
        };
        let lb = edits.lens_blur;
        let buf = linear(&fit(&img, 1600));
        let (w, h) = (buf.width(), buf.height());
        let src = AiSource {
            maps: &maps,
            edits: &edits,
            lens: None,
            decode: (img.width() as usize, img.height() as usize),
        };
        let t = std::time::Instant::now();
        let i = pipeline::lens_blur::inputs(&lb, &src).expect("inputs");
        let t_inputs = ms(t.elapsed());
        let t = std::time::Instant::now();
        let _ = pipeline::lens_blur::plane_bytes(&*i.depth.0, w, h);
        let t_depth = ms(t.elapsed());
        let t = std::time::Instant::now();
        let _ = pipeline::lens_blur::plane_bytes(&*i.subject.as_ref().expect("subject").0, w, h);
        let t_subject = ms(t.elapsed());
        let t = std::time::Instant::now();
        let _ = gpu_pipeline::GpuLensBlurStage::new(&lb, &i, &buf, &ctx).expect("the twin");
        let t_new = ms(t.elapsed());
        // ⚠ A drag's next frame: inputs and the twin built afresh, the planes from the memo.
        let t = std::time::Instant::now();
        let i = pipeline::lens_blur::inputs(&lb, &src).expect("inputs");
        let stage = gpu_pipeline::GpuLensBlurStage::new(&lb, &i, &buf, &ctx).expect("the twin");
        let t_warm = ms(t.elapsed());
        let mut chain = gpu_pipeline::GpuChain::new(ctx.clone(), vec![Box::new(stage)]);
        chain.set_timing(true);
        let _ = chain.apply(buf.clone()); // compiles the program
        let (mut walls, mut spans) = (Vec::new(), Vec::new());
        let mut gpu = buf.clone();
        for _ in 0..5 {
            let t = std::time::Instant::now();
            gpu = chain.apply(buf.clone());
            walls.push(ms(t.elapsed()));
            if let Some(tm) = chain.last_timings() {
                spans.push(tm.gpu_span_ms);
            }
        }
        let t = std::time::Instant::now();
        let cpu = pipeline::lens_blur::LensBlurStage::new(
            &lb,
            i.focus,
            i.depth.0.clone(),
            i.subject.as_ref().map(|s| s.0.clone()),
        )
        .apply(buf.clone());
        let t_cpu = ms(t.elapsed());
        let d = gpu_pipeline::compare(&cpu, &gpu);
        println!(
            "{photo}: {w}x{h}  inputs {t_inputs:.1} ms (planes cold: depth {t_depth:.1}, subject {t_subject:.1})  \
             twin new {t_new:.1} / {t_warm:.1} ms (planes memoised)  GPU frame {:.1} ms wall / {:.1} ms passes (median of 5)  \
             CPU {t_cpu:.0} ms  parity worst {:.3} levels over {} of {}",
            median(walls),
            median(spans),
            d.worst_levels,
            d.differing,
            d.total
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("gpu") {
        return gpu_mode(&args[2..]);
    }
    let out = std::path::PathBuf::from(&args[1]);
    let models = rustroom_app::ai_cache::models_dir().expect("models dir");
    let mut depth_net = ai_mask::depth::Depth::load(
        &models.join(ai_mask::ModelId::DepthAnythingV2Small.spec().file),
    )
    .expect("depth model");
    let subject_net = ai_mask::Subject::load(
        &models.join(ai_mask::ModelId::IsNet.spec().file),
        ai_mask::SubjectModel::IsNet,
    )
    .expect("subject model");
    let r = rustroom_app::depth_map::RECIPE;
    for photo in &args[2..] {
        let img = upright(photo);
        let guide = fit(&img, r.store_long);
        let raw = depth_net.run(&guide, r.infer_long).expect("depth");
        let depth =
            rustroom_app::depth_map::finish(&ai_mask::depth::normalise(&raw, r.scale), &guide, &r);
        let subject = subject_net.run(&fit(&img, 1024)).expect("subject");
        let lb = |boost: f32, bokeh| edit_model::LensBlur {
            on: true,
            amount: 50.0,
            boost,
            bokeh,
            ..edit_model::LensBlur::default()
        };
        // Timing: the preview size, then full resolution.
        for long in [1600u32, img.width().max(img.height())] {
            let (buf, d, s) = frame(&fit(&img, long), &depth, &subject);
            let f = focus(&d, &s);
            let p = pipeline::lens_blur::Params::new(
                &lb(25.0, edit_model::Bokeh::Circle),
                f,
                long as usize,
            );
            let t = std::time::Instant::now();
            let _ = pipeline::lens_blur::render(&buf, &d, Some(&s), &p);
            println!(
                "{photo}: {long} px — render {:.2} s (radius {:.0} px, focus {f:.2})",
                t.elapsed().as_secs_f64(),
                p.radius_px
            );
        }
        // The sheet, at the preview size.
        let small = fit(&img, 1600);
        let (buf, d, s) = frame(&small, &depth, &subject);
        let f = focus(&d, &s);
        let variants = [
            ("Subject A50 B0", lb(0.0, edit_model::Bokeh::Circle)),
            ("B50", lb(50.0, edit_model::Bokeh::Circle)),
            ("Cat-eye B50", lb(50.0, edit_model::Bokeh::CatEye)),
        ];
        let renders: Vec<image::RgbImage> = std::iter::once(small.clone())
            .chain(variants.iter().map(|(_, l)| {
                let p = pipeline::lens_blur::Params::new(l, f, 1600);
                to_rgb8(&pipeline::lens_blur::render(&buf, &d, Some(&s), &p))
            }))
            .collect();
        // The crop: a sixth of the frame around the subject's outline — the strongest change of the
        // subject alpha in the middle two thirds.
        let (w, h) = (small.width() as usize, small.height() as usize);
        let (mut best, mut at) = (-1.0f32, (w / 2, h / 2));
        for y in (h / 6..h * 5 / 6).step_by(2) {
            for x in (w / 6..w * 5 / 6).step_by(2) {
                let g = (s[y * w + (x + 1).min(w - 1)] - s[y * w + x.saturating_sub(1)]).abs()
                    + (s[(y + 1).min(h - 1) * w + x] - s[y.saturating_sub(1) * w + x]).abs();
                if g > best {
                    best = g;
                    at = (x, y);
                }
            }
        }
        let (cw, ch) = (w as u32 / 6, h as u32 / 6);
        let cx = (at.0 as u32).saturating_sub(cw / 2).min(w as u32 - cw);
        let cy = (at.1 as u32).saturating_sub(ch / 2).min(h as u32 - ch);
        let tile = 420u32;
        let th = (tile as f32 * h as f32 / w as f32).round() as u32;
        let zh = (tile as f32 * ch as f32 / cw as f32).round() as u32;
        let mut sheet =
            image::RgbImage::from_pixel(4 * (tile + 6), th + zh + 12, image::Rgb([40, 40, 40]));
        for (i, r) in renders.iter().enumerate() {
            let x = i as u32 * (tile + 6);
            sheet
                .copy_from(
                    &image::imageops::resize(r, tile, th, image::imageops::FilterType::Triangle),
                    x,
                    0,
                )
                .expect("fits");
            let crop = image::imageops::crop_imm(r, cx, cy, cw, ch).to_image();
            sheet
                .copy_from(
                    &image::imageops::resize(&crop, tile, zh, image::imageops::FilterType::Nearest),
                    x,
                    th + 6,
                )
                .expect("fits");
        }
        let stem = std::path::Path::new(photo)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        sheet
            .save(out.join(format!("{stem}_lens_blur.png")))
            .expect("save");
    }
}
