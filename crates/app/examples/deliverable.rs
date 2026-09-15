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
            ..ExportMetadata::default()
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
            // ⚠ A synthetic chart carries no lens metadata, so `lens: None` is the truth here
            // rather than a shortcut — the same thing a JPEG's decode hands over.
            rustroom_app::batch_export::ExportSource {
                image: image.clone(),
                baseline: color::wb::Illuminant::default(),
                lens: None,
            },
            &edit_model::Edits::default(),
            &dest,
            &o,
            rustroom_app::batch_export::ExportRender {
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
            },
            &export::FileMetadata::resolve(&o.metadata, described()),
        ) {
            Ok(()) => println!("wrote {}", dest.display()),
            Err(e) => println!("FAILED {}: {e}", dest.display()),
        }
    }
}

/// A photo that says everything a catalog can say about one — so `exiftool` on the output shows
/// every property M6.2 writes: title, caption, rating, label, both keyword forms, and the
/// capture facts.
fn described() -> export::FileMetadata {
    export::FileMetadata {
        title: "Deliverable probe".into(),
        caption: "Every field the packet can carry, for exiftool to read back".into(),
        rating: 4,
        label: Some("Blue".into()),
        keywords: vec![
            vec!["Europe".into(), "Germany".into(), "Munich".into()],
            vec!["Probe".into()],
        ],
        capture: core_image::Capture {
            taken: Some(1_787_824_800),
            camera: Some("FUJIFILM X-T4".into()),
            lens: Some("XF23mmF2 R WR".into()),
            iso: Some(1600),
            shutter: Some(0.004),
            aperture: Some(5.6),
            focal_len: Some(23.0),
        },
        ..export::FileMetadata::default()
    }
}
