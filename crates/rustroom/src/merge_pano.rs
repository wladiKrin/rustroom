//! `rustroom-cli merge-pano`: a row of frames into one floating-point DNG, with what it measured.

use anyhow::{bail, Context, Result};
use photo_merge::hdr;
use photo_merge::pano::{self, PanoOptions, Progress};
use photo_merge::projection::{Projection, MAX_LONG_EDGE};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// The long edge of the picture the DNG carries.
const PREVIEW: usize = 1024;

fn projection(s: &str) -> Result<Option<Projection>> {
    if s.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    [
        Projection::Spherical,
        Projection::Cylindrical,
        Projection::Perspective,
    ]
    .into_iter()
    .find(|p| p.label().eq_ignore_ascii_case(s))
    .map(Some)
    .with_context(|| {
        format!("projection must be auto, spherical, cylindrical or perspective, not {s:?}")
    })
}

pub fn run(
    inputs: &[PathBuf],
    output: &Path,
    proj: &str,
    auto_crop: bool,
    half: bool,
) -> Result<()> {
    if inputs.len() < 2 {
        bail!("a panorama needs two or more photos");
    }
    let opts = PanoOptions {
        projection: projection(proj)?,
        auto_crop,
        fill_edges: false,
    };
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let d = hdr::decode_all(inputs, half, &cancel, &mut |p| {
        if let hdr::Progress::Decoding { done, total } = p {
            eprintln!("decoding {} of {total}", done + 1);
        }
    })?;
    let decoded = t.elapsed();
    let t = Instant::now();
    let a = pano::align(&d, None, &cancel, &mut |_| {}).map_err(|e| match e {
        hdr::MergeError::Unconnected(v) => {
            let names: Vec<String> = v.iter().map(|&i| inputs[i].display().to_string()).collect();
            anyhow::anyhow!("these share nothing with the others: {}", names.join(", "))
        }
        e => e.into(),
    })?;
    let aligned = t.elapsed();
    let order: Vec<String> = a
        .rig
        .order
        .iter()
        .map(|&i| inputs[i].display().to_string())
        .collect();
    eprintln!("order: {}", order.join(" → "));
    eprintln!(
        "reference: {} · focal {:.1} px",
        inputs[a.rig.reference].display(),
        a.rig.focal_px
    );
    for (i, p) in inputs.iter().enumerate() {
        eprintln!(
            "  {}: gain {:.4} ({:+.2} EV)",
            p.display(),
            a.gains[i],
            a.gains[i].log2()
        );
    }
    if let Some(l) = &a.no_profile {
        eprintln!("no lens profile for {l}: distortion and vignetting are not corrected");
    }
    let t = Instant::now();
    let s = pano::stitch(&d, &a, &opts, MAX_LONG_EDGE, None, &cancel, &mut |p| {
        if p == Progress::Stitching {
            eprintln!("stitching");
        }
    })?;
    let stitched = t.elapsed();
    eprintln!(
        "{} · {} × {}{}",
        s.projection.label(),
        s.image.width(),
        s.image.height(),
        s.crop.map_or(String::new(), |c| format!(
            " · crop {:.3} {:.3} {:.3} {:.3}",
            c.left, c.top, c.right, c.bottom
        ))
    );
    let t = Instant::now();
    let meta = &d.metas[s.reference];
    let mut edits = edit_model::Edits::default();
    if let Some(c) = s.crop {
        edits.geometry.crop = c;
    }
    let preview = photo_merge::preview::render(
        &pano::to_prophoto(&s, &d),
        &edits,
        PREVIEW,
        if half { 0.5 } else { 1.0 },
        meta.metadata.as_shot.unwrap_or_default(),
    );
    pano::write_dng(&s, &d, output, &preview)?;
    let written = t.elapsed();
    let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "decode {:.1} s · align {:.1} s · stitch {:.1} s · preview+write {:.1} s · {} ({:.1} MB)",
        decoded.as_secs_f32(),
        aligned.as_secs_f32(),
        stitched.as_secs_f32(),
        written.as_secs_f32(),
        output.display(),
        size as f64 / 1e6
    );
    Ok(())
}
