//! The panorama merge on real rows, measured — M10.17b's probe, for the author's own panoramas.
//!
//! `cargo run --release --example probe_pano -- <out-dir> [--half] <raw…>`: the order, the reference, the
//! focal length, each gain; then per projection (Auto's pick first) a 2,000-px render with the seams drawn in
//! red and Auto crop's rectangle in yellow, a sheet of 1:1 crops at the middle of every seam, and a table of
//! times and sizes (`table.txt`).

use photo_merge::hdr;
use photo_merge::pano::{self, PanoOptions};
use photo_merge::projection::{self, Projection, MAX_LONG_EDGE};
use photo_merge::seam::NONE;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

const RENDER: usize = 2000;
const CROP: usize = 400;

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
    let mut table = String::new();

    let t = Instant::now();
    let d = hdr::decode_all(&paths, half, &cancel, &mut |_| {}).expect("decodes");
    writeln!(
        table,
        "decode: {:.1} s ({} frames, half {half})",
        t.elapsed().as_secs_f32(),
        paths.len()
    )
    .unwrap();
    let t = Instant::now();
    let a = pano::align(&d, None, &cancel, &mut |_| {}).expect("aligns");
    writeln!(
        table,
        "align: {:.1} s · focal {:.1} px · sweep {:?}",
        t.elapsed().as_secs_f32(),
        a.rig.focal_px,
        a.rig.sweep
    )
    .unwrap();
    for &i in &a.rig.order {
        let mark = if i == a.rig.reference {
            " (reference)"
        } else {
            ""
        };
        writeln!(
            table,
            "  {}: gain {:.4} ({:+.2} EV){mark}",
            paths[i].display(),
            a.gains[i],
            a.gains[i].log2()
        )
        .unwrap();
    }
    if let Some(l) = &a.no_profile {
        writeln!(table, "no lens profile: {l}").unwrap();
    }
    let (along, across) = projection::extent(&a.rig);
    writeln!(
        table,
        "extent {along:.1}° along, {across:.1}° across; Auto picks {}",
        projection::auto(&a.rig).label()
    )
    .unwrap();

    let auto = projection::auto(&a.rig);
    let mut order = vec![auto];
    order.extend(
        [
            Projection::Cylindrical,
            Projection::Spherical,
            Projection::Perspective,
        ]
        .into_iter()
        .filter(|p| *p != auto),
    );
    for p in order {
        let opts = PanoOptions {
            projection: Some(p),
            ..PanoOptions::default()
        };
        let t = Instant::now();
        let s = match pano::stitch(&d, &a, &opts, MAX_LONG_EDGE, None, &cancel, &mut |_| {}) {
            Ok(s) => s,
            Err(e) => {
                writeln!(table, "{}: {e}", p.label()).unwrap();
                continue;
            }
        };
        let stitched = t.elapsed().as_secs_f32();
        let (w, h) = (s.image.width(), s.image.height());
        let prophoto = pano::to_prophoto(&s, &d);
        let as_shot = d.metas[s.reference].metadata.as_shot.unwrap_or_default();
        let scale = if half { 0.5 } else { 1.0 };
        let mut img = photo_merge::preview::render(
            &prophoto,
            &edit_model::Edits::default(),
            RENDER,
            scale,
            as_shot,
        );
        let k = w as f32 / img.width() as f32;
        // The seams: where a pixel's owner differs from its right or lower neighbour's.
        let owner = |x: usize, y: usize| s.seams.owner[y * w + x];
        for y in 0..img.height() as usize {
            for x in 0..img.width() as usize {
                let (u, v) = (
                    ((x as f32 + 0.5) * k) as usize,
                    ((y as f32 + 0.5) * k) as usize,
                );
                if u + 2 >= w || v + 2 >= h {
                    continue;
                }
                let o = owner(u, v);
                let edge = o != NONE
                    && [(u + 2, v), (u, v + 2)]
                        .iter()
                        .any(|&(a, b)| owner(a, b) != NONE && owner(a, b) != o);
                if edge {
                    img.put_pixel(x as u32, y as u32, image::Rgb([255, 0, 0]));
                }
            }
        }
        if let Some(c) = s.crop {
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            let (l, r) = (
                (c.left * iw) as u32,
                ((c.right * iw) as u32).min(img.width() - 1),
            );
            let (t, b) = (
                (c.top * ih) as u32,
                ((c.bottom * ih) as u32).min(img.height() - 1),
            );
            for x in l..=r {
                img.put_pixel(x, t, image::Rgb([255, 220, 0]));
                img.put_pixel(x, b, image::Rgb([255, 220, 0]));
            }
            for y in t..=b {
                img.put_pixel(l, y, image::Rgb([255, 220, 0]));
                img.put_pixel(r, y, image::Rgb([255, 220, 0]));
            }
        }
        let name = p.label().to_lowercase();
        img.save(out.join(format!("{name}.jpg"))).unwrap();
        // 1:1 crops at each seam's middle row, side by side.
        let mut crops = Vec::new();
        for pair in a.rig.order.windows(2) {
            let v = h / 2;
            let Some(u) =
                (1..w).find(|&u| owner(u - 1, v) == pair[0] as u8 && owner(u, v) == pair[1] as u8)
            else {
                continue;
            };
            let (x0, y0) = (
                u.saturating_sub(CROP / 2).min(w - CROP.min(w)),
                v.saturating_sub(CROP / 2).min(h - CROP.min(h)),
            );
            let mut tile = core_image::ImageBuf::new(CROP.min(w), CROP.min(h));
            for y in 0..tile.height() {
                for x in 0..tile.width() {
                    tile.set_pixel(x, y, prophoto.pixel(x0 + x, y0 + y));
                }
            }
            crops.push(photo_merge::preview::render(
                &tile,
                &edit_model::Edits::default(),
                CROP,
                scale,
                as_shot,
            ));
        }
        if !crops.is_empty() {
            let mut sheet =
                image::RgbImage::new((CROP as u32 + 8) * crops.len() as u32, CROP as u32);
            for (i, c) in crops.iter().enumerate() {
                image::imageops::overlay(&mut sheet, c, (i as u32 * (CROP as u32 + 8)) as i64, 0);
            }
            sheet.save(out.join(format!("{name}-seams.jpg"))).unwrap();
        }
        writeln!(
            table,
            "{}: {w} × {h} ({:.1} Mpx) in {stitched:.1} s{}",
            p.label(),
            (w * h) as f64 / 1e6,
            s.crop.map_or(String::new(), |c| format!(
                " · crop {:.3} {:.3} {:.3} {:.3}",
                c.left, c.top, c.right, c.bottom
            ))
        )
        .unwrap();
    }
    print!("{table}");
    std::fs::write(out.join("table.txt"), table).unwrap();
}
