//! Test fixtures shared by more than one `batch_export` submodule.
//!
//! ⚠⚠ **Shared by MEASUREMENT, not by guess.** In the file this was split out of, `opts` had 24
//! uses, `source` 14 and `flat` 11, spanning the whole test module; `run_export`, `jobs` and
//! `ok_decode` are used by both the render tests and the worker tests. Every other helper is local
//! to one subject and lives beside it.
//!
//! ⚠⚠ **`TempDir` is one of the seven temp-dir isolation sites this repo records.** Its path comes
//! from a caller-supplied tag, so two tests sharing a tag collide — which is why **two gates must
//! never be run concurrently**. No tag changed in the 2026-09-18 split.

use super::*;
use crate::photo_id::PhotoId;
use core_image::ImageBuf;
use edit_model::Edits;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// A unique temp directory for one test, removed on drop.
pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("rustroom_export_{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A flat mid-gray image of the given size, standing in for a decode.
pub(crate) fn flat(w: usize, h: usize) -> ImageBuf {
    let mut img = ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.set_pixel(x, y, [0.25, 0.25, 0.25]);
        }
    }
    img
}

/// A decode with no lens profile, standing in for a JPEG or an unlisted lens.
pub(crate) fn source(image: ImageBuf) -> ExportSource {
    ExportSource {
        image,
        baseline: color::wb::Illuminant::default(),
        lens: None,
        // ⚠ No profile: the tests that care about the camera rendering build their own source
        // with a NON-IDENTITY curve, because the catalogue's own profile curve is `[[0,0],[1,1]]`
        // and a fixture using it would pass with the fix reverted.
        curve: None,
        baseline_ev: None,
        scene_referred: true,
    }
}

/// The render settings every test but the seam's own uses: the default kernel, CPU.
pub(crate) const CPU: ExportRender = ExportRender {
    resampler: pipeline::Resampler::CatmullRom,
    gpu_tail: false,
};

pub(crate) fn opts(dir: &TempDir, max_edge: Option<usize>) -> ExportOptions {
    ExportOptions {
        dest_dir: dir.0.clone(),
        quality: 90,
        max_edge,
        ..ExportOptions::default()
    }
}

/// Drive a whole export to completion, answering every collision prompt with `answer`.
///
/// Returns the report plus every event, so a test can assert on progress as well as the
/// outcome. `decode` receives the source path so a test can fail one specific photo.
pub(crate) fn run_export(
    jobs: Vec<ExportJob>,
    opts: ExportOptions,
    answer: Vec<Decision>,
    decode: impl Fn(&Path) -> Result<ExportSource, String> + Send + Sync + 'static,
) -> (ExportReport, Vec<ExportEvent>) {
    // ⚠ Width 1 by default here, so every pre-existing test keeps its exact meaning; the
    // carrier below drives the wide path explicitly.
    // ⚠ `None` looks directory: these tests exercise naming, collisions and cancellation, none of
    // which involve a `.cube`. The camera-rendering tests build their own worker.
    let worker = ExportWorker::spawn(
        jobs,
        opts,
        decode,
        Arc::new(AtomicBool::new(false)),
        CPU,
        1,
        None,
        // ⚠ No upload: this helper exercises naming, collisions and cancellation, and an export
        // that uploads is its own test with its own connection.
        None,
    );
    let mut answers = answer.into_iter();
    let mut events = Vec::new();
    let mut report = ExportReport::default();
    while let Ok(ev) = worker.events().recv() {
        match &ev {
            ExportEvent::Collision { .. } => {
                let d = answers.next().unwrap_or(Decision::Cancel);
                worker.decide(d);
            }
            ExportEvent::Finished(r) => report = r.clone(),
            _ => {}
        }
        let finished = matches!(ev, ExportEvent::Finished(_));
        events.push(ev);
        if finished {
            break;
        }
    }
    (report, events)
}

/// ⚠ All originals — the pre-existing export tests below keep their exact assertions
/// through the widening to `PhotoId`, which is what shows copies changed nothing for
/// ordinary photos.
pub(crate) fn jobs(dir: &TempDir, names: &[&str]) -> Vec<ExportJob> {
    names
        .iter()
        .map(|n| ExportJob {
            id: PhotoId::original(dir.0.join(n)),
            name: None,
            edits: Edits::default(),
            capture: core_image::Capture::default(),
            description: Default::default(),
            marks: Default::default(),
            keywords: Vec::new(),
        })
        .collect()
}

pub(crate) fn ok_decode() -> impl Fn(&Path) -> Result<ExportSource, String> + Send + 'static {
    |_: &Path| Ok(source(flat(8, 4)))
}
