//! Measure what lens correction did to a real photograph, by radius.
//!
//! ⚠ The number this exists to interrogate is the **corner vignetting lift**. The XF23 profile
//! asks for ~4.79x at the extreme corner — 2.3 stops — which is plausible for a fast prime wide
//! open and is also the single most likely thing in the profile to be wrong. A whole-image mean
//! would hide it entirely, so this reports by distance from the centre.
//!
//! `cargo run --release -p app --example lens_diff <off.jpg> <on.jpg>`
fn main() {
    let mut a = std::env::args().skip(1);
    let off = image::open(a.next().expect("usage: lens_diff <off> <on>"))
        .expect("open off")
        .to_rgb8();
    let on = image::open(a.next().expect("usage: lens_diff <off> <on>"))
        .expect("open on")
        .to_rgb8();
    assert_eq!(off.dimensions(), on.dimensions(), "sizes differ");

    let (w, h) = off.dimensions();
    let (cx, cy) = ((w - 1) as f32 * 0.5, (h - 1) as f32 * 0.5);
    let rmax = cx.hypot(cy);

    // Five radial bands, centre outward.
    const N: usize = 5;
    let mut sum_off = [0f64; N];
    let mut sum_on = [0f64; N];
    let mut n = [0u64; N];

    for y in 0..h {
        for x in 0..w {
            let r = (x as f32 - cx).hypot(y as f32 - cy) / rmax;
            let b = ((r * N as f32) as usize).min(N - 1);
            let p = off.get_pixel(x, y).0;
            let q = on.get_pixel(x, y).0;
            sum_off[b] += (p[0] as u32 + p[1] as u32 + p[2] as u32) as f64 / 3.0;
            sum_on[b] += (q[0] as u32 + q[1] as u32 + q[2] as u32) as f64 / 3.0;
            n[b] += 1;
        }
    }

    println!(
        "{:>10}  {:>9}  {:>9}  {:>7}",
        "radius", "off", "on", "ratio"
    );
    for b in 0..N {
        let (o, t) = (sum_off[b] / n[b] as f64, sum_on[b] / n[b] as f64);
        println!(
            "{:>4.2}-{:<4.2}  {o:9.2}  {t:9.2}  {:>7.3}",
            b as f32 / N as f32,
            (b + 1) as f32 / N as f32,
            t / o.max(1e-9)
        );
    }
}
