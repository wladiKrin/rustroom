//! Measures the local-contrast goldens rather than eyeballing them.
//!
//! ⚠ A golden nobody measured is a hash. This probe reports, on the exact chart and edits the
//! `texture_*` / `clarity_*` goldens are rendered from:
//!
//! - the coarse-band excursion `|d_c|` at the chart's hard edge versus in its texture, which is
//!   what `CLARITY_HALO_KNEE` is derived from;
//! - Clarity's **edge overshoot** (how far a pixel beside the edge moves past the flat band's own
//!   value) and its **fine-texture contrast**, with and without the halo suppressor;
//! - Texture's **edge-contrast retention** under −80, against a Gaussian base and a guided base
//!   at several `eps`, which is what `TEXTURE_GUIDED_EPS` is derived from.
//!
//! Run: `cargo run --release -p pipeline --example local_contrast_probe`
//!
//! ⚠ The chart is a copy of `local_contrast_chart()` in `tests/golden.rs`, which is private to
//! that test. If the two ever differ, the numbers here describe a different image from the one
//! in the reference.

use core_image::ImageBuf;
use edit_model::{Edits, LocalContrast};
use pipeline::spatial::{gaussian_blur, guided_filter, wide_blur, wide_blur_radius};
use pipeline::{build_pipeline, RenderScale};

const S: usize = 256;
/// The hard edge's first bright row.
const EDGE: usize = 3 * S / 4;
/// Interior columns, away from the left/right borders where the blurs clamp.
const X0: usize = 30;
const X1: usize = S - 30;

fn chart() -> ImageBuf {
    let mut img = ImageBuf::new(S, S);
    for y in 0..S {
        for x in 0..S {
            let coarse = 0.15 + 0.5 * (x as f32 / S as f32);
            let fine = 0.03 * (x as f32 / 6.0 * std::f32::consts::TAU).sin();
            let step = if y >= EDGE { 0.25 } else { 0.0 };
            let v = (coarse + fine + step).clamp(0.0, 1.0);
            img.set_pixel(x, y, color::srgb_to_prophoto([v, v * 0.85, v * 0.7]));
        }
    }
    img
}

fn luma_plane(img: &ImageBuf) -> Vec<f32> {
    img.data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| color::luminance(*p))
        .collect()
}

/// 8-bit level of the luma at a pixel, quantised the way the golden harness quantises.
fn level(v: f32) -> f32 {
    (v.clamp(0.0, 1.0) * 255.0).round()
}

/// Max over interior columns of the largest excursion past the flat band's value on each side
/// of the edge, in luma and in 8-bit levels. Each column is flat in `y` except the step, so the
/// source's own value in that column *is* the band's value.
fn overshoot(out: &ImageBuf, src: &ImageBuf) -> (f32, f32) {
    let (lo, ls) = (luma_plane(out), luma_plane(src));
    let (mut worst, mut worst_lv) = (0.0f32, 0.0f32);
    for x in X0..X1 {
        for y in 0..S {
            let i = y * S + x;
            // Bright side overshoots upward, dark side undershoots downward — both are halo.
            let e = if y >= EDGE {
                lo[i] - ls[i]
            } else {
                ls[i] - lo[i]
            };
            worst = worst.max(e);
            let elv = if y >= EDGE {
                level(lo[i]) - level(ls[i])
            } else {
                level(ls[i]) - level(lo[i])
            };
            worst_lv = worst_lv.max(elv);
        }
    }
    (worst, worst_lv)
}

/// RMS of the fine band along row `y`: the row minus its 6-sample (one period) running mean.
/// The ramp under the sine largely cancels in that difference, and it cancels identically for
/// every image compared here, so ratios of this number are about the sine alone.
fn fine_rms(img: &ImageBuf, y: usize) -> f32 {
    let l = luma_plane(img);
    let row = &l[y * S..(y + 1) * S];
    let mut sum = 0.0f32;
    let mut n = 0usize;
    for x in X0..X1 {
        let mean: f32 = row[x - 3..x + 3].iter().sum::<f32>() / 6.0;
        let d = row[x] - mean;
        sum += d * d;
        n += 1;
    }
    (sum / n as f32).sqrt()
}

/// Mean luma step across the edge — the row just below minus the row just above.
fn edge_contrast(img: &ImageBuf) -> f32 {
    let l = luma_plane(img);
    let mut sum = 0.0f32;
    for x in X0..X1 {
        sum += l[EDGE * S + x] - l[(EDGE - 1) * S + x];
    }
    sum / (X1 - X0) as f32
}

fn render(lc: LocalContrast) -> ImageBuf {
    let edits = Edits {
        local_contrast: lc,
        ..Default::default()
    };
    build_pipeline(&edits, RenderScale::FULL).run(chart())
}

