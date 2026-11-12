use super::*;
use core_image::ImageBuf;
use edit_model::{Defringe, Edits};

fn ctx() -> Option<Arc<GpuContext>> {
    match crate::shared() {
        Some(c) => Some(c.clone()),
        None => {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            None
        }
    }
}

/// Constructed: one strong dark/bright edge, and a purple band 3–4 px from it on a grey of the SAME luminance, so the
/// band's own rim is no edge — only the max filter's reach can bring the band near one. At full scale (radius 6) it
/// is reached; at 0.4 (radius 3) it falls outside. ⚠ A first fixture put the band ON the edge, where the Sobel alone
/// reaches it, and passed with the max filter deleted and the radius unscaled.
fn fringed(w: usize, h: usize) -> ImageBuf {
    let purple = color::srgb_to_prophoto([0.35, 0.12, 0.55]);
    let y = color::luminance(purple);
    let grey = [y, y, y];
    let edge = w / 4;
    let mut img = ImageBuf::new(w, h);
    for yy in 0..h {
        for x in 0..w {
            let c = if x < edge {
                [0.005, 0.005, 0.005]
            } else if (edge + 3..edge + 5).contains(&x) {
                purple
            } else {
                grey
            };
            img.set_pixel(x, yy, c);
        }
    }
    img
}

fn edits(d: Defringe) -> Edits {
    Edits {
        defringe: d,
        ..Edits::default()
    }
}

fn both_amounts() -> Defringe {
    Defringe {
        purple_amount: 80.0,
        green_amount: 60.0,
        ..Default::default()
    }
}

/// The GPU twin renders the CPU stage's picture, at full scale and at a preview's — where the edge radius shrinks.
/// ⚠ It must change the picture first, or agreeing with the CPU proves nothing.
#[test]
fn the_gpu_defringe_matches_the_cpu() {
    let Some(ctx) = ctx() else { return };
    let img = fringed(160, 120);
    for scale in [RenderScale::FULL, RenderScale::new(0.4)] {
        let e = edits(both_amounts());
        let stage = GpuDefringeStage::new(&e, scale, &ctx).expect("a global defringe has a twin");
        let cpu = stage.cpu().apply(img.clone());
        assert!(
            crate::compare(&cpu, &img).worst_levels > 10.0,
            "fixture check: the defringe must change the picture"
        );
        // ⚠⚠ Through `apply_keeping_input`, whose kept image exists only when the GPU RAN: a chain that fails falls
        // back to its CPU twin, so a plain `apply` agrees with the CPU whatever the shader does — this test passed
        // with a shader that did not even compile (`smooth` is a WGSL keyword) and with both sabotages in.
        let (gpu, ran) = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)])
            .apply_keeping_input(img.clone());
        assert!(ran.is_some(), "the chain fell back to the CPU");
        assert!(
            gpu.data().iter().all(|v| v.is_finite()),
            "a non-finite GPU output"
        );
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 1.0,
            "scale {}: {} levels",
            scale.get(),
            d.worst_levels
        );
    }
}

/// No defringe, or a mask carrying a local one, leaves the CPU step: its coverage is not built here.
#[test]
fn a_local_defringe_or_none_declines() {
    let Some(ctx) = ctx() else { return };
    assert!(GpuDefringeStage::new(&Edits::default(), RenderScale::FULL, &ctx).is_none());
    let mut e = edits(both_amounts());
    e.masks = vec![edit_model::MaskedAdjustment {
        mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
        adjustments: edit_model::LocalAdjustments {
            defringe: -100.0,
            ..Default::default()
        },
        ..Default::default()
    }];
    assert!(GpuDefringeStage::new(&e, RenderScale::FULL, &ctx).is_none());
    e.masks[0].disabled = true;
    assert!(
        GpuDefringeStage::new(&e, RenderScale::FULL, &ctx).is_some(),
        "a disabled mask defringes nothing, so it does not decline"
    );
}
