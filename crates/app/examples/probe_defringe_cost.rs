//! M9.6: what the defringe step costs at the fit rung — global only, and with one masked local —
//! so whether it needs a GPU twin is decided on a number.
//!
//! ```text
//! cargo run --release -p app --example probe_defringe_cost <raw> [long-edge, default 3200]
//! ```
use core_image::ImageBuf;
use pipeline::DefringeStage;
use pipeline::MaskStage;
use pipeline::{PipelineStage, RenderScale};

fn median_ms(mut f: impl FnMut()) -> f64 {
    let mut t: Vec<f64> = (0..15)
        .map(|_| {
            let s = std::time::Instant::now();
            f();
            s.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    t.sort_by(|a, b| a.total_cmp(b));
    t[t.len() / 2]
}

fn main() {
    let mut a = std::env::args().skip(1);
    let raw = a
        .next()
        .expect("usage: probe_defringe_cost <raw> [long-edge]");
    let long: usize = a.next().map_or(3200, |s| s.parse().expect("a number"));
    let d = raw_decode::decode_raw(
        std::path::Path::new(&raw),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode");
    let full_long = d.image.width().max(d.image.height());
    let img: ImageBuf = d.image.downscale_to(long);
    let scale = RenderScale::new(img.width().max(img.height()) as f32 / full_long as f32);
    println!(
        "{}x{} ({:.1} Mpx), scale {:.3}",
        img.width(),
        img.height(),
        (img.width() * img.height()) as f64 / 1e6,
        scale.get()
    );
    let global = edit_model::Defringe {
        purple_amount: 50.0,
        green_amount: 50.0,
        ..Default::default()
    };
    let g = DefringeStage::new(global, Vec::new(), scale);
    println!(
        "global only:        {:>7.1} ms",
        median_ms(|| drop(g.apply(img.clone())))
    );
    let clone = median_ms(|| drop(img.clone()));
    println!("  (of which clone:  {clone:>7.1} ms)");

    let m = edit_model::MaskedAdjustment {
        mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
        refine: Vec::new(),
        adjustments: edit_model::LocalAdjustments {
            defringe: -100.0,
            ..Default::default()
        },
        disabled: false,
    };
    let stage = MaskStage::new_capture(&m, scale, &edit_model::Geometry::default(), &[], None);
    let with_mask = DefringeStage::new(global, vec![(stage, -100.0)], scale);
    println!(
        "global + 1 mask:    {:>7.1} ms",
        median_ms(|| drop(with_mask.apply(img.clone())))
    );
}
