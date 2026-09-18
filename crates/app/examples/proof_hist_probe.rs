//! **The measurement that blocked M6.4 soft proofing, kept so it can be re-run.**
//!
//! The soft-proofing design has `render_engine` pass `space: OutputSpace::ProPhoto` to the tail
//! while proofing — the documented bit-exact identity — so `pack` receives near-unclipped data,
//! and requires that `pack` still derive **byte-for-byte the same sRGB histogram** as it does
//! today. That is only reachable through a ProPhoto encode/decode round trip, and the spec says
//! in as many words to measure it rather than assume it.
//!
//! ```text
//! Proof OFF today:  tail(space = Srgb)     -> quantize                       -> histogram
//! Proof ON  (spec): tail(space = ProPhoto) -> ProPhoto decode -> Srgb encode -> histogram
//! ```
//!
//! ⚠⚠ **Measured 2026-09-04 over 12 real X-T4 frames: it does NOT hold, and 12 of 12 histograms
//! differ either way.** The two settings fail for two different reasons, which is why both are
//! run:
//!
//! ```text
//! rendering ON   213 of 61,516,800 samples move, worst 1 level   (147,127 clamped low, 0 high)
//! rendering OFF 1541 of 61,516,800 samples move, worst 5 levels  (2 clamped low, 5,776 high)
//! ```
//!
//! With the rendering **on** the moves are the irreducible f32 cost of `v^(1/1.8)` followed by
//! `^1.8`: about one sample in 290,000 sits close enough to an 8-bit rounding boundary to cross
//! it. With the rendering **off** the data is still scene-referred, so ProPhoto's `encode`
//! ceilings 5,776 samples of genuine headroom at 1.0 *before* the sRGB matrix mixes the channels,
//! where the direct path does not — **five levels is not rounding**, and the spec's "the display
//! rendering bounds the data before it, so the residue is negligible" does not hold for a photo
//! whose rendering is off, which is every golden, the CLI, and `Edits::default()`.
//!
//! ```text
//! cargo run --release -p app --example proof_hist_probe <raw>...
//! ```

use edit_model::{Edits, Rendering};
use pipeline::{RenderScale, RenderSpec};

const PROPHOTO_TOE_END: f32 = 0.001_953_125;

/// The inverse of `OutputSpace::ProPhoto.encode`.
fn prophoto_decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v < PROPHOTO_TOE_END * 16.0 {
        v / 16.0
    } else {
        v.powf(1.8)
    }
}

fn quantize(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

struct Tally {
    frames: usize,
    samples: u64,
    differing: u64,
    worst: u8,
    /// Samples ProPhoto's encode floored at 0 — a negative working channel, which the camera
    /// matrix produces on saturated colour and which the direct path feeds to the sRGB matrix.
    clamped_low: u64,
    /// Samples ProPhoto's encode ceilinged at 1 — scene-referred headroom above sensor white.
    clamped_high: u64,
    /// Frames whose 256-bucket histogram is not byte-identical.
    bad_frames: usize,
}

impl Tally {
    fn new() -> Self {
        Self {
            frames: 0,
            samples: 0,
            differing: 0,
            worst: 0,
            clamped_low: 0,
            clamped_high: 0,
            bad_frames: 0,
        }
    }
    fn report(&self, what: &str) {
        let pct = 100.0 * self.differing as f64 / self.samples.max(1) as f64;
        println!(
            "{what:<26} frames {:>3}  samples {:>10}  differing {:>7} ({pct:.6}%)  \
             worst |delta| {:>3}  clamped low {:>8}  clamped high {:>8}  \
             frames with a moved histogram {:>3}/{}",
            self.frames,
            self.samples,
            self.differing,
            self.worst,
            self.clamped_low,
            self.clamped_high,
            self.bad_frames,
            self.frames
        );
    }
}

fn run(paths: &[String], rendering: Rendering, label: &str) {
    let mut t = Tally::new();
    for f in paths {
        let path = std::path::PathBuf::from(f);
        let Ok(d) = raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::linear_scene())
        else {
            continue;
        };
        let buffer_long = d.image.width().max(d.image.height());
        let preview = d.image.downscale_to(1600);
        let scale = RenderScale::new(
            0.5 * preview.width().max(preview.height()) as f32 / buffer_long as f32,
        );

        let edits = Edits {
            rendering: Some(rendering),
            ..Edits::default()
        };

        let srgb_out = pipeline::build_pipeline_spec(
            &edits,
            scale,
            RenderSpec {
                space: color::OutputSpace::Srgb,
                ..RenderSpec::default()
            },
            pipeline::ProfileRendering::colorimetric(),
        )
        .run(preview.clone());
        let pp_out = pipeline::build_pipeline_spec(
            &edits,
            scale,
            RenderSpec {
                space: color::OutputSpace::ProPhoto,
                ..RenderSpec::default()
            },
            pipeline::ProfileRendering::colorimetric(),
        )
        .run(preview.clone());

        // ⚠ `clamped` counts ProPhoto-*encoded* samples that came back exactly 0 or 1. The tail
        // without its output transform is not reachable from here, so this is the closest proxy
        // for "the encode had to clip it" — and it over-counts, since a sample landing exactly on
        // the boundary is indistinguishable from one pushed there.
        let mut hist_a = [[0u32; 256]; 3];
        let mut hist_b = [[0u32; 256]; 3];
        let (srgb_px, _) = srgb_out.data().as_chunks::<3>();
        let (pp_px, _) = pp_out.data().as_chunks::<3>();
        for (pa, pb) in srgb_px.iter().zip(pp_px.iter()) {
            let recovered = [
                prophoto_decode(pb[0]),
                prophoto_decode(pb[1]),
                prophoto_decode(pb[2]),
            ];
            let lin = color::OutputSpace::Srgb.from_working(recovered);
            for c in 0..3 {
                if pb[c] <= 0.0 {
                    t.clamped_low += 1;
                } else if pb[c] >= 1.0 {
                    t.clamped_high += 1;
                }
                let a = quantize(pa[c]);
                let b = quantize(color::linear_to_srgb(lin[c]));
                hist_a[c][a as usize] += 1;
                hist_b[c][b as usize] += 1;
                t.samples += 1;
                if a != b {
                    t.differing += 1;
                    t.worst = t.worst.max(a.abs_diff(b));
                }
            }
        }
        t.frames += 1;
        if hist_a != hist_b {
            t.bad_frames += 1;
        }
    }
    t.report(label);
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    run(&paths, Rendering::Camera, "camera rendering ON");
    run(&paths, Rendering::None, "camera rendering OFF");
}
