//! Decomposes `HslStage`'s cost and sizes its hue table, so the milestone is scoped from numbers
//! rather than from the colour-grading comparison — which is what got HSL's cost wrong three times.
//!
//! Run: `cargo run --release -p app --example bench_hsl <raw>`
//!
//! Three kernels live here and each answers a different question:
//!
//! - **`orig_kernel`** — the code as it shipped before this milestone. Kept as the baseline the
//!   ablation decomposes, and as the "did a photograph change?" reference.
//! - **`exact_kernel`** — the current maths with the bands evaluated **directly**, no table. The
//!   ground truth the table's drift is measured against, so the sweep isolates the table's error
//!   from the rotation's.
//! - **`lut_kernel`** — what actually ships, parameterised by table size. ⚠⚠ The cross-check
//!   asserts it is bit-identical to the real `HslStage`; without it the sweep would silently
//!   describe something else, which is exactly what happened the first time this example was run
//!   after the stage changed.
use color::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use core_image::ImageBuf;
use edit_model::HslBand;
use pipeline::hue_table::{band_weights, cos_small, sin_small, HueTable, MAX_HUE_SHIFT_DEG};
use std::time::Instant;

const PREVIEW_MAX_DIM: usize = 1600;
const BAND_HUES: [f32; 8] = [29.23, 52.98, 109.77, 142.50, 194.77, 264.05, 293.94, 328.36];
const CHROMA_FADE: f32 = 0.02;

/// `hsl.rs`'s band weighting **as it shipped before this milestone**, `fmodf` and all. Kept so
/// the ablation measures the code the spec's table describes.
fn band_weights_orig(h: f32) -> [f32; 8] {
    let mut w = [0.0f32; 8];
    let n = BAND_HUES.len();
    for i in 0..n {
        let next = (i + 1) % n;
        let span = (BAND_HUES[next] - BAND_HUES[i] + 360.0) % 360.0;
        let delta = (h - BAND_HUES[i] + 360.0) % 360.0;
        if delta < span {
            let t = delta / span;
            let s = t * t * (3.0 - 2.0 * t);
            w[i] = 1.0 - s;
            w[next] = s;
            return w;
        }
    }
    w[0] = 1.0;
    w
}

fn hue_of(a: f32, b: f32) -> f32 {
    let d = b.atan2(a).to_degrees();
    if d < 0.0 {
        d + 360.0
    } else {
        d
    }
}

/// Weighted band parameters at `hue`, evaluated directly — the table's ground truth.
fn params_direct(hue: f32, bands: &[HslBand; 8]) -> [f32; 3] {
    let w = band_weights(hue);
    let mut p = [0.0f32; 3];
    for i in 0..8 {
        p[0] += w[i] * bands[i].hue;
        p[1] += w[i] * bands[i].saturation;
        p[2] += w[i] * bands[i].luminance;
    }
    p
}

