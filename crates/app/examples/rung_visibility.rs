//! Does a slider still *do* anything at a lower render rung?
//!
//! Run: `cargo run --release -p app --example rung_visibility <raw>`
//!
//! ⚠⚠ **This exists to settle one design question and nothing else: whether the interactive rung
//! may drop a rung while a SPATIAL slider is being dragged.** The scale-aware stages —
//! `SharpenStage`, `LocalContrastStage`, `NoiseReductionStage` — take a `RenderScale` so their
//! radii track the render resolution, which keeps their effect *proportionate* to the image at any
//! rung. That is the design working. The question it does not answer is whether the effect is
//! still **visible**, and `CLAUDE.md` already records the case where it is not: at fit scale a
//! full-resolution σ of 1.0 becomes 0.256 px, "a kernel of [0.0005, 0.999, 0.0005], i.e. the
//! identity".
//!
//! ⚠ **The measurement is deliberately WITHIN a rung, with no resampling anywhere.** For each edit
//! it renders neutral and edited *at the same rung* and reports how far apart they are. That is
//! exactly the question a person dragging the slider is asking — "is the picture in front of me
//! changing?" — and it needs no assumption about how one rung's pixels would be resampled to
//! another's, which is where a cross-rung comparison would quietly smuggle in an answer.
//!
//! ⚠ A **per-pixel** edit is included as the control. Exposure has no kernel, so it must read
//! essentially identically at every rung; if it does not, the harness is wrong before any spatial
//! row can be trusted.
use edit_model::{Dehaze, Edits, LocalContrast, NoiseReduction, Sharpening};
use rustroom_app::render_engine::render_once;

/// The ladder the app actually walks, from `view_region::tier_max_dim`.
const FIT_CAP: usize = 1600;

/// Mean absolute difference in 8-bit levels, and the share of samples that moved at all.
///
/// ⚠ Both numbers are reported because either alone misleads. A *mean* hides an effect confined to
/// edges — which is precisely what sharpening is — and a *share* counts a one-level dither as
/// equal to a visible change. Sharpening moving 3% of samples by 40 levels and a global exposure
/// moving 100% by 3 are opposite shapes with a similar mean.
fn diff(a: &[u8], b: &[u8]) -> (f64, f64) {
    let mut sum = 0.0;
    let mut moved = 0usize;
    for (x, y) in a.iter().zip(b.iter()) {
        let d = (*x as i32 - *y as i32).unsigned_abs();
        sum += d as f64;
        if d > 0 {
            moved += 1;
        }
    }
    (sum / a.len() as f64, moved as f64 * 100.0 / a.len() as f64)
}

fn sharpen() -> Edits {
    Edits {
        sharpening: Sharpening {
            amount: 70.0,
            ..Sharpening::default()
        },
        ..Edits::default()
    }
}

fn texture() -> Edits {
    Edits {
        local_contrast: LocalContrast {
            texture: 70.0,
            clarity: 0.0,
        },
        ..Edits::default()
    }
}

fn clarity() -> Edits {
    Edits {
        local_contrast: LocalContrast {
            texture: 0.0,
            clarity: 70.0,
        },
        ..Edits::default()
    }
}

fn denoise() -> Edits {
    Edits {
        noise_reduction: NoiseReduction {
            luminance: 50.0,
            color: 50.0,
            ..NoiseReduction::default()
        },
        ..Edits::default()
    }
}

fn haze() -> Edits {
    Edits {
        dehaze: Dehaze { amount: 60.0 },
        ..Edits::default()
    }
}

/// The control: per-pixel, no kernel, so it must read the same at every rung.
fn exposure() -> Edits {
    Edits {
        exposure: 0.5,
        ..Edits::default()
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: rung_visibility <raw>");
    let path = std::path::Path::new(&path);

    // ⚠ A **full-resolution** decode, not `preview()`. The ladder is derived from the buffer's own
    // long edge, so a half-size buffer yields two rungs where the running app (which upgrades to
    // full resolution after 800 ms) has three — and the rung under question, 3200, would not
    // exist at all.
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let buf = d.image;
    let scale = opts.source_scale();
    let buffer_long = buf.width().max(buf.height());
    println!(
        "buffer {}x{} (source_scale {scale})",
        buf.width(),
        buf.height()
    );

    let rungs: Vec<usize> = {
        let mut v = vec![FIT_CAP];
        let mut r = FIT_CAP;
        while r < buffer_long {
            r = (r * 2).min(buffer_long);
            v.push(r);
        }
        v
    };
    println!("ladder: {rungs:?}\n");

    let cases: Vec<(&str, Edits)> = vec![
        ("exposure +0.5 (control)", exposure()),
        ("sharpening 70", sharpen()),
        ("texture 70", texture()),
        ("clarity 70", clarity()),
        ("noise reduction 50/50", denoise()),
        ("dehaze 60", haze()),
    ];

    let render = |edits: &Edits, max_dim: usize| {
        render_once(
            &buf,
            edits,
            max_dim,
            scale,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
        )
    };

    print!("{:<24}", "edit");
    for r in &rungs {
        print!("{:>22}", format!("rung {r}"));
    }
    println!();
    print!("{:<24}", "");
    for _ in &rungs {
        print!("{:>22}", "mean levels / % moved");
    }
    println!();

    for (label, edits) in &cases {
        print!("{label:<24}");
        for r in &rungs {
            let base = render(&Edits::default(), *r);
            let edited = render(edits, *r);
            let (mean, moved) = diff(&base.srgb, &edited.srgb);
            print!("{:>22}", format!("{mean:8.3} / {moved:6.2}%"));
        }
        println!();
    }

    println!(
        "\n⚠ Read DOWN each column for 'what does this slider do at this rung', and ACROSS a row\n\
         for how much of that survives dropping a rung. The control row must be flat across;\n\
         anything else means the harness, not the stage."
    );
}
