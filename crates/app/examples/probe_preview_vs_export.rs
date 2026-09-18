//! Reproduce a preview/export divergence reported by use.
//!
//! Writes two PNGs from ONE file with ONE edit set: the preview path's render and the export
//! path's. ⚠ Both decode with the same camera profile, so a difference is the pipeline's, not the
//! decoder's.
use edit_model::Edits;
use pipeline::{RenderScale, RenderSpec, Resampler};

fn main() {
    let mut a = std::env::args().skip(1);
    let raw = a
        .next()
        .expect("usage: probe_preview_vs_export <raw> [profile.dcp]");
    let dcp = a.next().map(std::path::PathBuf::from);
    let out = std::path::PathBuf::from("/tmp/pve");
    std::fs::create_dir_all(&out).unwrap();
    let resampler = Resampler::CatmullRom;
    let e = Edits::default();

    // ── the preview path: half-size decode, as-shot baseline, sRGB ───────────────────
    let popts = raw_decode::DecodeOptions {
        camera_profile: dcp.clone(),
        ..raw_decode::DecodeOptions::preview()
    };
    let pd = raw_decode::decode_raw(std::path::Path::new(&raw), &popts).expect("preview decode");
    let baseline = pd.metadata.as_shot.unwrap_or_default();
    println!(
        "preview buffer {}x{}  as_shot {:?}",
        pd.image.width(),
        pd.image.height(),
        baseline
    );
    let preview = pd.image.downscale_to(1600);
    let scale = RenderScale::new(
        preview.width().max(preview.height()) as f32
            / (pd.image.width().max(pd.image.height()) as f32 / popts.source_scale()),
    );
    let spec = RenderSpec {
        space: color::OutputSpace::Srgb,
        resampler,
        baseline,
        airlight: Some(pipeline::prefix_airlight(&e, &pd.image, resampler, None)),
    };
    let pv = pipeline::build_pipeline_spec(&e, scale, spec).run(preview);
    export::save(
        &pv,
        &out.join("preview.png"),
        export::ExportFormat::Png8,
        100,
        color::OutputSpace::Srgb,
    )
    .unwrap();

    // ── the export path: full decode through `export_one` ────────────────────────────
    let eopts = raw_decode::DecodeOptions {
        camera_profile: dcp.clone(),
        demosaic: raw_decode::Demosaic::Better,
        ..raw_decode::DecodeOptions::linear_scene()
    };
    let ed = raw_decode::decode_raw(std::path::Path::new(&raw), &eopts).expect("export decode");
    println!(
        "export buffer  {}x{}  as_shot {:?}",
        ed.image.width(),
        ed.image.height(),
        ed.metadata.as_shot.unwrap_or_default()
    );
    let (curve, baseline_ev) = match dcp
        .as_ref()
        .and_then(|p| camera_profile::CameraProfile::from_file(p).ok())
    {
        Some(p) => (p.tone_curve, p.baseline_exposure),
        None => (None, None),
    };
    let mut o = rustroom_app::batch_export::ExportOptions {
        max_edge: Some(4000),
        format: export::ExportFormat::Png8,
        ..Default::default()
    };
    o.space = color::OutputSpace::Srgb;
    rustroom_app::batch_export::export_one(
        rustroom_app::batch_export::ExportSource {
            image: ed.image,
            baseline: ed.metadata.as_shot.unwrap_or_default(),
            lens: None,
            curve,
            baseline_ev,
            scene_referred: true,
        },
        &e,
        &out.join("export.png"),
        &o,
        rustroom_app::batch_export::ExportRender {
            resampler,
            gpu_tail: false,
        },
        &export::FileMetadata::default(),
        None,
    )
    .expect("export");
    println!(
        "wrote {}/preview.png and {}/export.png",
        out.display(),
        out.display()
    );
}
