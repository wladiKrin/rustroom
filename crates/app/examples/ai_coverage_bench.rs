//! M9.13 follow-up: what one AI component's frame map costs to build (`ai_coverage::to_frame`), at
//! the tint's size, a fitted preview and full resolution — the cost a geometry drag or a tint miss
//! pays every frame. `cargo run --release -p app --example ai_coverage_bench`

use std::time::Instant;

fn main() {
    let map = ai_mask::ProbMap::from_fn(1024, 683, |x, y| ((x ^ y) & 1) as f32);
    let g = edit_model::Geometry {
        angle: 3.0,
        ..Default::default()
    };
    for (label, fw, fh) in [
        ("tint 1400x933", 1400, 933),
        ("fit 2048x1365", 2048, 1365),
        ("full 6240x4160", 6240, 4160),
    ] {
        let dm = rustroom_app::ai_space::DecodeMap::new(
            &g,
            edit_model::Perspective::default(),
            fw,
            fh,
            None,
            6240,
            4160,
        );
        let (cover, outside) =
            rustroom_app::ai_coverage::cover_fn(&edit_model::Mask::Ai(Default::default()))
                .expect("an AI mask reads a map");
        let _ = rustroom_app::ai_coverage::to_frame(&map, &cover, outside, &dm, None);
        let t = Instant::now();
        let n = 5;
        for _ in 0..n {
            std::hint::black_box(rustroom_app::ai_coverage::to_frame(
                &map, &cover, outside, &dm, None,
            ));
        }
        println!(
            "{label}: {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0 / n as f64
        );
    }

    // With the author's real lens, when a RAF is given: `… -- /path/to/DSCF.RAF`.
    let Some(raf) = std::env::args().nth(1) else {
        return;
    };
    let d = raw_decode::decode_raw(
        std::path::Path::new(&raf),
        &raw_decode::DecodeOptions::preview(),
    )
    .unwrap();
    let l = d.metadata.lens.as_ref().expect("the RAF records a lens");
    let shot = lens_profile::Catalogue::shared()
        .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
        .map(|profile| lens_profile::LensShot {
            profile,
            shot: lens_profile::Shot {
                focal: l.focal,
                aperture: l.aperture,
            },
        })
        .expect("the lens has a profile");
    let (dw, dh) = (d.image.width(), d.image.height());
    let edits = edit_model::Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let t = Instant::now();
    for _ in 0..5 {
        std::hint::black_box(rustroom_app::ai_space::corrector_for(&shot, &edits, dw, dh));
    }
    println!(
        "Corrector::new ({dw}x{dh}): {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0 / 5.0
    );
    // The shipped path: the map warped through the lens ONCE (memoised), then per frame only the
    // geometry — what a rotation frame or a tint miss now costs.
    let map = std::sync::Arc::new(map);
    let key = rustroom_app::ai_space::lens_key(Some(&shot), &edits, dw, dh);
    let t = Instant::now();
    let _ = rustroom_app::ai_coverage::post_lens(&map, key, (dw, dh), || {
        rustroom_app::ai_space::corrector_for(&shot, &edits, dw, dh)
    });
    println!(
        "post_lens, first (warp + corrector): {:.1} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    for (label, fw, fh) in [
        ("tint+lens 1400x933", 1400, 933),
        ("fit+lens 2048x1365", 2048, 1365),
        ("full+lens 6240x4160", 6240, 4160),
    ] {
        let t = Instant::now();
        for _ in 0..5 {
            let hit = rustroom_app::ai_coverage::post_lens(&map, key, (dw, dh), || {
                panic!("the memo must hit")
            });
            let dm = rustroom_app::ai_space::DecodeMap::new(
                &g,
                edit_model::Perspective::default(),
                fw,
                fh,
                None,
                dw,
                dh,
            );
            std::hint::black_box(rustroom_app::ai_coverage::to_frame(
                &hit,
                |p| rustroom_app::ai_coverage::refine(p, 0.0, false),
                0,
                &dm,
                None,
            ));
        }
        println!(
            "{label} (memo hit, per frame): {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0 / 5.0
        );
    }
}
