//! Does a **masked** dehaze's airlight move during a Straighten drag?
//!
//! ⚠⚠ **Both do, and the header this probe SHIPPED WITH said otherwise — corrected by its own
//! first run.** `render_engine`, `batch_export` and the CLI all pass `prefix_airlight` into
//! `build_prefix_steps`, so the global `DehazeStage` is built with `with_airlight`. That makes its
//! airlight a deterministic function of the **edits** — which is not the same as invariant, because
//! **a Straighten drag changes an edit on every frame** and `prefix_airlight` replays the prefix
//! *including geometry*. `docs/BACKLOG.md` says the replay *"does not move during a drag at all"*;
//! measured, it moves **6.06–6.81 levels** on `DSCF6291` and **1.47–1.71** on `DSCF6296`.
//! M8.7 took the wobble from ~21 levels to ~6. It did not remove it.
//!
//! ⚠⚠ **FIXED 2026-09-17, and the second row is now a COUNTERFACTUAL rather than a measurement.**
//! A mask's capture half used to get no airlight at all: `MaskStage::new_capture` built through
//! `capture_stages`, which called `DehazeStage::new`, so a masked dehaze estimated from its own
//! input on every frame. It is now handed the frame-global value, exactly as the global stage is.
//!
//! This probe computes that second row by calling `dehaze_airlight` itself, so it still reports
//! **what an UNSUPPLIED stage would estimate** — which no longer describes the shipped code.
//! ⚠ Kept, labelled, and not deleted: it is the size of the defect that was removed, which is
//! worth being able to reproduce. **The guard that the fix holds is
//! `a_masks_capture_dehaze_uses_the_supplied_airlight`, not this row** — a probe cannot see a
//! supplied value, and measuring the alternative is how a probe comes to describe code that no
//! longer exists.
//!
//! This prices that, and decides nothing. ⚠⚠ The airlight is an **order statistic** (the mean of
//! the haziest 0.1%), so it is **strongly scene-dependent** — measured across a 0-6 degree drag:
//!
//! | photo | GLOBAL (replay) | MASKED (per frame) |
//! |---|---|---|
//! | `DSCF6291` | 6.06-6.81 | **9.19-10.70** |
//! | `DSCF6296` | 1.47-1.71 | **10.70-11.13** |
//! | `DSCF6509` | 0.03-0.08 | 0.05-0.06 |
//!
//! ⚠ One photograph is not an answer, and `DSCF6509` is why: a dark scene whose airlight sits at
//! ~0.113 barely moves at all. **Run it on several before concluding anything.**
//!
//! ```text
//! cargo run --release -p app --example probe_masked_airlight <raw>
//! ```

use edit_model::{Dehaze, Edits, Mask, MaskedAdjustment};
use pipeline::{PrefixStepKey, RenderScale, Resampler};

/// The angles an ordinary Straighten drag passes through.
const ANGLES: [f32; 17] = [
    -40.0, -35.0, -30.0, -27.0, -25.0, -20.0, -10.0, -5.0, 0.0, 5.0, 10.0, 20.0, 25.0, 27.0, 30.0,
    35.0, 40.0,
];

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_masked_airlight <raw>");
    // ⚠ `preview()` — the half-size decode the app opens with, so the rung is one a drag renders at.
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let (bw, bh) = (d.image.width(), d.image.height());
    let buffer = d.image;
    let scale = RenderScale::new(1.0);
    eprintln!("buffer {bw}x{bh} ({:.1} Mpx)", (bw * bh) as f64 / 1e6);

    let base = |angle: f32| {
        let mut e = Edits::default();
        e.geometry.angle = angle;
        // A mask whose CAPTURE half carries dehaze — the configuration with no airlight.
        let mut m = MaskedAdjustment::default();
        m.adjustments.dehaze = Dehaze { amount: 60.0 };
        m.mask = Mask::default();
        e.masks = vec![m];
        // The GLOBAL dehaze too, so both are exercised on the same frames.
        e.dehaze = Dehaze { amount: 60.0 };
        e
    };

    let mut masked: Vec<[f32; 3]> = Vec::new();
    let mut global: Vec<[f32; 3]> = Vec::new();

    for angle in ANGLES {
        let e = base(angle);
        // The GLOBAL airlight: what M8.7 ships — a deterministic replay, independent of the frame.
        let g = pipeline::prefix_airlight(&e, &buffer, Resampler::default(), None);
        global.push(g);

        // The MASKED one: estimated from the stage's own input, which is the prefix image at the
        // masked-capture step. Run the prefix up to that step and estimate there.
        let steps = pipeline::build_prefix_steps(&e, scale, None, Resampler::default(), Some(g));
        let mut img = buffer.clone();
        let mut found = false;
        for s in &steps {
            if matches!(s.key, PrefixStepKey::MaskedCapture(_)) {
                masked.push(pipeline::dehaze_airlight(&img));
                found = true;
                break;
            }
            img = s.stage.apply(img);
        }
        if !found {
            eprintln!(
                "⚠ no MaskedCapture step at angle {angle} — the fixture is wrong, not the code"
            );
            return;
        }
    }

    let spread = |v: &[[f32; 3]], name: &str| {
        for c in 0..3 {
            let lo = v.iter().map(|a| a[c]).fold(f32::INFINITY, f32::min);
            let hi = v.iter().map(|a| a[c]).fold(f32::NEG_INFINITY, f32::max);
            println!(
                "  {name} ch{c}  {lo:.5} .. {hi:.5}   spread {:.5}  = {:.2} of 255 levels",
                hi - lo,
                (hi - lo) * 255.0
            );
        }
    };

    println!(
        "\nacross a Straighten drag of {} angles, 0.0 .. 6.0 deg:",
        ANGLES.len()
    );
    spread(&global, "GLOBAL (replay, M8.7)");
    spread(&masked, "IF UNSUPPLIED (counterfactual)");
    println!(
        "\n⚠⚠ The GLOBAL rows are NOT zero, and that is this probe's sharpest finding.\n\
         docs/BACKLOG.md says M8.7's replay airlight \"does not move during a drag at all\". That\n\
         conflates DETERMINISTIC with INVARIANT: it is a deterministic function of the EDITS, and a\n\
         Straighten drag changes an edit on every frame. prefix_airlight replays the prefix\n\
         INCLUDING geometry at 256 px, so the angle feeds straight into it. M8.7 took the wobble\n\
         from ~21 levels to ~6; it did not remove it."
    );
}
