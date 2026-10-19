//! M10.16c1: the removals' step — right after AI denoise, before lens correction, waiting for its fills.

use super::*;
use crate::{AiRasters, Fill, ImageSource, RasterSource};
use core_image::ImageBuf;
use std::sync::Arc;

/// A gray background with a bright block — the "object" — at x 8..12, y 4..8 of a 20 × 12 frame.
fn photo(w: usize, h: usize) -> ImageBuf {
    let mut img = ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let object = (8..12).contains(&x) && (4..8).contains(&y);
            img.set_pixel(
                x,
                y,
                if object {
                    [0.9, 0.9, 0.1]
                } else {
                    [0.3, 0.35, 0.4]
                },
            );
        }
    }
    img
}

/// The object's rect, normalised, and a fill of the background over it.
const RECT: [f32; 4] = [8.0 / 20.0, 4.0 / 12.0, 12.0 / 20.0, 8.0 / 12.0];
fn background() -> Arc<Fill> {
    Arc::new(Fill {
        rect: RECT,
        w: 4,
        h: 4,
        rgb: [0.3, 0.35, 0.4].repeat(16),
        alpha: vec![1.0; 16],
    })
}

/// The maps: the "denoised" photo — computed from the decode, so it STILL HOLDS the object — and each
/// removal's fill under an id (`None`: not computed yet).
struct Maps {
    denoised: bool,
    fill: Option<u64>,
}
impl AiRasters for Maps {
    fn source(&self, _: &edit_model::Mask) -> Option<(Arc<dyn RasterSource>, u64)> {
        None
    }
    fn denoised(&self) -> Option<(Arc<dyn ImageSource>, u64)> {
        self.denoised
            .then(|| (Arc::new(Photo) as Arc<dyn ImageSource>, 7))
    }
    fn fill(&self, _: &edit_model::Removal) -> Option<(Arc<Fill>, u64)> {
        self.fill.map(|id| (background(), id))
    }
}
struct Photo;
impl ImageSource for Photo {
    fn image(&self, w: usize, h: usize) -> Option<Arc<ImageBuf>> {
        Some(Arc::new(photo(w, h)))
    }
}

fn removal() -> edit_model::Removal {
    edit_model::Removal {
        points: vec![[0.5, 0.5]],
        radius: 0.1,
        detect: false,
        variant: 0,
        opacity: 100.0,
    }
}

fn with_removal() -> Edits {
    Edits {
        removals: vec![removal()],
        ..Default::default()
    }
}

fn steps(
    e: &Edits,
    lens: Option<lens_profile::LensShot>,
    ai: Option<&dyn AiRasters>,
) -> Vec<PrefixStep> {
    build_prefix_steps(e, RenderScale::FULL, lens, Resampler::default(), None, ai)
}

#[test]
fn no_fill_yet_adds_no_step() {
    let e = with_removal();
    let none = Maps {
        denoised: false,
        fill: None,
    };
    assert!(steps(&e, None, Some(&none)).is_empty(), "no fill yet");
    assert!(steps(&e, None, None).is_empty(), "no maps at all");
    let one = Maps {
        denoised: false,
        fill: Some(1),
    };
    assert_eq!(steps(&e, None, Some(&one)).len(), 1, "positive control");
    assert!(
        steps(&Edits::default(), None, Some(&one)).is_empty(),
        "no removal, no step, whatever the maps hold"
    );
}

/// A synthetic distortion profile, so the lens step exists.
fn a_lens() -> Option<lens_profile::LensShot> {
    let xml = r#"<lensdatabase version="1">
        <camera><maker>M</maker><model>C</model><mount>X</mount><cropfactor>1.0</cropfactor></camera>
        <lens><maker>M</maker><model>L 50mm</model><mount>X</mount><cropfactor>1.0</cropfactor>
            <calibration><distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/></calibration>
        </lens></lensdatabase>"#;
    let db = lens_profile::Catalogue::from_xml(xml).unwrap();
    let profile = db.find("M", "C", "L 50mm").unwrap();
    Some(lens_profile::LensShot {
        profile,
        shot: lens_profile::Shot {
            focal: 50.0,
            aperture: 2.8,
        },
    })
}

/// In decode space, so after AI denoise (whose result would bring the object back) and before the lens
/// warp moves anything.
#[test]
fn the_step_follows_ai_denoise_and_precedes_lens() {
    let e = Edits {
        ai_denoise: edit_model::AiDenoise { amount: 60.0 },
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            ..Default::default()
        }),
        dehaze: edit_model::Dehaze { amount: 30.0 },
        ..with_removal()
    };
    let maps = Maps {
        denoised: true,
        fill: Some(1),
    };
    let names: Vec<&str> = steps(&e, a_lens(), Some(&maps))
        .iter()
        .map(|s| s.stage.name())
        .collect();
    assert_eq!(
        names[..3],
        ["ai_denoise", "remove", "lens_correction"],
        "{names:?}"
    );
}

/// The key follows every removal's stroke, Refresh, Opacity and Detect, and its fill's content.
#[test]
fn the_key_moves_with_each_removal_and_its_fill() {
    let key = |e: &Edits, id: u64| {
        steps(
            e,
            None,
            Some(&Maps {
                denoised: false,
                fill: Some(id),
            }),
        )[0]
        .key
        .clone()
    };
    let base = with_removal();
    assert_eq!(key(&base, 1), key(&base, 1));
    let moved = |f: &dyn Fn(&mut edit_model::Removal)| {
        let mut e = base.clone();
        f(&mut e.removals[0]);
        e
    };
    let variants = [
        ("the stroke", moved(&|r| r.points[0][0] = 0.6)),
        ("the radius", moved(&|r| r.radius = 0.2)),
        ("Refresh", moved(&|r| r.variant = 1)),
        ("Opacity", moved(&|r| r.opacity = 40.0)),
        ("Detect objects", moved(&|r| r.detect = true)),
    ];
    for (what, e) in variants {
        assert_ne!(key(&base, 1), key(&e, 1), "{what}");
    }
    assert_ne!(key(&base, 1), key(&base, 2), "another fill");
}

/// ⚠⚠ The anti-goal: AI denoise's result was computed from the decode WITH the object, so a removal
/// pasted before it would be undone by the blend. At Amount 100 the object's pixels must be the fill's.
#[test]
fn a_removal_after_ai_denoise_stays_removed() {
    let e = Edits {
        ai_denoise: edit_model::AiDenoise { amount: 100.0 },
        ..with_removal()
    };
    let maps = Maps {
        denoised: true,
        fill: Some(1),
    };
    let out = steps(&e, None, Some(&maps))
        .iter()
        .fold(photo(20, 12), |img, s| s.stage.apply(img));
    for y in 4..8 {
        for x in 8..12 {
            let p = out.pixel(x, y);
            for (c, want) in [0.3, 0.35, 0.4].iter().enumerate() {
                assert!((p[c] - want).abs() < 1e-6, "({x}, {y}) {c}: {}", p[c]);
            }
        }
    }
}