/// The code as it shipped before this milestone.
fn orig_kernel(image: &ImageBuf, bands: &[HslBand; 8]) -> ImageBuf {
    image.clone().map_pixels(|rgb| {
        let [l, a, b] = prophoto_to_oklab(rgb);
        let chroma = a.hypot(b);
        if chroma < 1e-6 {
            return rgb;
        }
        let hue = hue_of(a, b);
        let w = band_weights_orig(hue);
        let (mut dh, mut sat, mut lum) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..8 {
            dh += w[i] * bands[i].hue;
            sat += w[i] * bands[i].saturation;
            lum += w[i] * bands[i].luminance;
        }
        let t = (chroma / CHROMA_FADE).min(1.0);
        let strength = t * t * (3.0 - 2.0 * t);
        dh *= strength;
        sat *= strength;
        lum *= strength;
        let hue2 = (hue + dh * MAX_HUE_SHIFT_DEG / 100.0).to_radians();
        let chroma2 = (chroma * (1.0 + sat / 100.0)).max(0.0);
        let l2 = (l * (1.0 + lum / 200.0)).max(0.0);
        let out = oklab_to_prophoto([l2, chroma2 * hue2.cos(), chroma2 * hue2.sin()]);
        [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
    })
}

/// The fade and rotation exactly as they now ship, applied to band parameters from anywhere.
#[inline]
fn finish(l: f32, a: f32, b: f32, chroma: f32, p: [f32; 3]) -> [f32; 3] {
    let t = (chroma / CHROMA_FADE).min(1.0);
    let strength = t * t * (3.0 - 2.0 * t);
    let (dh, sat, lum) = (p[0] * strength, p[1] * strength, p[2] * strength);
    let delta = (dh * MAX_HUE_SHIFT_DEG / 100.0).to_radians();
    let (cd, sd) = (cos_small(delta), sin_small(delta));
    let k = (1.0 + sat / 100.0).max(0.0);
    let l2 = (l * (1.0 + lum / 200.0)).max(0.0);
    let out = oklab_to_prophoto([l2, k * (a * cd - b * sd), k * (a * sd + b * cd)]);
    [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
}

/// The current maths with the bands evaluated directly — no table, so no sampling error.
fn exact_kernel(image: &ImageBuf, bands: &[HslBand; 8]) -> ImageBuf {
    image.clone().map_pixels(|rgb| {
        let [l, a, b] = prophoto_to_oklab(rgb);
        let chroma = (a * a + b * b).sqrt();
        if chroma < 1e-6 {
            return rgb;
        }
        finish(l, a, b, chroma, params_direct(hue_of(a, b), bands))
    })
}

/// What ships, against an explicitly sized table.
fn lut_kernel(image: &ImageBuf, table: &HueTable) -> ImageBuf {
    image.clone().map_pixels(|rgb| {
        let [l, a, b] = prophoto_to_oklab(rgb);
        let chroma = (a * a + b * b).sqrt();
        if chroma < 1e-6 {
            return rgb;
        }
        finish(l, a, b, chroma, table.sample(hue_of(a, b)))
    })
}

/// Worst and mean 8-bit difference between two renders, plus the share of samples that differ.
fn drift(a: &ImageBuf, b: &ImageBuf) -> (i32, f64, f64) {
    let (x, y) = (a.to_rgb8(), b.to_rgb8());
    let (mut worst, mut sum, mut differing) = (0i32, 0u64, 0usize);
    for (p, q) in x.iter().zip(y.iter()) {
        let d = (*p as i32 - *q as i32).abs();
        worst = worst.max(d);
        sum += d as u64;
        if d != 0 {
            differing += 1;
        }
    }
    (
        worst,
        sum as f64 / x.len() as f64,
        differing as f64 * 100.0 / x.len() as f64,
    )
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: bench_hsl <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let img = d.image.downscale_to(PREVIEW_MAX_DIM);
    println!("image {}x{}\n", img.width(), img.height());

    let mut bands = edit_model::Hsl::default().bands;
    bands[0].saturation = 30.0;

    let bench = |label: &str, f: &dyn Fn(&ImageBuf) -> ImageBuf| {
        let mut best = f64::MAX;
        for _ in 0..7 {
            let t = Instant::now();
            let out = f(&img);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&out);
            best = best.min(ms);
        }
        println!("{best:8.2} ms   {label}");
        best
    };

    // --- The ablation, against the ORIGINAL kernel. Each variant is cumulative, so consecutive
    // deltas are that step's cost. Every variant writes a different image, so nothing is
    // dead-code eliminated.
    let t0 = bench("map_pixels identity (harness floor)", &|im| {
        im.clone().map_pixels(|p| [p[0] * 1.0000001, p[1], p[2]])
    });
    let t1 = bench("+ Oklab round trip (3 cbrt in, 3 cubes out)", &|im| {
        im.clone()
            .map_pixels(|p| oklab_to_prophoto(prophoto_to_oklab(p)))
    });
    let t2 = bench("+ chroma via hypot()  [was shipped]", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            oklab_to_prophoto([l + a.hypot(b) * 1e-6, a, b])
        })
    });
    let t2b = bench("+ chroma via (a*a+b*b).sqrt()  [ships]", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            oklab_to_prophoto([l + (a * a + b * b).sqrt() * 1e-6, a, b])
        })
    });
    let t3 = bench("+ hue via atan2()", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            let c = (a * a + b * b).sqrt();
            oklab_to_prophoto([l + (c + hue_of(a, b)) * 1e-6, a, b])
        })
    });
    let t4 = bench("+ band weighting, ORIGINAL (fmod + 24 mul-adds)", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            let c = (a * a + b * b).sqrt();
            let h = hue_of(a, b);
            let w = band_weights_orig(h);
            let (mut dh, mut sat, mut lum) = (0.0f32, 0.0f32, 0.0f32);
            for i in 0..8 {
                dh += w[i] * bands[i].hue;
                sat += w[i] * bands[i].saturation;
                lum += w[i] * bands[i].luminance;
            }
            oklab_to_prophoto([l + (c + dh + sat + lum) * 1e-6, a, b])
        })
    });
    let t5 = bench("= FULL ORIGINAL STAGE (+ fade + cos/sin)", &|im| {
        orig_kernel(im, &bands)
    });
    let t6 = bench("  the stage as it SHIPS NOW (table + identity)", &|im| {
        lut_kernel(im, &HueTable::build(&bands, 1024))
    });

    // --- ⚠⚠ CROSS-CHECK BY PIXELS, NOT BY TIMING.
    {
        use pipeline::{build_pipeline_split, RenderScale};
        let sc = RenderScale::new(1.0);
        let hsl = edit_model::Hsl { bands };
        let (_, tail_hsl) = build_pipeline_split(
            &edit_model::Edits {
                hsl,
                ..Default::default()
            },
            sc,
        );
        let (_, tail_neutral) = build_pipeline_split(&edit_model::Edits::default(), sc);
        let real = tail_hsl.run(img.clone());
        let mine = tail_neutral.run(lut_kernel(&img, &HueTable::build(&bands, 1024)));
        let differing = real
            .data()
            .iter()
            .zip(mine.data())
            .filter(|(x, y)| x != y)
            .count();
        assert_eq!(
            differing, 0,
            "lut_kernel is NOT the shipped stage — every number below describes something else"
        );
        println!("\n  cross-check: lut_kernel is bit-identical to the shipped HslStage");
    }

    // --- ⚠⚠ SIZING THE TABLE. Two criteria, not one: drift below half an 8-bit level so no
    // golden can move, AND no cost regression — at 4096 entries the table is 48 KB, a large slice
    // of L1, and a table too big to stay cached can be slower than computing the thing.
    // Measured against `exact_kernel`, so this isolates the TABLE's error from the rotation's.
    {
        let exact = exact_kernel(&img, &bands);
        println!("\n     n | worst 8-bit | mean 8-bit | samples differing |    cost");
        for n in [64usize, 128, 256, 512, 1024, 2048, 4096] {
            let t = HueTable::build(&bands, n);
            let mut best = f64::MAX;
            for _ in 0..7 {
                let s = Instant::now();
                let o = lut_kernel(&img, &t);
                best = best.min(s.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(&o);
            }
            let (worst, mean, pct) = drift(&exact, &lut_kernel(&img, &t));
            println!("{n:6} | {worst:11} | {mean:10.5} | {pct:16.4}% | {best:6.2} ms");
        }
    }

    // --- Did a PHOTOGRAPH change? The goldens answer that for synthetic charts; this answers it
    // for real pixels, against the code as it shipped before the milestone.
    {
        let (worst, mean, pct) = drift(
            &orig_kernel(&img, &bands),
            &lut_kernel(&img, &HueTable::build(&bands, 1024)),
        );
        println!(
            "\n  whole milestone vs the original kernel, on a real frame:\n    worst {worst} of 255, mean {mean:.5}, {pct:.4}% of samples differing"
        );
    }

    println!("\n  --- what each step cost in the ORIGINAL, above the harness floor ---");
    println!("  Oklab round trip (IRREDUCIBLE FLOOR) {:8.2} ms", t1 - t0);
    println!("  chroma: hypot                        {:8.2} ms", t2 - t1);
    println!("  chroma: sqrt                         {:8.2} ms", t2b - t1);
    println!("  hue: atan2                           {:8.2} ms", t3 - t2b);
    println!("  band weighting: original             {:8.2} ms", t4 - t3);
    println!("  chroma fade + cos/sin                {:8.2} ms", t5 - t4);
    println!(
        "\n  ORIGINAL stage above floor           {:8.2} ms",
        t5 - t0
    );
    println!(
        "  SHIPPED stage above floor            {:8.2} ms   ({:.2}x)",
        t6 - t0,
        (t5 - t0) / (t6 - t0)
    );
}
