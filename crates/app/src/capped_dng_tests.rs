//! A panorama at the cap in Develop (M10.17b): a constructed 16,000 × 5,800 floating-point DNG — the largest
//! picture the panorama merge writes, and larger than anything Rustroom opened before — decoded at full and
//! half size, rendered at fit and at 1:1, and exported.
//!
//! ⚠ `#[ignore]`d: ~1.1 GB a copy. Run in release:
//! `cargo test --release --workspace capped_dng -- --ignored --nocapture`.

use crate::batch_export::export_one;
use crate::batch_export::fixtures::{opts, source, TempDir, CPU};
use crate::render_engine::render_once;
use core_image::ImageBuf;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

const W: usize = 16_000;
const H: usize = 5_800;

/// The DNG, written once per test run into `dir`: a camera's frame facts from a constructed M9.10 DNG
/// (libraw decoding a file of our own making), a smooth light field with texture over it.
fn capped_dng(dir: &TempDir) -> PathBuf {
    use rayon::prelude::*;
    let small = dir.0.join("small.dng");
    export::dng::save(
        &ImageBuf::from_vec(300, 200, vec![0.2; 300 * 200 * 3]),
        &small,
    )
    .unwrap();
    let d = photo_merge::hdr::decode_all(&[small], false, &AtomicBool::new(false), &mut |_| {})
        .unwrap();
    let data: Vec<f32> = (0..H)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..W).flat_map(move |x| {
                let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
                let l = 0.05
                    + 0.4 * u * (1.0 - v)
                    + 0.03 * ((x as f32 * 0.11).sin() * (y as f32 * 0.07).sin());
                [l * 1.1, l, l * 0.9]
            })
        })
        .collect();
    let path = dir.0.join("capped-Pano.dng");
    let preview = image::RgbImage::from_pixel(64, 24, image::Rgb([120, 120, 120]));
    photo_merge::hdr::write_raw_dng(
        &ImageBuf::from_vec(W, H, data),
        &d.metas[0],
        &path,
        &preview,
        false,
    )
    .unwrap();
    path
}

#[test]
#[ignore = "~1.1 GB a copy; release only"]
fn a_capped_panorama_decodes_renders_and_exports() {
    let dir = TempDir::new("capped-dng");
    let t = Instant::now();
    let path = capped_dng(&dir);
    eprintln!(
        "written in {:.1} s ({:.0} MB)",
        t.elapsed().as_secs_f32(),
        std::fs::metadata(&path).unwrap().len() as f64 / 1e6
    );

    let t = Instant::now();
    let full =
        raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::linear_scene()).expect("decodes");
    eprintln!("full decode {:.1} s", t.elapsed().as_secs_f32());
    assert_eq!((full.image.width(), full.image.height()), (W, H));
    let t = Instant::now();
    let half = raw_decode::decode_raw(
        &path,
        &raw_decode::DecodeOptions {
            half_size: true,
            ..raw_decode::DecodeOptions::linear_scene()
        },
    )
    .expect("decodes at half");
    eprintln!("half decode {:.1} s", t.elapsed().as_secs_f32());
    assert_eq!((half.image.width(), half.image.height()), (W / 2, H / 2));
    assert_eq!(
        raw_decode::dimensions(&path),
        Some((W as u32, H as u32)),
        "libraw reads its size"
    );

    let t = Instant::now();
    let fit = render_once(
        &half.image,
        &edit_model::Edits::default(),
        2560,
        0.5,
        color::OutputSpace::Srgb,
        pipeline::Resampler::CatmullRom,
        None,
    );
    eprintln!("fit render {:.2} s", t.elapsed().as_secs_f32());
    assert_eq!(fit.width.max(fit.height), 2560);
    // 1:1 — a viewport's worth of the full decode, rendered at its own size.
    let t = Instant::now();
    let view = full.image.crop(7_000, 2_400, 2_400, 1_600);
    let one = render_once(
        &view,
        &edit_model::Edits::default(),
        2_400,
        1.0,
        color::OutputSpace::Srgb,
        pipeline::Resampler::CatmullRom,
        None,
    );
    eprintln!("1:1 viewport render {:.2} s", t.elapsed().as_secs_f32());
    assert_eq!((one.width, one.height), (2_400, 1_600));

    let t = Instant::now();
    let o = opts(&dir, None);
    let dest = dir.0.join("out.jpg");
    export_one(
        source(full.image),
        &edit_model::Edits::default(),
        &dest,
        &o,
        CPU,
        &export::FileMetadata::resolve(&o.metadata, export::FileMetadata::default()),
        None,
        None,
    )
    .expect("exports");
    eprintln!("full-size export {:.1} s", t.elapsed().as_secs_f32());
    // Peak memory, where the platform says (Linux).
    if let Some(peak) = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM"))
                .map(str::to_owned)
        })
    {
        eprintln!("peak memory: {}", peak.trim_start_matches("VmHWM:").trim());
    }
    let (w, h) = image::image_dimensions(&dest).unwrap();
    assert_eq!((w as usize, h as usize), (W, H));
}
