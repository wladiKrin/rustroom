//! Decomposes `HslStage`'s per-pixel cost, so the hue-LUT milestone is scoped from numbers
//! rather than from the colour-grading comparison — which is what got HSL's cost wrong twice.
//!
//! Run: `cargo run --release -p app --example bench_hsl <raw>`
//!
//! Each variant is CUMULATIVE: the delta between consecutive lines is that step's cost. Every
//! variant writes a genuinely different image, so nothing is dead-code eliminated.
use color::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use core_image::ImageBuf;
use edit_model::HslBand;
use std::time::Instant;

const PREVIEW_MAX_DIM: usize = 1600;
const BAND_HUES: [f32; 8] = [29.23, 52.98, 109.77, 142.50, 194.77, 264.05, 293.94, 328.36];
const CHROMA_FADE: f32 = 0.02;
const MAX_HUE_SHIFT_DEG: f32 = 30.0;

/// `hsl.rs`'s implementation, copied verbatim so the ablation measures the shipped code.
fn band_weights_current(h: f32) -> [f32; 8] {
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

/// Spans are constants of the table, and `delta` wants a conditional add rather than `fmodf`.
/// Returns only the live pair — two weights are ever non-zero, so the caller need not loop 8.
fn band_weights_cheap(h: f32) -> (usize, usize, f32) {
    const SPANS: [f32; 8] = {
        let mut s = [0.0f32; 8];
        let mut i = 0;
        while i < 8 {
            let next = (i + 1) % 8;
            let d = BAND_HUES[next] - BAND_HUES[i];
            s[i] = if d < 0.0 { d + 360.0 } else { d };
            i += 1;
        }
        s
    };
    let mut i = 0;
    while i < 8 {
        let d = h - BAND_HUES[i];
        let delta = if d < 0.0 { d + 360.0 } else { d };
        if delta < SPANS[i] {
            let t = delta / SPANS[i];
            return (i, (i + 1) % 8, t * t * (3.0 - 2.0 * t));
        }
        i += 1;
    }
    (0, 1, 0.0)
}

fn hue_of(a: f32, b: f32) -> f32 {
    let d = b.atan2(a).to_degrees();
    if d < 0.0 {
        d + 360.0
    } else {
        d
    }
}

/// The shipped kernel, copied from `stages/hsl.rs`. Step 7 proves it bit-identical.
fn full_kernel(image: &ImageBuf, bands: &[HslBand; 8]) -> ImageBuf {
    image.clone().map_pixels(|rgb| {
        let [l, a, b] = prophoto_to_oklab(rgb);
        let chroma = a.hypot(b);
        if chroma < 1e-6 {
            return rgb;
        }
        let hue = hue_of(a, b);
        let w = band_weights_current(hue);
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

    // 1. The harness floor: map_pixels plus rayon, doing essentially nothing.
    let t0 = bench("map_pixels identity (harness floor)", &|im| {
        im.clone().map_pixels(|p| [p[0] * 1.0000001, p[1], p[2]])
    });

    // 2. The Oklab round trip alone. ⚠ NOTHING proposed in this milestone touches this.
    let t1 = bench("+ Oklab round trip (3 cbrt in, 3 cubes out)", &|im| {
        im.clone()
            .map_pixels(|p| oklab_to_prophoto(prophoto_to_oklab(p)))
    });

    // 3a/3b. Chroma, two ways. `hypot` guards against an overflow Oklab chroma cannot reach.
    let t2 = bench("+ chroma via hypot()  [shipped]", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            oklab_to_prophoto([l + a.hypot(b) * 1e-6, a, b])
        })
    });
    let t2b = bench("+ chroma via (a*a+b*b).sqrt()", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            oklab_to_prophoto([l + (a * a + b * b).sqrt() * 1e-6, a, b])
        })
    });

    // 4. Hue via atan2 — needed to INDEX any hue LUT, so it cannot simply be deleted.
    let t3 = bench("+ hue via atan2()", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            let c = (a * a + b * b).sqrt();
            let h = hue_of(a, b);
            oklab_to_prophoto([l + (c + h) * 1e-6, a, b])
        })
    });

    // 5a/5b. Band weighting, two ways.
    let t4 = bench("+ band_weights, SHIPPED (fmod loop + 24 mul-adds)", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            let c = (a * a + b * b).sqrt();
            let h = hue_of(a, b);
            let w = band_weights_current(h);
            let (mut dh, mut sat, mut lum) = (0.0f32, 0.0f32, 0.0f32);
            for i in 0..8 {
                dh += w[i] * bands[i].hue;
                sat += w[i] * bands[i].saturation;
                lum += w[i] * bands[i].luminance;
            }
            oklab_to_prophoto([l + (c + dh + sat + lum) * 1e-6, a, b])
        })
    });
    let t4b = bench("+ band_weights, CHEAP (no fmod, 2 live bands)", &|im| {
        im.clone().map_pixels(|p| {
            let [l, a, b] = prophoto_to_oklab(p);
            let c = (a * a + b * b).sqrt();
            let h = hue_of(a, b);
            let (i, j, s) = band_weights_cheap(h);
            let (w0, w1) = (1.0 - s, s);
            let dh = w0 * bands[i].hue + w1 * bands[j].hue;
            let sat = w0 * bands[i].saturation + w1 * bands[j].saturation;
            let lum = w0 * bands[i].luminance + w1 * bands[j].luminance;
            oklab_to_prophoto([l + (c + dh + sat + lum) * 1e-6, a, b])
        })
    });

    // 6. The full shipped kernel: adds the chroma fade and the cos/sin rebuild.
    let t5 = bench(
        "+ chroma fade + cos/sin rebuild = FULL SHIPPED STAGE",
        &|im| full_kernel(im, &bands),
    );

    // 7. ⚠ CROSS-CHECK BY PIXELS, NOT BY TIMING. `pipeline::stages` is private, so the real
    // `HslStage` cannot be called from here — which is the better outcome: comparing OUTPUT
    // proves the kernel copied above is the one that ships, where a matching duration would
    // only have proved it costs about the same. The tail for HSL-only edits is [HslStage,
    // output transform] and the neutral tail is [output transform] alone, so our kernel
    // followed by the neutral tail must reproduce it exactly.
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
        let mine = tail_neutral.run(full_kernel(&img, &bands));
        let differing = real
            .data()
            .iter()
            .zip(mine.data())
            .filter(|(x, y)| x != y)
            .count();
        assert_eq!(
            differing, 0,
            "the copied kernel is NOT the shipped one — every number below describes something else"
        );
        println!("\n  cross-check: the copied kernel is bit-identical to the shipped stage");
    }

    println!("\n  --- what each step costs, above the harness floor ---");
    println!("  Oklab round trip (IRREDUCIBLE FLOOR) {:8.2} ms", t1 - t0);
    println!("  chroma: hypot                        {:8.2} ms", t2 - t1);
    println!("  chroma: sqrt                         {:8.2} ms", t2b - t1);
    println!("  hue: atan2                           {:8.2} ms", t3 - t2b);
    println!("  band weighting: shipped              {:8.2} ms", t4 - t3);
    println!("  band weighting: cheap                {:8.2} ms", t4b - t3);
    println!("  chroma fade + cos/sin                {:8.2} ms", t5 - t4);
    println!(
        "\n  full shipped stage above floor       {:8.2} ms",
        t5 - t0
    );
}
