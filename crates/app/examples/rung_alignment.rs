//! Does the PHOTOGRAPH move when the render rung changes?
//!
//! Run: `cargo run --release -p app --example rung_alignment <raw>`
//!
//! ⚠⚠ **This exists because reasoning about placement has been wrong here twice, and because the
//! canvas's own instrumentation ruled the placement out.** `RUSTROOM_DEBUG_PLACE` over 836 real
//! frames showed the frame *rect* is rung-invariant to 0.25 px, and exactly invariant vertically
//! — the constrained axis's on-screen size is `avail` by construction. Yet the shift is reported
//! as visible. So the remaining suspect is the rendered **content**: two rungs producing images
//! that are not aligned with each other.
//!
//! ⚠ Lens corrections are **ON**, as the GUI has them and as `Edits::default()` does not. They
//! are the only geometric stage whose displacement field is rebuilt per resolution, so a probe
//! that left them off would measure a pipeline nobody runs — `bench_crop_rung`'s note, for
//! `bench_crop_rung`'s reason.
//!
//! ⚠ The comparison resamples the higher rung DOWN to the lower one with `downscale_to`, which is
//! area-averaging and therefore centre-aligned: output pixel `i` covers source `[i*n/d,
//! (i+1)*n/d)`, and those boxes tile the source exactly. So it introduces no offset of its own,
//! which is what makes a measured offset attributable to the render.
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::RenderScale;

const FIT_CAP: usize = 1600;

/// Rec.709 luma of an `ImageBuf`, as a plain plane.
fn luma(img: &ImageBuf) -> Vec<f32> {
    img.data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| color::luminance(*p))
        .collect()
}

/// Sum of squared differences between `a` and `b` when `b` is shifted by `(dx, dy)`, over the
/// overlapping interior only.
///
/// ⚠ An **interior** window, inset by the search radius plus a border: the two renders genuinely
/// disagree at the frame edge (one is a region of a larger image, the other is not), and a border
/// term would dominate a sub-pixel measurement.
fn ssd(a: &[f32], b: &[f32], w: usize, h: usize, dx: i32, dy: i32, inset: usize) -> f64 {
    let mut sum = 0.0;
    for y in inset..h - inset {
        for x in inset..w - inset {
            let sx = (x as i32 + dx) as usize;
            let sy = (y as i32 + dy) as usize;
            let d = (a[y * w + x] - b[sy * w + sx]) as f64;
            sum += d * d;
        }
    }
    sum
}

/// The sub-pixel shift of `b` relative to `a`, by SSD minimisation with a parabolic refinement.
///
/// ⚠ The parabola is fitted on each axis independently through the integer minimum and its two
/// neighbours. That is exact for a quadratic and adequate here; it is a diagnostic, not a
/// registration algorithm, and the question it answers is "is this zero or is it a pixel?".
fn shift(a: &[f32], b: &[f32], w: usize, h: usize) -> (f64, f64) {
    const R: i32 = 3;
    let inset = R as usize + 2;
    let mut best = (f64::INFINITY, 0i32, 0i32);
    for dy in -R..=R {
        for dx in -R..=R {
            let s = ssd(a, b, w, h, dx, dy, inset);
            if s < best.0 {
                best = (s, dx, dy);
            }
        }
    }
    let (_, bx, by) = best;
    let refine = |f: &dyn Fn(i32) -> f64, c: i32| -> f64 {
        let (lo, mid, hi) = (f(c - 1), f(c), f(c + 1));
        let den = lo - 2.0 * mid + hi;
        if den.abs() < 1e-12 {
            c as f64
        } else {
            c as f64 + 0.5 * (lo - hi) / den
        }
    };
    let fx = |dx: i32| ssd(a, b, w, h, dx, by, inset);
    let fy = |dy: i32| ssd(a, b, w, h, bx, dy, inset);
    (refine(&fx, bx), refine(&fy, by))
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: rung_alignment <raw>");
    let path = std::path::Path::new(&path);

    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let lens = d.metadata.lens.as_ref().and_then(|l| {
        lens_profile::Catalogue::shared()
            .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
            .map(|profile| lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
    });
    println!(
        "lens profile: {}",
        if lens.is_some() { "yes" } else { "no" }
    );

    let buf = std::sync::Arc::new(d.image);
    let buffer_long = buf.width().max(buf.height());
    println!("buffer {}x{}", buf.width(), buf.height());

    let rungs: Vec<usize> = {
        let mut v = vec![FIT_CAP];
        let mut r = FIT_CAP;
        while r < buffer_long {
            r = (r * 2).min(buffer_long);
            v.push(r);
        }
        v
    };
    println!("ladder: {rungs:?}\n");

    // ⚠⚠ Lens corrections ON, as the GUI has them. See the module note.
    let with_lens = Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        }),
        ..Edits::default()
    };
    let bare = Edits::default();

    let render = |edits: &Edits, max_dim: usize, lens: &Option<lens_profile::LensShot>| {
        let scale = RenderScale::new(max_dim as f32 / buffer_long as f32);
        let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            edits,
            scale,
            None,
            None,
            pipeline::RenderSpec::default(),
            lens.clone(),
            pipeline::ProfileRendering::default(),
        );
        let src = ImageBuf::clone(&buf).downscale_to(max_dim);
        tail.run(prefix.run(src))
    };

    for (label, edits, lens) in [
        (
            "lens corrections ON (what the GUI runs)",
            &with_lens,
            lens.clone(),
        ),
        ("lens corrections off (the control)", &bare, None),
    ] {
        println!("=== {label} ===");
        let base = render(edits, rungs[0], &lens);
        let (bw, bh) = (base.width(), base.height());
        let a = luma(&base);
        println!("  rung {:>5}  {bw}x{bh}   (reference)", rungs[0]);
        for r in &rungs[1..] {
            let hi = render(edits, *r, &lens);
            let (hw, hh) = (hi.width(), hi.height());
            let down = hi.downscale_to(bw.max(bh));
            if down.width() != bw || down.height() != bh {
                println!(
                    "  rung {r:>5}  {hw}x{hh} -> {}x{} MISMATCH against {bw}x{bh}, skipped",
                    down.width(),
                    down.height()
                );
                continue;
            }
            let b = luma(&down);
            let (dx, dy) = shift(&a, &b, bw, bh);
            // At fit resolution one pixel is `fit * zoom` screen pixels; at the reporter's
            // 7.72x zoom on a 2167 pt canvas that is 8.88.
            println!(
                "  rung {r:>5}  {hw}x{hh} -> {bw}x{bh}   shift ({dx:+.3}, {dy:+.3}) fit px \
                 = ({:+.2}, {:+.2}) screen px at 8.88x",
                dx * 8.88,
                dy * 8.88
            );
        }
        println!();
    }
    println!(
        "⚠ A shift of 0.000 means the two rungs are aligned and the reported movement is NOT in\n\
         the render. Anything above ~0.1 fit px is about a screen pixel at a zoomed view and is."
    );
}
