//! Does an already-rendered source (JPEG/PNG/TIFF) get rendered TWICE at export?
//!
//! ⚠⚠ `render_cached`'s worker strips `Edits.rendering` when the buffer is **not** scene-referred:
//! *"A source that is already rendered never renders again. A JPEG, a PNG or a TIFF has been
//! through whatever produced it; applying ours on top would render the photograph twice."*
//! `ExportSource` carries no such flag, so the export path **cannot** strip it. This measures
//! whether that is a real divergence.
use edit_model::{Edits, Rendering};
use pipeline::{RenderScale, RenderSpec, Resampler};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_jpeg_rendering_gap <jpeg>");
    let d = image_decode::decode(std::path::Path::new(&path)).expect("decode");
    let img = d.image.downscale_to(1200);
    println!("{}x{}", img.width(), img.height());
    let resampler = Resampler::CatmullRom;
    let scale = RenderScale::new(1.0);
    let spec = RenderSpec {
        space: color::OutputSpace::Srgb,
        intent: color::RenderingIntent::default(),
        resampler,
        baseline: color::wb::Illuminant::default(),
        airlight: None,
    };
    let run = |e: &Edits| {
        pipeline::build_pipeline_spec(e, scale, spec, pipeline::ProfileRendering::colorimetric())
            .run(img.clone())
    };

    let asked = Edits {
        rendering: Some(Rendering::Camera),
        ..Default::default()
    };
    // What the PREVIEW renders: the worker strips `rendering` for a non-scene-referred buffer.
    let stripped = Edits {
        rendering: None,
        ..asked.clone()
    };
    let preview = run(&stripped);
    // What the EXPORT renders today: whatever the edits say, with nothing stripped.
    let export = run(&asked);
    let dv = gpu_pipeline::compare(&preview, &export);
    println!(
        "preview (stripped) vs export (not stripped): worst {:.2} levels, {:.1}% of samples differ",
        dv.worst_levels,
        dv.differing_percent()
    );
    export::save(
        &preview,
        std::path::Path::new("/tmp/pve/jpeg_preview.png"),
        export::ExportFormat::Png8,
        100,
        color::OutputSpace::Srgb,
    )
    .unwrap();
    export::save(
        &export,
        std::path::Path::new("/tmp/pve/jpeg_export.png"),
        export::ExportFormat::Png8,
        100,
        color::OutputSpace::Srgb,
    )
    .unwrap();
}
