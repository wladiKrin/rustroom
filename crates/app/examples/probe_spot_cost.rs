//! What does spot removal actually cost, before anyone ports it?
//!
//! ⚠⚠ **M8.4's rule: measure the alternatives to zero before committing a milestone.** Item 5 of
//! the agreed GPU-port order is spot removal, and its value has never been measured — unlike every
//! stage ported before it, whose cost was priced first.
//!
//! **Two facts make this stage unlike the others, and both cut the same way:**
//!
//! 1. It is a **prefix** stage (`PrefixStepKey::SpotRemoval`), so `PrefixCache` holds its output.
//!    It re-runs when the SPOT LIST changes — while placing or dragging a spot — not on every
//!    slider drag the way a tail stage does.
//! 2. Its work is proportional to the **spots' own disc area**, not to the frame. `apply_spot`
//!    touches roughly `2.25·π·r²` per spot (the disc plus the ring a heal reads), so a frame with
//!    a few small spots is a few thousand pixels of work on a 6.5 Mpx image.
//!
//! This prints ms per frame against spot count and radius, beside the whole prefix for scale.
//! ⚠ It decides nothing. In particular it must NOT be read against the catalogue's sidecars:
//! `CLAUDE.md` records that the user has not been using Rustroom yet, so those describe folders
//! rather than what a photographer does.
//!
//! ```text
//! cargo run --release -p app --example probe_spot_cost <raw>
//! ```

use edit_model::{Edits, Spot, SpotMode};
use pipeline::{PipelineStage, RenderScale, Resampler};
use std::time::Instant;

/// Radii as a fraction of the frame's long edge — a dust speck, a sensor blob, a bird.
const RADII: [f32; 3] = [0.004, 0.015, 0.05];
const COUNTS: [usize; 4] = [1, 5, 20, 100];

fn spots(n: usize, radius: f32, mode: SpotMode) -> Vec<Spot> {
    (0..n)
        .map(|i| {
            // ⚠ Spread over the frame and never overlapping the source, so no spot reads another's
            // output — which would price a different thing.
            let t = i as f32 / n.max(1) as f32;
            Spot {
                decode: false,
                centre: [0.2 + 0.6 * t, 0.3 + 0.4 * ((i % 7) as f32 / 7.0)],
                source: [0.2 + 0.6 * t, 0.7 - 0.4 * ((i % 7) as f32 / 7.0)],
                radius,
                feather: 50.0,
                mode,
                dust: false,
            }
        })
        .collect()
}

fn best_of(n: usize, mut f: impl FnMut()) -> f64 {
    (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_spot_cost <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let buffer = d.image;
    let (w, h) = (buffer.width(), buffer.height());
    let long = w.max(h) as f32;
    println!("buffer {w}x{h} ({:.1} Mpx)\n", (w * h) as f64 / 1e6);

    // The whole prefix with no spots, for scale.
    let plain = Edits::default();
    let whole = best_of(3, || {
        let steps = pipeline::build_prefix_steps(
            &plain,
            RenderScale::new(1.0),
            None,
            Resampler::default(),
            None,
            None,
        );
        let mut img = buffer.clone();
        for s in &steps {
            img = s.stage.apply(img);
        }
        std::hint::black_box(&img);
    });
    println!("  whole prefix, no spots, no edits: {whole:.2} ms  (the scale to read against)\n");

    for mode in [SpotMode::Clone, SpotMode::Heal] {
        println!("  {mode:?}");
        println!("    radius (px) |     1 spot     5      20     100   ms");
        for r in RADII {
            let px = r * long;
            print!("    {px:8.0}    |");
            for n in COUNTS {
                let st = pipeline::SpotRemovalStage::new(&spots(n, r, mode));
                let ms = best_of(3, || {
                    let out = st.apply(buffer.clone());
                    std::hint::black_box(&out);
                });
                print!(" {ms:7.2}");
            }
            println!();
        }
        println!();
    }

    // ⚠⚠ The clone of the buffer is INSIDE the timed region above, because `apply` consumes its
    // input — so these figures include one full-frame copy the real chain also pays. Priced here so
    // it is not mistaken for spot work.
    let copy = best_of(5, || {
        let c = buffer.clone();
        std::hint::black_box(&c);
    });
    println!("  ⚠ a bare buffer clone, included in every figure above: {copy:.2} ms");
    println!(
        "\n⚠ Spot removal is a PREFIX stage, so `PrefixCache` holds its output: this cost is paid\n\
         when the SPOT LIST changes, not on every slider drag. Read it as the cost of placing or\n\
         dragging a spot, not as a per-frame tax."
    );
}
