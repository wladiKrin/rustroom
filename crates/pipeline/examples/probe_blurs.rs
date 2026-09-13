//! What the three blur kinds behind `local_contrast` cost, and at what radii.
//!
//! ⚠⚠ **This is the measurement that decides how local contrast is ported.** The stage's own
//! total (see `probe_stages`) says only that it is worth porting; it does not say what to build.
//! The answer is that the "three blur kinds" framing overstates the work: `wide_blur` is three
//! iterated `box_blur` passes and `guided_filter` is six, so **`box_blur` is the only genuinely
//! new GPU primitive** — `gaussian_blur` was already ported for sharpening at M8.3c.
//!
//! ⚠ And the two consumers ask for radically different radii, which is the whole difficulty:
//! Clarity's sigma of 40 gives r = 40, while Texture's guided base gives r = 2. A GPU box blur
//! is O(r) per pixel where the CPU's running sum is O(1), so the two land in different regimes.
//!
//! Run: `cargo run --release -p pipeline --example probe_blurs`
use pipeline::spatial::{
    box_blur, gaussian_blur, guided_filter, kernel_radius, wide_blur, wide_blur_radius,
};
use std::time::Instant;

/// Best of five, so a scheduling hiccup cannot be read as a cost.
fn time(mut f: impl FnMut() -> Vec<f32>) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let out = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&out);
    }
    best
}

fn main() {
    let (w, h) = (3200usize, 2136);
    let n = w * h;
    // A plane with real structure at both bands, so no pass is measured on a degenerate input.
    let plane: Vec<f32> = (0..n)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            0.4 + 0.1 * (x / 128.0).sin() + 0.02 * (x / 6.0).sin() + 0.05 * (y / 200.0).cos()
        })
        .collect();

    println!("{w}x{h} ({:.1} Mpx), plane = {:.1} MB", n as f64 / 1e6, (n * 4) as f64 / 1e6);
    println!();
    println!("radii the stage actually asks for, at RenderScale::FULL:");
    println!("  texture  gaussian_blur(sigma 2)   -> kernel_radius   {:>3} ({} taps)", kernel_radius(2.0), 2 * kernel_radius(2.0) + 1);
    println!("  texture  guided_filter            -> box radius      {:>3} ({} taps)", wide_blur_radius(2.0).max(1), 2 * wide_blur_radius(2.0).max(1) + 1);
    println!("  clarity  wide_blur(sigma 40)      -> box radius      {:>3} ({} taps)", wide_blur_radius(40.0), 2 * wide_blur_radius(40.0) + 1);
    println!();

    let r_clarity = wide_blur_radius(40.0);
    let r_tex = wide_blur_radius(2.0).max(1);

    println!("primitive costs (best of 5):");
    let t = time(|| gaussian_blur(&plane, w, h, 2.0));
    println!("  gaussian_blur(sigma 2)        {t:6.2} ms   <- already on the GPU (sharpen.rs)");
    let t = time(|| box_blur(&plane, w, h, r_clarity));
    println!("  box_blur(r {r_clarity})                 {t:6.2} ms   <- x3 inside wide_blur");
    let t = time(|| box_blur(&plane, w, h, r_tex));
    println!("  box_blur(r {r_tex})                  {t:6.2} ms   <- x6 inside guided_filter");
    let t = time(|| wide_blur(&plane, w, h, 40.0));
    println!("  wide_blur(sigma 40)           {t:6.2} ms   <- clarity's base");
    let t = time(|| guided_filter(&plane, &plane, w, h, r_tex, 2e-4));
    println!("  guided_filter(r {r_tex}, self)       {t:6.2} ms   <- negative texture's base");
    println!();

    // ⚠⚠ Is the self-guided case doing twice the box blurs it needs? When `p == guide`,
    // `mean_p` IS `mean_guide` and `cross` IS `guide_sq`, so two of the four stat blurs are
    // recomputing a value already in hand. This times the reduced form to see whether that is
    // worth anything — it would speed up noise reduction too, which is also self-guided.
    let t_full = time(|| guided_filter(&plane, &plane, w, h, r_tex, 2e-4));
    let t_self = time(|| self_guided(&plane, w, h, r_tex, 2e-4));
    println!("self-guided, duplicate blurs removed:");
    println!("  guided_filter  (4 stat blurs)  {t_full:6.2} ms");
    println!("  self_guided    (2 stat blurs)  {t_self:6.2} ms   ({:+.1}%)", (t_self / t_full - 1.0) * 100.0);
    let a = guided_filter(&plane, &plane, w, h, r_tex, 2e-4);
    let b = self_guided(&plane, w, h, r_tex, 2e-4);
    let worst = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    println!("  worst difference between them: {worst:e}   (0 means bit-identical)");
}

/// [`guided_filter`] with `p == guide`, skipping the two box blurs that recompute a value the
/// other two already have. Same arithmetic, so it must be bit-identical.
fn self_guided(g: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let g_sq: Vec<f32> = g.iter().map(|v| v * v).collect();
    let mean_g = box_blur(g, w, h, r);
    let mean_g_sq = box_blur(&g_sq, w, h, r);
    let mut slope = Vec::with_capacity(g.len());
    let mut offset = Vec::with_capacity(g.len());
    for (&mg, &mgsq) in mean_g.iter().zip(&mean_g_sq) {
        // ⚠⚠ `cov` and `var` are the SAME expression here, but `var` is clamped at 0 and `cov`
        // deliberately is not — `guided_filter`'s own comment says why. On a flat area where
        // cancellation pushes the value slightly negative the two differ, and writing
        // `s = var / (var + eps)` silently drops the small negative slope. Measured: that
        // mistake costs 4.7e-6 of divergence, which is how it was caught.
        let cov = mgsq - mg * mg;
        let var = cov.max(0.0);
        let s = cov / (var + eps);
        slope.push(s);
        offset.push(mg - s * mg);
    }
    let mean_slope = box_blur(&slope, w, h, r);
    let mean_offset = box_blur(&offset, w, h, r);
    mean_slope
        .iter()
        .zip(&mean_offset)
        .zip(g)
        .map(|((&ms, &mo), &gv)| ms * gv + mo)
        .collect()
}
