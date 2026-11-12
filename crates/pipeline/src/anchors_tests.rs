use super::*;
use crate::brush::FrameMap;
use crate::{build_prefix_steps, RenderScale, Resampler};
use core_image::ImageBuf;
use edit_model::{Brush, BrushStroke, Eye, EyeKind, MaskedAdjustment, Spot, SpotMode};

/// Constructed: a synthetic barrel lens (`ptlens` b = 0.05), the prefix tests' fixture — no RAW, no database.
fn lens() -> LensShot {
    let xml = r#"<lensdatabase version="1">
        <camera><maker>TestMaker</maker><model>TestCam</model>
            <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
        <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
            <mount>TestMount</mount><cropfactor>1.0</cropfactor>
            <calibration>
                <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
            </calibration></lens>
    </lensdatabase>"#;
    let db = lens_profile::Catalogue::from_xml(xml).unwrap();
    LensShot {
        profile: db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap(),
        shot: lens_profile::Shot {
            focal: 50.0,
            aperture: 2.8,
        },
    }
}

fn on() -> LensCorrection {
    LensCorrection {
        distortion: true,
        ..Default::default()
    }
}

fn keystone() -> Perspective {
    Perspective {
        vertical: 30.0,
        horizontal: -15.0,
        ..Default::default()
    }
}

const W: usize = 400;
const H: usize = 300;

/// A dark frame with one bright 3×3 dot centred on pixel `(x, y)`.
fn dot(x: usize, y: usize) -> ImageBuf {
    let mut img = ImageBuf::new(W, H);
    for yy in y - 1..=y + 1 {
        for xx in x - 1..=x + 1 {
            img.set_pixel(xx, yy, [1.0; 3]);
        }
    }
    img
}

/// The brightness-weighted centre of `img`, in pixel coordinates (a pixel's centre is `+0.5`).
fn centroid(img: &ImageBuf) -> (f32, f32) {
    let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
    for y in 0..img.height() {
        for x in 0..img.width() {
            let v = img.pixel(x, y)[1];
            if v > 0.05 {
                sx += (x as f32 + 0.5) * v;
                sy += (y as f32 + 0.5) * v;
                sw += v;
            }
        }
    }
    assert!(sw > 0.5, "the dot left the frame");
    (sx / sw, sy / sw)
}

fn prefix(edits: &Edits, img: ImageBuf) -> ImageBuf {
    let steps = build_prefix_steps(
        edits,
        RenderScale::FULL,
        Some(lens()),
        Resampler::default(),
        None,
        None,
    );
    steps.iter().fold(img, |i, s| s.stage.apply(i))
}

