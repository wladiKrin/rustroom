//! Decomposes what a **range mask's coverage** costs per pixel, so the milestone is scoped from
//! numbers rather than from a guess about which step dominates.
//!
//! Run: `cargo run --release -p app --example bench_range_mask <raw>`
//!
//! ⚠⚠ **This exists because the obvious answer is probably wrong, and this project has the record
//! to prove it.** The eye goes to `prophoto_to_oklab`'s three `cbrt` and to `atan2`. But
//! `ColourRange::coverage_at` calls **`rem_euclid` twice per pixel** — f32 `%` is `fmodf`, a libm
//! call — and recomputes `span` and `w` from *constants* on every pixel. That is precisely the
//! shape the hue LUT work found to be **42% of `HslStage`**, larger than its Oklab round trip,
//! after the cost had been mis-reasoned three times running.
//!
//! ⚠ **The kernels differ ONLY in the coverage computation.** Each does the same clone, the same
//! sub-stage run and the same blend traversal, so a difference between two rows is attributable to
//! the one step between them and to nothing else.
//!
//! ⚠⚠ **`full_kernel` is cross-checked against the real `MaskStage` and the run aborts if they
//! disagree.** Without that the ablation would silently describe a stage that no longer ships —
//! which is exactly what happened to `bench_hsl` the first time the stage changed under it.
use color::oklab::prophoto_to_oklab;
use core_image::ImageBuf;
use edit_model::{ColourRange, Geometry, LocalAdjustments, LuminanceRange, Mask, MaskedAdjustment};
use pipeline::{ExposureStage, MaskStage, PipelineStage, RenderScale};
use std::time::Instant;

const PREVIEW_MAX_DIM: usize = 1600;
const EXPOSURE: f32 = 0.5;

/// `mask.rs`'s own `hue_chroma`, verbatim.
fn hue_chroma(rgb: [f32; 3]) -> (f32, f32) {
    let [_, a, b] = prophoto_to_oklab(rgb);
    (b.atan2(a).to_degrees(), a.hypot(b))
}

/// `mask.rs`'s own `lightness`, verbatim.
fn lightness(rgb: [f32; 3]) -> f32 {
    color::linear_to_srgb(color::luminance(rgb)) * 100.0
}

/// The blend every kernel shares, parameterised by how coverage is obtained.
///
/// ⚠ `adjusted` is built once outside, so the sub-stage's own cost is identical in every row and
/// cancels out of every difference between rows.
fn blend<F: Fn([f32; 3]) -> f32 + Sync + Send>(
    image: &ImageBuf,
    adjusted: &ImageBuf,
    coverage: F,
) -> ImageBuf {
    image.map_pixels_xy(|x, y, base| {
        let c = coverage(base).clamp(0.0, 1.0);
        let adj = adjusted.pixel(x, y);
        [
            base[0] + (adj[0] - base[0]) * c,
            base[1] + (adj[1] - base[1]) * c,
            base[2] + (adj[2] - base[2]) * c,
        ]
    })
}

/// ⚠ Mirrors of `ColourRange`'s **private** associated constants. Deliberately copied rather than
/// made `pub`: opening them for one benchmark is the `pub mod stages` mistake `bench_hsl` records,
/// where a whole module would have become public API for the sake of a measurement. **The guard
/// that these copies are right is `hoist_drift` below** — a wrong constant makes the hoisted
/// kernel disagree with the shipped one, and the run asserts it does not.
const FEATHER_SCALE: f32 = 180.0;
const FEATHER_EPS: f32 = 1e-4;
const CHROMA_FADE: f32 = 0.02;

/// `coverage_at` with `span` and `w` hoisted out of the loop and both `rem_euclid` calls replaced
/// by a branch.
///
/// ⚠ **Equivalent, not approximate**, for the inputs that occur: `hue - low` lies in `-540..=540`
/// and `span` is already non-negative, so a single conditional add recovers what `rem_euclid`
/// computes. The point of the row is to price `fmodf`, so it must not change the answer.
struct HoistedRange {
    span: f32,
    w: f32,
    low: f32,
    min_chroma: f32,
    invert: bool,
}

