//! Writes magnified crops of a noisy frame rendered with noise reduction under a straighten, so
//! the author can judge by eye what M7.3b's reorder (NR ahead of the resamplers) did to the grain.
//!
//! ⚠ Compiles on both `main` before M7.3b and the branch after it. It names no stage and relies
//! only on the prefix order the build it is compiled into ships.
//!
//! Run: `cargo run --release -p app --example crop_nr_order <raw> <out-prefix>`
//! Writes `<out-prefix>_centre.png` and `<out-prefix>_corner.png`, each a 400x400 native crop at
//! 200% (nearest neighbour, so no resampling of the viewer's own is added).

use pipeline::{RenderScale, Resampler};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: crop_nr_order <raw> <out-prefix>");
    let out = args
        .next()
        .expect("usage: crop_nr_order <raw> <out-prefix>");
    let d = raw_decode::decode_raw(
        std::path::Path::new(&path),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode");
    let buf = d.image;
    let shot = d.metadata.lens.as_ref().and_then(|l| {
        lens_profile::Catalogue::shared()
            .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
            .map(|profile| lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
    });
    let edits = edit_model::Edits {
        exposure: 0.7,
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            ..Default::default()
        }),
        noise_reduction: edit_model::NoiseReduction {
            luminance: 50.0,
            color: 40.0,
            ..Default::default()
        },
        geometry: edit_model::Geometry {
            angle: 3.0,
            ..Default::default()
        },
        ..edit_model::Edits::default()
    };
    let sc = RenderScale::FULL;
    let res = Resampler::default();
    let air = pipeline::prefix_airlight(&edits, &buf, res, shot.clone());
    let mut img = buf;
    for s in pipeline::build_prefix_steps(&edits, sc, shot, res, Some(air), None) {
        img = s.stage.apply(img);
    }
    let (_, tail) =
        pipeline::build_pipeline_split(&edits, sc, pipeline::TailRendering::colorimetric());
    let img = tail.run(img);
    let (w, h) = (img.width(), img.height());
    println!("rendered {w}x{h}");
    const C: usize = 400;
    for (name, x0, y0) in [
        ("centre", w / 2 - C / 2, h / 2 - C / 2),
        ("corner", w / 10, h / 10),
    ] {
        let crop = img.crop(x0, y0, C, C);
        let rgb = export::to_rgb8(&crop);
        let mut big = vec![0u8; C * 2 * C * 2 * 3];
        for y in 0..C * 2 {
            for x in 0..C * 2 {
                let s = ((y / 2) * C + x / 2) * 3;
                let d = (y * C * 2 + x) * 3;
                big[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
            }
        }
        let p = format!("{out}_{name}.png");
        image::save_buffer(
            &p,
            &big,
            (C * 2) as u32,
            (C * 2) as u32,
            image::ColorType::Rgb8,
        )
        .expect("write");
        println!("wrote {p}");
    }
}