fn norm(x: usize, y: usize) -> [f32; 2] {
    [(x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32]
}

/// ⚠⚠ The one fact the milestone rests on, against the real stages: a decode point mapped to a stroke's space
/// lands where the lens step and the keystone actually put that pixel — and where it lands moved, so a stroke
/// stored there and not mapped would sit off its content.
#[test]
fn a_decode_point_lands_where_the_lens_and_keystone_put_its_pixel() {
    let (x, y) = (36, 28);
    for (lc, p) in [
        (on(), Perspective::default()),
        (LensCorrection::default(), keystone()),
        (on(), keystone()),
    ] {
        let edits = Edits {
            lens_correction: Some(lc),
            perspective: p,
            ..Edits::default()
        };
        let out = prefix(&edits, dot(x, y));
        let got = centroid(&out);
        let a = Anchoring::for_edits(Some(&lens()), &edits, W, H);
        let map = FrameMap::new(&edits.geometry, out.width(), out.height());
        let want = map.point(a.decode_to_stroke(norm(x, y)));
        let (dx, dy) = (got.0 - want.0, got.1 - want.1);
        assert!(dx.hypot(dy) < 0.75, "{lc:?} {p:?}: {got:?} vs {want:?}");
        let unmapped = map.point(norm(x, y));
        assert!(
            (got.0 - unmapped.0).hypot(got.1 - unmapped.1) > 3.0,
            "the fixture must move the dot: {got:?} vs {unmapped:?}"
        );
    }
}

/// The same for a spot or an eye, whose stage runs after the lens and before the keystone.
#[test]
fn a_decode_point_lands_where_the_lens_put_its_pixel_for_a_spot() {
    let (x, y) = (364, 270);
    let edits = Edits {
        lens_correction: Some(on()),
        ..Edits::default()
    };
    let out = prefix(&edits, dot(x, y));
    let got = centroid(&out);
    let a = Anchoring::for_edits(Some(&lens()), &edits, W, H);
    let p = a.decode_to_post_lens(norm(x, y));
    let want = (p[0] * W as f32, p[1] * H as f32);
    assert!(
        (got.0 - want.0).hypot(got.1 - want.1) < 0.75,
        "{got:?} vs {want:?}"
    );
}

#[test]
fn decode_and_working_space_round_trip_to_a_hundredth_of_a_pixel() {
    let a = Anchoring::new(Some(&lens()), on(), keystone(), 6000, 4000);
    for i in 0..=10 {
        for j in 0..=10 {
            let d = [i as f32 / 10.0, j as f32 / 10.0];
            let s = a.stroke_to_decode(a.decode_to_stroke(d));
            let p = a.post_lens_to_decode(a.decode_to_post_lens(d));
            for back in [s, p] {
                let err = ((back[0] - d[0]) * 6000.0).hypot((back[1] - d[1]) * 4000.0);
                assert!(err < 0.01, "{d:?} → {back:?}: {err} px");
            }
        }
    }
}

/// ⚠ Built at 1024 px, used for a 6000 px decode: lensfun normalises by size, so the two must agree — measured
/// 0.12 px with no scaling (the nominal frame's rounded aspect and the grid), and with Scale at its default 100 to within the autoscale's own estimate, which each corrector makes
/// by sampling its own field: measured 1.28 px at the extreme corner of 6000 px (0.04 % of the radius), 0 at the
/// centre. ⚠ Do not loosen the first bound to match the second.
#[test]
fn the_nominal_corrector_agrees_with_a_full_size_one() {
    for (scale, bound) in [(0.0, 0.2), (100.0, 1.5)] {
        let lc = LensCorrection { scale, ..on() };
        let a = Anchoring::new(Some(&lens()), lc, Perspective::default(), 6000, 4000);
        let l = lens();
        let full = Corrector::new(
            &l.profile,
            &l.shot,
            6000,
            4000,
            Corrections {
                distortion: true,
                tca: false,
                vignetting: false,
                scale,
            },
        )
        .unwrap();
        let mut worst: f32 = 0.0;
        for i in 0..=8 {
            for j in 0..=8 {
                let p = [i as f32 / 8.0, j as f32 / 8.0];
                // Pixel centres at `i + 0.5`, the convention `post_lens_to_decode` follows.
                let s = full.source(p[0] * 6000.0 - 0.5, p[1] * 4000.0 - 0.5)[1];
                let n = a.post_lens_to_decode(p);
                worst = worst.max((s[0] + 0.5 - n[0] * 6000.0).hypot(s[1] + 0.5 - n[1] * 4000.0));
            }
        }
        assert!(
            worst.is_finite() && worst < bound,
            "scale {scale}: {worst} px"
        );
    }
}

fn marked(decode: bool) -> Edits {
    Edits {
        spots: vec![Spot {
            centre: [0.1, 0.1],
            source: [0.2, 0.1],
            radius: 0.02,
            feather: 50.0,
            mode: SpotMode::Heal,
            dust: false,
            decode,
        }],
        eyes: vec![Eye {
            centre: [0.9, 0.85],
            radius: 0.01,
            kind: EyeKind::Red,
            pupil_size: 50.0,
            darken: 50.0,
            catchlight: None,
            decode,
        }],
        masks: vec![MaskedAdjustment {
            mask: Mask::Brush(Brush {
                strokes: vec![BrushStroke {
                    points: vec![[0.05, 0.9], [0.1, 0.92]],
                    radius: 0.03,
                    decode,
                    ..Default::default()
                }],
                invert: false,
            }),
            ..Default::default()
        }],
        lens_correction: Some(on()),
        perspective: keystone(),
        ..Edits::default()
    }
}

#[test]
fn a_mark_from_before_passes_through_untouched() {
    let e = marked(false);
    let a = Anchoring::for_edits(Some(&lens()), &e, W, H);
    let r = resolve(&e, &a);
    assert!(matches!(r, Cow::Borrowed(_)));
    assert_eq!(*r, e);
}

#[test]
fn without_a_lens_or_a_keystone_a_decode_mark_is_where_it_was() {
    let e = Edits {
        lens_correction: None,
        perspective: Perspective::default(),
        ..marked(true)
    };
    let a = Anchoring::for_edits(Some(&lens()), &e, W, H);
    assert!(a.is_identity());
    let r = resolve(&e, &a).into_owned();
    assert_eq!(
        r,
        Edits {
            lens_correction: None,
            perspective: Perspective::default(),
            ..marked(false)
        }
    );
}

/// Opening a photo moves its old marks into decode space; resolving them gives back exactly where they rendered.
#[test]
fn a_migrated_mark_resolves_to_where_it_rendered() {
    let before = marked(false);
    let a = Anchoring::for_edits(Some(&lens()), &before, W, H);
    let mut e = before.clone();
    assert!(migrate(&mut e, &a));
    assert!(!has_legacy(&e) && has_anchored(&e));
    assert_ne!(
        e.spots[0].centre, before.spots[0].centre,
        "the lens must move it"
    );
    let back = resolve(&e, &a).into_owned();
    let close = |x: [f32; 2], y: [f32; 2]| (x[0] - y[0]).abs() < 1e-5 && (x[1] - y[1]).abs() < 1e-5;
    assert!(close(back.spots[0].centre, before.spots[0].centre));
    assert!(close(back.spots[0].source, before.spots[0].source));
    assert!(close(back.eyes[0].centre, before.eyes[0].centre));
    assert!((back.spots[0].radius - before.spots[0].radius).abs() < 1e-6);
    assert!((back.eyes[0].radius - before.eyes[0].radius).abs() < 1e-6);
    let Mask::Brush(b) = &back.masks[0].mask else {
        unreachable!()
    };
    let Mask::Brush(was) = &before.masks[0].mask else {
        unreachable!()
    };
    assert!((b.strokes[0].radius - was.strokes[0].radius).abs() < 1e-6);
    for (p, q) in b.strokes[0].points.iter().zip(&was.strokes[0].points) {
        assert!(close(*p, *q), "{p:?} vs {q:?}");
    }
    assert!(!migrate(&mut e, &a), "a second migration changes nothing");
}

/// What a frame pays: ten strokes of 2,000 points, resolved, recorded as a number.
#[test]
fn resolving_twenty_thousand_points_costs_well_under_a_frame() {
    let mut e = marked(true);
    let Mask::Brush(b) = &mut e.masks[0].mask else {
        unreachable!()
    };
    b.strokes = (0..10)
        .map(|k| BrushStroke {
            points: (0..2000)
                .map(|i| [i as f32 / 2000.0, k as f32 / 10.0])
                .collect(),
            radius: 0.02,
            decode: true,
            ..Default::default()
        })
        .collect();
    let a = Anchoring::for_edits(Some(&lens()), &e, 6000, 4000);
    let _ = resolve(&e, &a);
    let t = std::time::Instant::now();
    let _ = resolve(&e, &a);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!("resolve 20,000 points: {ms:.2} ms");
    assert!(ms < 50.0, "{ms} ms");
}

/// ⚠⚠ The author's report (2026-11-12): after a keystone, the perspective's Scale moved a stroke with its content
/// but left its WIDTH behind. Measured against the real stages: how far apart two decode pixels land, at Scale 0
/// and at +100, must grow exactly as a resolved stroke's radius does — and it must grow, or the test sees nothing.
#[test]
fn a_strokes_width_follows_the_scale_as_its_content_does() {
    let at = |scale: f32| Edits {
        lens_correction: Some(on()),
        perspective: Perspective {
            scale,
            ..keystone()
        },
        ..Edits::default()
    };
    let gap = |e: &Edits| {
        let a = centroid(&prefix(e, dot(194, 146)));
        let b = centroid(&prefix(e, dot(206, 146)));
        (a.0 - b.0).hypot(a.1 - b.1)
    };
    let width = |e: &Edits| {
        let stroke = Edits {
            masks: marked(true).masks,
            ..e.clone()
        };
        let a = Anchoring::for_edits(Some(&lens()), &stroke, W, H);
        let r = resolve(&stroke, &a).into_owned();
        let Mask::Brush(b) = &r.masks[0].mask else {
            unreachable!()
        };
        b.strokes[0].radius
    };
    let (lo, hi) = (at(0.0), at(100.0));
    let content = gap(&hi) / gap(&lo);
    assert!(
        (content - 1.0).abs() > 0.03,
        "the Scale must magnify: {content}"
    );
    let drawn = width(&hi) / width(&lo);
    assert!(
        (drawn - content).abs() < 0.01,
        "width ×{drawn} against content ×{content}"
    );
}

/// Placing a patch and reading it back keeps its size: stored in decode units, drawn at the magnification where
/// it sits.
#[test]
fn a_spots_radius_round_trips_through_its_anchor() {
    let a = Anchoring::new(Some(&lens()), on(), keystone(), W, H);
    let worked = Spot {
        centre: [0.1, 0.12],
        source: [0.3, 0.12],
        radius: 0.03,
        feather: 50.0,
        mode: SpotMode::Heal,
        dust: false,
        decode: false,
    };
    let stored = anchor_spot(&a, worked);
    assert!(
        (stored.radius - worked.radius).abs() > 1e-4,
        "the lens must change the unit here"
    );
    let e = Edits {
        spots: vec![stored],
        ..Edits::default()
    };
    let back = resolve(&e, &a).into_owned().spots[0];
    assert!(
        (back.radius - worked.radius).abs() < 1e-5,
        "{} vs {}",
        back.radius,
        worked.radius
    );
}