impl HoistedRange {
    fn new(r: &ColourRange) -> Self {
        Self {
            span: (r.high - r.low).rem_euclid(360.0),
            w: (r.feather / 100.0).clamp(0.0, 1.0) * FEATHER_SCALE,
            low: r.low,
            min_chroma: r.min_chroma,
            invert: r.invert,
        }
    }

    fn coverage_at(&self, hue_degrees: f32, chroma: f32) -> f32 {
        let mut offset = hue_degrees - self.low;
        if offset < 0.0 {
            offset += 360.0;
        }
        if offset >= 360.0 {
            offset -= 360.0;
        }
        let band = if self.w <= FEATHER_EPS {
            if offset <= self.span {
                1.0
            } else {
                0.0
            }
        } else {
            let past = (offset - self.span).max(0.0);
            let before = (360.0 - offset).max(0.0);
            let t = (1.0 - past.min(before) / self.w).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let gate = ((chroma - self.min_chroma) / CHROMA_FADE).clamp(0.0, 1.0);
        let gate = gate * gate * (3.0 - 2.0 * gate);
        let c = band * gate;
        if self.invert {
            1.0 - c
        } else {
            c
        }
    }
}

fn best_of<F: FnMut() -> T, T>(mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let v = f();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(&v);
        best = best.min(ms);
    }
    best
}

