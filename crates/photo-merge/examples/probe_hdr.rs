//! The HDR merge on real brackets, measured — M10.17a's probe, kept as the re-test.
//!
//! `cargo run --release --example probe_hdr -- <out-dir> [--half] <raw…>`: per frame its EXIF exposure,
//! the alignment (corner motion, the fitted offset, overlap, residual) and the measured scale; then
//! the merge at every deghost level, a 1600-px render of each (overlay in red) and a sheet of 4×
//! crops — None · Low · Medium · High side by side — where the overlay marks most.

use photo_merge::align::{self, COARSEST};
use photo_merge::deghost::Deghost;
use photo_merge::hdr::{self, HdrOptions};
use photo_merge::homography::Homography;
use photo_merge::pyramid::LumPyramid;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.remove(0));
    let half = args.first().is_some_and(|a| a == "--half");
    if half {
        args.remove(0);
    }
    let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    std::fs::create_dir_all(&out).unwrap();
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let d = hdr::decode_all(&paths, half, &cancel, &mut |_| {}).expect("decodes");
    println!(
        "decode: {:.1} s ({} frames, half {half})",
        t.elapsed().as_secs_f32(),
        paths.len()
    );

    // The alignment, frame by frame, against the median frame.
    let evs: Vec<Option<f32>> = d.frames.iter().map(|f| f.ev).collect();
    let r = hdr::reference_index(&evs, &vec![0.0; evs.len()], None);
    let t = Instant::now();
    let pyr: Vec<LumPyramid> = d
        .frames
        .iter()
        .map(|f| LumPyramid::build(f, COARSEST))
        .collect();
    println!("pyramids: {:.2} s", t.elapsed().as_secs_f32());
    let (w, h) = (d.frames[r].width, d.frames[r].height);
    for i in 0..paths.len() {
        let exif = match (evs[i], evs[r]) {
            (Some(a), Some(b)) => format!("{:+.2}", a - b),
            _ => "?".into(),
        };
        if i == r {
            println!("{}: reference, EXIF {exif} EV", paths[i].display());
            continue;
        }
        // The independent check: no alignment, no fitting — the median ratio of the raw green where
        // both frames show it mid-range, pixel for pixel (the frames barely moved).
        let mut ratios: Vec<f32> = Vec::new();
        let (fr, ff) = (&d.frames[r], &d.frames[i]);
        for y in (0..h).step_by(7) {
            for x in (0..w).step_by(7) {
                let (a, b) = (fr.raw(x, y)[1], ff.raw(x, y)[1]);
                if !fr.clipped(x, y)
                    && !ff.clipped(x, y)
                    && (0.05..0.7).contains(&a)
                    && (0.05..0.7).contains(&b)
                {
                    ratios.push(b / a);
                }
            }
        }
        ratios.sort_by(|a, b| a.total_cmp(b));
        let naive = ratios.get(ratios.len() / 2).map_or(f32::NAN, |v| v.log2());
        let t = Instant::now();
        let finest = if half { 0 } else { hdr::FULL_FINEST };
        let a = align::align(&pyr[r], &pyr[i], None, finest);
        let took = t.elapsed().as_secs_f32();
        let scale = photo_merge::exposure::measure(&d.frames[r], &d.frames[i], &a);
        println!(
            "{}: EXIF {exif} EV · unaligned median {naive:+.3} EV · fitted {:+.3} EV · measured {:+.3} EV · corners {:.2} px · overlap {:.3} · residual {:.4} · {took:.2} s",
            paths[i].display(),
            a.log_gain,
            -scale.log2(),
            a.h.corner_shift(&Homography::identity(), w, h),
            a.overlap,
            a.residual
        );
    }

    // Every deghost level, rendered.
    let mut renders = Vec::new();
    let mut overlays = Vec::new();
    for level in Deghost::ALL {
        let t = Instant::now();
        let m = hdr::merge(
            &d,
            None,
            &HdrOptions {
                auto_align: true,
                deghost: level,
            },
            None,
            &cancel,
            &mut |_| {},
        )
        .expect("merges");
        let took = t.elapsed().as_secs_f32();
        let mut img = photo_merge::preview::render(
            &hdr::to_prophoto(&m, &d),
            &edit_model::Edits::default(),
            1600,
            if half { 0.5 } else { 1.0 },
            d.metas[m.reference].metadata.as_shot.unwrap_or_default(),
        );
        let marked = m.overlay.as_ref().map_or(0.0, |o| {
            o.data.iter().filter(|v| **v > 0.5).count() as f32 / o.data.len() as f32
        });
        println!(
            "{}: merge {took:.2} s, {:.2} % marked",
            level.label(),
            100.0 * marked
        );
        if let Some(o) = &m.overlay {
            photo_merge::preview::tint(&mut img, o);
        }
        img.save(out.join(format!("{}.jpg", level.label().to_lowercase())))
            .unwrap();
        renders.push(img);
        overlays.push(m.overlay);
    }

    // Where High marks most: the 160 × 120 window of the render with the most overlay.
    let (rw, rh) = (renders[0].width(), renders[0].height());
    let (cw, ch) = (160u32, 120u32);
    let mut best = (0u32, 0u32, -1f32);
    if let Some(o) = overlays[3].as_ref() {
        for y in (0..rh.saturating_sub(ch)).step_by(20) {
            for x in (0..rw.saturating_sub(cw)).step_by(20) {
                let mut s = 0f32;
                for yy in (y..y + ch).step_by(8) {
                    for xx in (x..x + cw).step_by(8) {
                        s += o.at(xx as f32 / rw as f32, yy as f32 / rh as f32);
                    }
                }
                if s > best.2 {
                    best = (x, y, s);
                }
            }
        }
    }
    let k = 4;
    let mut sheet = image::RgbImage::new(cw * k * 4 + 3 * 8, ch * k);
    for (n, img) in renders.iter().enumerate() {
        let x0 = n as u32 * (cw * k + 8);
        for y in 0..ch * k {
            for x in 0..cw * k {
                let p = *img.get_pixel((best.0 + x / k).min(rw - 1), (best.1 + y / k).min(rh - 1));
                sheet.put_pixel(x0 + x, y, p);
            }
        }
    }
    sheet.save(out.join("deghost-sheet.jpg")).unwrap();
    println!(
        "sheet at render ({}, {}) — None · Low · Medium · High",
        best.0, best.1
    );
}
