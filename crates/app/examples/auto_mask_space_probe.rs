//! M1 for auto-mask: how far apart are the space the reference is sampled in and the space the
//! pixel is compared in?
//!
//! The auto-mask reference is sampled from the **displayed** frame — after the display rendering
//! S-curve, the creative look and the output transform. The pixel it is compared against is
//! `MaskStage`'s input, **before** all three. Both convert to Oklab, which is absolute, but the
//! S-curve has moved one of them, and the gate is a *threshold*, so a systematic offset in L
//! biases it.
//!
//! ⚠ Run on a **neutral** `Edits`. Every global tail stage is then inactive and therefore not
//! inserted, so the prefix output *is* the mask input. That is what makes this measurable with the
//! existing split builder and no new pipeline seam.
//!
//! ⚠ Bucketed by displayed lightness rather than pooled. A whole-image mean is the wrong statistic
//! for a question about a curve that compresses the ends — this file's record has been misled that
//! way twice already (the HueSatMap and colour-grading work).
//!
//! Run: `cargo run --release -p app --example auto_mask_space_probe <raw>`
use edit_model::Edits;

const MAX_DIM: usize = 1600;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: auto_mask_space_probe <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let decoded =
        raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode failed");
    let img = decoded.image.downscale_to(MAX_DIM);
    let scale = pipeline::RenderScale::new(1.0);

    // ⚠⚠ **Both configurations, because they are genuinely different questions and measuring only
    // the first is measuring nothing.** `Edits::default()` carries `rendering: None`, so its tail
    // holds no display rendering at all and the two ends agree to quantization error by
    // construction — that is the CLI's and the goldens' case. The **canvas** opens every photo
    // with `Settings::default_rendering`, which is `Camera`, so the S-curve really does sit
    // between the mask input and the displayed frame. The second row is the one the design turns
    // on.
    for (label, rendering) in [
        ("rendering off (CLI, goldens)", None),
        (
            "rendering on  (what the canvas opens)",
            Some(edit_model::Rendering::Camera),
        ),
    ] {
        let edits = Edits {
            rendering,
            ..Edits::default()
        };
        run(&img, scale, &edits, &path, label);
    }
}

fn run(
    img: &core_image::ImageBuf,
    scale: pipeline::RenderScale,
    edits: &Edits,
    path: &str,
    label: &str,
) {
    // End A: what `MaskStage` sees — the prefix output, which on neutral edits is the mask input.
    let (prefix, tail) = pipeline::build_pipeline_split(edits, scale);
    let mut base = img.clone();
    for stage in prefix.into_stages() {
        base = stage.apply(base);
    }

    // End B: what the canvas samples — the displayed frame.
    let mut shown = base.clone();
    for stage in tail.into_stages() {
        shown = stage.apply(shown);
    }

    // ⚠ The displayed frame reaches the canvas as sRGB **bytes**, so it is round-tripped through
    // the same quantization `sample_reference` will read back. Anything else would measure a
    // buffer nobody samples.
    let mut buckets = vec![(0f64, 0f64, 0f64, 0usize); 10];
    for y in 0..base.height() {
        for x in 0..base.width() {
            let a = color::oklab::prophoto_to_oklab(base.pixel(x, y));
            let s = shown.pixel(x, y);
            // ⚠⚠ `OutputTransform` ALREADY encodes (`space.encode`), so `shown` holds
            // display-encoded values. Encoding again here is a double-encode that reads as a
            // +0.24 lightness offset — a probe bug wearing the shape of a finding. Only quantize.
            let q = [
                ((s[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8 as f32) / 255.0,
                ((s[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8 as f32) / 255.0,
                ((s[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8 as f32) / 255.0,
            ];
            let b = color::linear_srgb_to_oklab([
                color::srgb_to_linear(q[0]),
                color::srgb_to_linear(q[1]),
                color::srgb_to_linear(q[2]),
            ]);

            let bucket = ((b[0] * 10.0) as usize).min(9);
            let dl = (b[0] - a[0]) as f64;
            let dc = (((b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()) as f64;
            // The full distance is what the gate actually thresholds on.
            let de = (((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2))
                .sqrt()) as f64;
            buckets[bucket].0 += dl;
            buckets[bucket].1 += dc;
            buckets[bucket].2 += de;
            buckets[bucket].3 += 1;
        }
    }

    println!("=== {path}  |  {label}");
    println!("displayed L | mean dL      | mean chroma dist | mean full dE | samples");
    for (i, (dl, dc, de, n)) in buckets.iter().enumerate() {
        if *n == 0 {
            continue;
        }
        let n_f = *n as f64;
        println!(
            "{:.1}-{:.1}     | {:+.4}       | {:.4}           | {:.4}       | {}",
            i as f32 / 10.0,
            (i + 1) as f32 / 10.0,
            dl / n_f,
            dc / n_f,
            de / n_f,
            n
        );
    }

    // The decision turns on the middle of the range, where real subjects sit.
    let (mid_dl, mid_de, mid_n) = buckets[2..8].iter().fold((0.0, 0.0, 0usize), |acc, b| {
        (acc.0 + b.0, acc.1 + b.2, acc.2 + b.3)
    });
    if mid_n > 0 {
        println!(
            "\nmiddle six buckets: mean dL {:+.4}, mean full dE {:.4}, over {} samples",
            mid_dl / mid_n as f64,
            mid_de / mid_n as f64,
            mid_n
        );
    }
}
