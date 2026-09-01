//! Export one real photograph through `batch_export::export_one` with the metadata, watermark
//! and output sharpening all switched on, in every format, so the results can be inspected with
//! an outside tool.
//!
//! ⚠ Kept, like `sidecar_check` and `lens_diff`, because "the unit tests pass" is not evidence
//! that `exiftool` — or Lightroom, or Preview — can read what we wrote. The tests assert the
//! bytes are where we put them; this is how one checks that anyone else agrees.
//!
//! ```text
//! cargo run --release -p app --example deliverable <photo> <out-dir>
//! ```
//! With no photo, a synthetic chart is used, which is enough for the container work.

use rustroom_app::batch_export::{
    export_one, ExportMetadata, ExportOptions, OutputSharpening, Watermark, WatermarkCorner,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let src = args.next();
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "/tmp/rr".into()));
    std::fs::create_dir_all(&out).unwrap();

    let image = match src.as_deref() {
        Some(p) => {
            let path = std::path::Path::new(p);
            println!("source: {}", path.display());
            if catalog::is_raw(path) {
                raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene())
                    .expect("decode")
                    .image
            } else {
                image_decode::decode(path).expect("decode").image
            }
        }
        None => {
            println!("source: a synthetic 800x600 chart");
            let mut img = core_image::ImageBuf::new(800, 600);
            for y in 0..600 {
                for x in 0..800 {
                    let v = if (x / 40 + y / 40) % 2 == 0 {
                        0.08
                    } else {
                        0.5
                    };
                    img.set_pixel(x, y, [v, v * 0.9, v * 0.7]);
                }
            }
            img
        }
    };

    let opts = ExportOptions {
        dest_dir: out.clone(),
        quality: 92,
        metadata: ExportMetadata {
            copyright: "© 2026 Wladi — all rights reserved".into(),
            creator: "Wladi".into(),
            caption: "A test frame with an ampersand & an <angle> in it".into(),
        },
        watermark: Watermark {
            text: "rustroom".into(),
            corner: WatermarkCorner::BottomRight,
            size: 4,
            opacity: 70,
        },
        output_sharpening: OutputSharpening { amount: 60 },
        ..ExportOptions::default()
    };

    for fmt in export::ExportFormat::ALL {
        let mut o = opts.clone();
        o.format = fmt;
        let dest = out.join(format!("deliverable_{fmt:?}.{}", fmt.extension()));
        match export_one(
            image.clone(),
            &edit_model::Edits::default(),
            &dest,
            &o,
            pipeline::Resampler::default(),
            color::wb::Illuminant::default(),
        ) {
            Ok(()) => println!("wrote {}", dest.display()),
            Err(e) => println!("FAILED {}: {e}", dest.display()),
        }
    }
}
