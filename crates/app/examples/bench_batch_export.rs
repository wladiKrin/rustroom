//! What does a *real* batch export cost at width 1 against width N?
//!
//! ⚠⚠ **This exists because composing component measurements has missed in this crate three
//! times.** `probe_decode_parallel` priced **decodes alone** and read 3.31× on four files; a real
//! export also renders, encodes and writes, and the renders contend for the same rayon pool the
//! decodes do. So the end-to-end figure had to be lower, and the only honest way to know by how
//! much is to run the shipped path.
//!
//! ⚠ It drives `batch_export::ExportWorker::spawn` — the production worker, not a copy — so what
//! is measured is what the GUI runs. The decode fn mirrors `decode_for_export` (which lives in the
//! bin and is unreachable from an example); ⚠ **the lens match is part of it deliberately**, since
//! lens correction is on by default for every RAW and is 650–696 ms a photo whole-buffer.
//!
//! Measured on an X-T4, 8 distinct `.RAF` at the default `Better` tier, 8 cores, three runs each,
//! each width in **its own process** (⚠ load 5.1–6.2, *not* a quiet machine):
//!
//! | | wall clock | peak RSS |
//! |---|---|---|
//! | width 1 | **66.02–66.15 s** (8.25 s/photo) | **1.32 GiB** |
//! | width 4 | **22.17–23.37 s** (2.77 s/photo) | **4.01 GiB** |
//! | | **2.82–2.98×** | 3.04× |
//!
//! ⚠ **Lower than the decode sweep's 3.31×, exactly as predicted** — the renders contend where
//! the decodes did not.
//!
//! ⚠⚠ **THE MEMORY IS NOT A LEAK, AND SEPARATING THE WIDTHS IS WHAT ESTABLISHES THAT.** The plan
//! predicted ~1.2 GiB at width 4 and read 4.01, which is the one failure mode this design can
//! have. But that estimate priced the **decode buffers only** (4 × 313 MiB): width 1 — one photo
//! in flight — already peaks at **1.32 GiB**, so a photo costs its decode buffer *plus* about a
//! gigabyte of whole-buffer prefix render, and four of those at 3.04× is **sub-linear** because
//! the process constants and the lensfun database are shared. ⚠ The proof it is not a buffer held
//! past its own photo is that **width 1 does not grow across its 8 photos** — a leak would.
//!
//! ⚠⚠ **AND THE BYTE-IDENTITY CHECK CANNOT BE MET LITERALLY, FOR A REASON THAT PREDATES THIS
//! MILESTONE.** Two exports of the same photo differ in **exactly 2 bytes of 7,156,352**, at
//! offsets 72 and 74 — the **minute and second of the ICC profile's creation timestamp**, which
//! `color::srgb_icc()` has lcms2 stamp into every profile. ⚠ It differs between two **sequential**
//! runs just as much as between width 1 and width 4, so it says nothing about concurrency. What
//! it does say is that **an export is not byte-reproducible**, which matters to anyone
//! checksumming or diffing one. The whole compressed image is byte-identical.
//!
//! Run: `cargo run --release -p app --example bench_batch_export <dest-dir> <raw>...`
//! ⚠ `BENCH_WIDTHS=4` selects the widths; `BENCH_KEEP=1` leaves the output for a `cmp`.
//! ⚠ Peak RSS and load come from the shell (`/usr/bin/time -l`, `uptime`), never from a `libc`
//! dependency added to `app` for a probe's two readings.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use rustroom_app::batch_export::{
    ExportEvent, ExportJob, ExportOptions, ExportRender, ExportSource, ExportWorker,
};
use rustroom_app::photo_id::PhotoId;