fn main() {
    let src = chart();
    let neutral = render(LocalContrast::default());
    let l = luma_plane(&src);

    // --- 1. The coarse band's excursion at the edge and in texture (derives the knee). ---
    let base_c = wide_blur(&l, S, S, 40.0);
    let dc: Vec<f32> = l.iter().zip(&base_c).map(|(a, b)| a - b).collect();
    let mut edge_dc = 0.0f32;
    for x in X0..X1 {
        for y in [EDGE - 1, EDGE] {
            edge_dc = edge_dc.max(dc[y * S + x].abs());
        }
    }
    let (mut tex_dc_max, mut tex_dc_sum, mut n) = (0.0f32, 0.0f32, 0usize);
    for y in 20..EDGE - 130 {
        for x in X0..X1 {
            let v = dc[y * S + x].abs();
            tex_dc_max = tex_dc_max.max(v);
            tex_dc_sum += v;
            n += 1;
        }
    }
    println!(
        "coarse band |d_c|: at the edge {edge_dc:.4}, in texture max {tex_dc_max:.4} mean {:.4}",
        tex_dc_sum / n as f32
    );

    // --- 2. Clarity: overshoot and fine-texture retention, as the goldens render. ---
    for amount in [60.0f32, -60.0] {
        let out = render(LocalContrast {
            texture: 0.0,
            clarity: amount,
        });
        let (ov, ov_lv) = overshoot(&out, &neutral);
        let (f_src, f_out) = (fine_rms(&neutral, 100), fine_rms(&out, 100));
        println!(
            "clarity {amount:+.0}: edge overshoot {ov:.4} luma ({ov_lv:.0} levels); fine rms {f_src:.5} -> {f_out:.5} (x{:.3})",
            f_out / f_src
        );
    }

    // --- 3. Texture: edge retention under -80, as the golden renders. ---
    let out = render(LocalContrast {
        texture: -80.0,
        clarity: 0.0,
    });
    let (e_src, e_out) = (edge_contrast(&neutral), edge_contrast(&out));
    let (f_src, f_out) = (fine_rms(&neutral, 100), fine_rms(&out, 100));
    println!(
        "texture -80: edge contrast {e_src:.4} -> {e_out:.4} (retained {:.1}%); fine rms {f_src:.5} -> {f_out:.5} (x{:.3})",
        100.0 * e_out / e_src,
        f_out / f_src
    );

    // --- 4. The bases themselves: how much of the edge and of the texture each removes. ---
    // Measured on the luma plane directly: d = l - base, so the band's share of the edge step
    // and of the fine sine is what negative texture would subtract at k = -1.
    let edge_of = |base: &[f32]| {
        let mut sum = 0.0f32;
        for x in X0..X1 {
            let d_lo = l[EDGE * S + x] - base[EDGE * S + x];
            let d_hi = l[(EDGE - 1) * S + x] - base[(EDGE - 1) * S + x];
            sum += d_lo - d_hi;
        }
        sum / (X1 - X0) as f32
    };
    let fine_of = |base: &[f32]| {
        let y = 100;
        let mut sum = 0.0f32;
        for x in X0..X1 {
            let d = l[y * S + x] - base[y * S + x];
            sum += d * d;
        }
        (sum / (X1 - X0) as f32).sqrt()
    };
    let step = edge_contrast(&src);
    let fine = fine_rms(&src, 100);

    // ⚠ The criterion, stated once: at k the output is `l - k·band`, so the edge **retained**
    // is `1 - |k|·(band's share of the edge)` and the fine texture retained is the same with
    // the sine's share. The spec asks for edge retention within 5% at -70, and the largest
    // eps meeting it (the most texture-removing power) is the one to take.
    let report = |name: &str, base: &[f32]| {
        let (e, f) = (edge_of(base) / step, fine_of(base) / fine);
        println!(
            "{name:<26} band holds {:>5.1}% edge, {:>5.1}% fine  ->  at -70 keeps {:>5.1}% of the edge, {:>5.1}% of the texture",
            100.0 * e,
            100.0 * f,
            100.0 * (1.0 - 0.7 * e),
            100.0 * (1.0 - 0.7 * f)
        );
    };
    report("gaussian sigma 2:", &gaussian_blur(&l, S, S, 2.0));
    let r = wide_blur_radius(2.0).max(1);
    for eps in [3e-4f32, 5e-4, 6e-4, 7e-4, 8e-4, 9e-4, 1e-3, 3e-3] {
        report(
            &format!("guided r={r} eps={eps:.0e}:"),
            &guided_filter(&l, &l, S, S, r, eps),
        );
    }
}
