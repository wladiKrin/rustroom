//! `rustroom-cli merge-hdr`: a bracket into one floating-point DNG, with what it measured.

use anyhow::{bail, Context, Result};
use photo_merge::hdr::{self, Deghost, HdrOptions, Progress};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// The long edge of the picture the DNG carries.
const PREVIEW: usize = 1024;

fn deghost(s: &str) -> Result<Deghost> {
    Deghost::ALL
        .into_iter()
        .find(|d| d.label().eq_ignore_ascii_case(s))
        .with_context(|| format!("deghost must be none, low, medium or high, not {s:?}"))
}

pub fn run(
    inputs: &[PathBuf],
    output: &Path,
    level: &str,
    auto_align: bool,
    half: bool,
) -> Result<()> {
    if inputs.len() < 2 {
        bail!("an HDR merge needs two or more photos");
    }
    let opts = HdrOptions {
        auto_align,
        deghost: deghost(level)?,
    };
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let d = hdr::decode_all(inputs, half, &cancel, &mut |p| {
        if let Progress::Decoding { done, total } = p {
            eprintln!("decoding {} of {total}", done + 1);
        }
    })?;
    let decoded = t.elapsed();
    let t = Instant::now();
    let m = hdr::merge(&d, None, &opts, None, &cancel, &mut |_| {})?;
    let merged = t.elapsed();
    let (w, h) = (m.image.width(), m.image.height());
    eprintln!("reference: {}", inputs[m.reference].display());
    for (i, p) in inputs.iter().enumerate() {
        let moved =
            m.transforms[i].corner_shift(&photo_merge::homography::Homography::identity(), w, h);
        eprintln!(
            "  {}: scale {:.4} ({:+.2} EV), corners moved {moved:.2} px",
            p.display(),
            m.scales[i],
            -m.scales[i].log2()
        );
    }
    let t = Instant::now();
    let meta = &d.metas[m.reference];
    let preview = photo_merge::preview::render(
        &hdr::to_prophoto(&m, &d),
        &edit_model::Edits::default(),
        PREVIEW,
        if half { 0.5 } else { 1.0 },
        meta.metadata.as_shot.unwrap_or_default(),
    );
    hdr::write_dng(&m, &d, output, &preview)?;
    let written = t.elapsed();
    let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "decode {:.1} s · align+merge {:.1} s · preview+write {:.1} s · {} ({:.1} MB)",
        decoded.as_secs_f32(),
        merged.as_secs_f32(),
        written.as_secs_f32(),
        output.display(),
        size as f64 / 1e6
    );
    Ok(())
}