/// The production decode, reproduced. ⚠ Keep in step with `decode::decode_for_export`.
fn decode(path: &Path, demosaic: raw_decode::Demosaic) -> Result<ExportSource, String> {
    if catalog::is_raw(path) {
        raw_decode::decode_raw(
            path,
            &raw_decode::DecodeOptions {
                demosaic,
                ..raw_decode::DecodeOptions::linear_scene()
            },
        )
        .map(|d| {
            let lens = d.metadata.lens.as_ref().and_then(|l| {
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
            ExportSource {
                lens,
                baseline: d.metadata.as_shot.unwrap_or_default(),
                image: d.image,
                // ⚠ The bench prices wall clock, not colour; a profile would only add a stage.
                curve: None,
                baseline_ev: None,
                scene_referred: true,
            }
        })
        .map_err(|e| e.to_string())
    } else {
        image_decode::decode(path)
            .map(|d| ExportSource {
                image: d.image,
                baseline: color::wb::Illuminant::default(),
                lens: None,
                curve: None,
                baseline_ev: None,
                scene_referred: true,
            })
            .map_err(|e| e.to_string())
    }
}

fn one_run(files: &[PathBuf], dest: &Path, width: usize, demosaic: raw_decode::Demosaic) -> f64 {
    // ⚠ A fresh destination per run, so no run is served by the collision path the others were not.
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).expect("dest");

    let jobs: Vec<ExportJob> = files
        .iter()
        .map(|p| ExportJob {
            id: PhotoId::original(p.clone()),
            name: None,
            edits: Default::default(),
            capture: Default::default(),
            description: Default::default(),
            marks: Default::default(),
            keywords: Vec::new(),
        })
        .collect();

    let opts = ExportOptions {
        dest_dir: dest.to_path_buf(),
        demosaic,
        ..Default::default()
    };
    let render = ExportRender {
        resampler: Default::default(),
        gpu_tail: false,
    };

    let t = Instant::now();
    let w = ExportWorker::spawn(
        jobs,
        opts,
        move |p: &Path| decode(p, demosaic),
        Arc::new(AtomicBool::new(false)),
        render,
        width,
        None,
    );
    // Drain to completion. ⚠ A collision here would block forever — the fresh directory above is
    // what makes that unreachable rather than something to answer.
    let mut exported = 0usize;
    while let Ok(e) = w.events().recv() {
        if let ExportEvent::Finished(r) = e {
            exported = r.exported;
            assert!(r.failed.is_empty(), "export failed: {:?}", r.failed);
            break;
        }
    }
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(exported, files.len(), "not every photo exported");
    secs
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dest: PathBuf = args.next().expect("usage: <dest-dir> <raw>...").into();
    let files: Vec<PathBuf> = args.map(Into::into).collect();
    assert!(!files.is_empty(), "usage: <dest-dir> <raw>...");

    let demosaic = raw_decode::Demosaic::default();
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // ⚠ Peak RSS and the load average are taken from **outside**, by the shell that runs this
    // (`/usr/bin/time -l`, `uptime`), rather than from `libc` — adding a dependency to `app` for a
    // probe's two readings is a poor trade, and the outside figure covers the whole process.
    println!(
        "{} files, {} cores, demosaic {:?}",
        files.len(),
        cores,
        demosaic
    );

    // ⚠ Selectable so each width can be measured in **its own process**, which is the only way
    // peak RSS says anything: a single process running both reports the larger, and cannot
    // distinguish "four times a correct per-photo footprint" from "a buffer held too long".
    let widths: Vec<usize> = std::env::var("BENCH_WIDTHS")
        .ok()
        .map(|v| v.split(',').filter_map(|w| w.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1, 4]);

    // ⚠ One untimed run first, so no timed row pays a cold page cache the others do not — at the
    // **first measured width**, never a fixed one, or a width-4 process would peak at width 1's
    // warm-up and the RSS reading would describe the wrong run.
    let _ = one_run(&files, &dest, widths[0], demosaic);
    for width in widths {
        let mut runs = Vec::new();
        for _ in 0..3 {
            runs.push(one_run(&files, &dest, width, demosaic));
        }
        let lo = runs.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = runs.iter().cloned().fold(0.0f64, f64::max);
        println!(
            "width {}:  {:.2}-{:.2} s   ({:.2} s/photo at the floor)",
            width,
            lo,
            hi,
            lo / files.len() as f64,
        );
    }
    // ⚠ `BENCH_KEEP=1` leaves the output in place so two widths can be compared byte for byte
    // from the shell. **That comparison is the one thing no amount of speed makes up for**: a wide
    // run and a sequential one must produce identical files, and they agree *by design*, so
    // nothing in the timing can tell you whether they actually do.
    if std::env::var("BENCH_KEEP").is_err() {
        let _ = std::fs::remove_dir_all(&dest);
    }
}