fn max_diff(a: &ImageBuf, b: &ImageBuf) -> f32 {
    a.data()
        .iter()
        .zip(b.data().iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_range_mask <raw>");
    let path = std::path::Path::new(&path);
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let buf = d.image.downscale_to(PREVIEW_MAX_DIM);
    println!("image {}x{}\n", buf.width(), buf.height());

    let colour = ColourRange::default();
    let luma = LuminanceRange::default();
    let hoisted = HoistedRange::new(&colour);

    // Built once: identical in every row, so it cancels out of every difference between rows.
    let exposure = ExposureStage::new(EXPOSURE);
    let adjusted = exposure.apply(buf.clone());

    // ---- The cross-check, before any timing. ----
    let full = blend(&buf, &adjusted, |p| {
        let (h, c) = hue_chroma(p);
        colour.coverage_at(h, c)
    });
    let real = MaskStage::new(
        &MaskedAdjustment {
            mask: Mask::Colour(colour),
            adjustments: LocalAdjustments {
                exposure: EXPOSURE,
                ..Default::default()
            },
        },
        RenderScale::FULL,
        &Geometry::default(),
    )
    .apply(buf.clone());
    let drift = max_diff(&full, &real);
    assert!(
        drift == 0.0,
        "full_kernel is NOT bit-identical to MaskStage (max diff {drift}) — the ablation below \
         would describe something that does not ship. Fix the kernel before reading any number."
    );
    println!("✅ cross-check: full_kernel is bit-identical to MaskStage\n");

    let hoisted_out = blend(&buf, &adjusted, |p| {
        let (h, c) = hue_chroma(p);
        hoisted.coverage_at(h, c)
    });
    let hoist_drift = max_diff(&full, &hoisted_out);
    // ⚠⚠ This is the guard on the copied constants above, not a nicety. A wrong FEATHER_SCALE,
    // FEATHER_EPS or CHROMA_FADE makes the hoisted row price a *different* function, and the
    // "fmodf is free" or "fmodf is everything" conclusion would be drawn from it regardless.
    assert!(
        hoist_drift == 0.0,
        "the hoisted coverage_at is NOT bit-identical to the shipped one (max diff {hoist_drift:e}) \
         — most likely one of the mirrored constants is wrong. Its row would be meaningless."
    );
    println!("✅ cross-check: hoisted coverage_at is bit-identical to ColourRange::coverage_at\n");

    let rows: Vec<(&str, f64)> = vec![
        (
            "floor: traversal + blend only",
            best_of(|| blend(&buf, &adjusted, |_| 0.5)),
        ),
        (
            "+ oklab round trip only (3 cbrt)",
            best_of(|| {
                blend(&buf, &adjusted, |p| {
                    let [_, a, b] = prophoto_to_oklab(p);
                    (a + b).clamp(0.0, 1.0)
                })
            }),
        ),
        (
            "+ oklab + hypot (chroma)",
            best_of(|| {
                blend(&buf, &adjusted, |p| {
                    let [_, a, b] = prophoto_to_oklab(p);
                    a.hypot(b)
                })
            }),
        ),
        (
            "+ oklab + hypot + atan2 (hue_chroma)",
            best_of(|| {
                blend(&buf, &adjusted, |p| {
                    let (h, c) = hue_chroma(p);
                    (h + c) * 0.0 + 0.5
                })
            }),
        ),
        (
            "FULL colour range (as it ships)",
            best_of(|| {
                blend(&buf, &adjusted, |p| {
                    let (h, c) = hue_chroma(p);
                    colour.coverage_at(h, c)
                })
            }),
        ),
        (
            "full, but coverage_at hoisted (no fmodf)",
            best_of(|| {
                blend(&buf, &adjusted, |p| {
                    let (h, c) = hue_chroma(p);
                    hoisted.coverage_at(h, c)
                })
            }),
        ),
        (
            "LUMINANCE range (as it ships)",
            best_of(|| blend(&buf, &adjusted, |p| luma.coverage(lightness(p)))),
        ),
        (
            "luminance, without the linear_to_srgb powf",
            best_of(|| {
                blend(&buf, &adjusted, |p| {
                    luma.coverage(color::luminance(p) * 100.0)
                })
            }),
        ),
    ];

    let floor = rows[0].1;
    println!("{:<42} {:>9} {:>12}", "kernel", "ms", "above floor");
    for (label, ms) in &rows {
        println!("{label:<42} {ms:9.2} {:12.2}", ms - floor);
    }

    // ---- Is a cheap chroma pre-gate worth anything? ----
    //
    // ⚠ The gate zeroes coverage below `min_chroma`, so a pixel that can be *proved* below it
    // needs no hue at all. The question is how many pixels that is on a real photograph, and
    // whether a cheap RGB proxy can prove it without the Oklab round trip.
    let n = buf.width() * buf.height();
    let mut below = 0usize;
    let mut proxy_below = 0usize;
    let mut proxy_wrong = 0usize;
    let mut worst_ratio = 0.0f32;
    for y in 0..buf.height() {
        for x in 0..buf.width() {
            let p = buf.pixel(x, y);
            let (_, chroma) = hue_chroma(p);
            let mx = p[0].max(p[1]).max(p[2]);
            let mn = p[0].min(p[1]).min(p[2]);
            let spread = mx - mn;
            if chroma < colour.min_chroma {
                below += 1;
            }
            // A conservative proxy: if the raw channel spread is tiny the Oklab chroma must be
            // too. `worst_ratio` measures how conservative the constant would have to be.
            if spread < 0.01 {
                proxy_below += 1;
                if chroma >= colour.min_chroma {
                    proxy_wrong += 1;
                }
            }
            if spread > 1e-6 {
                worst_ratio = worst_ratio.max(chroma / spread);
            }
        }
    }
    println!(
        "\nchroma pre-gate feasibility (min_chroma {:.3}):",
        colour.min_chroma
    );
    println!(
        "  pixels genuinely below min_chroma: {below} of {n} ({:.1}%)",
        below as f64 * 100.0 / n as f64
    );
    println!(
        "  rejected by a raw spread < 0.01 proxy: {proxy_below} ({:.1}%), of which WRONG: {proxy_wrong}",
        proxy_below as f64 * 100.0 / n as f64
    );
    println!("  worst observed chroma/spread ratio: {worst_ratio:.3}");
    println!(
        "\n⚠ A pre-gate only pays if the 'genuinely below' share is large. If it is small on real\n\
         photographs, the gate adds a branch to every pixel and skips almost none of them."
    );
}
