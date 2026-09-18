//! Decomposes `LensCorrectionStage` into the two things it does, and prices the one design
//! decision the GPU port turns on.
//!
//! ⚠⚠ **Written before the spec, because this project's rule is measure before theorising** —
//! and `CLAUDE.md`'s own numbers for this stage do not compose. It records "400 ms in lensfun's
//! coordinate calls and 765 ms in the resampler" from the per-pixel era, and then a whole
//! resampling prefix of 309 ms after row batching. Both cannot describe the same stage, so the
//! split has to be re-measured rather than inferred.
//!
//! Two questions:
//!
//! 1. **Of lens correction's ~220 ms, how much is the displacement FIELD and how much is the
//!    RESAMPLING?** Only the resampling is obviously GPU work; the field is lensfun's own maths
//!    on the CPU. If the field dominates, a shader that resamples buys little.
//! 2. ⚠ **Can the field be carried to the GPU as a COARSE GRID instead of per pixel?** Per pixel
//!    it is 6 source coordinates plus a gain — 28 bytes — which at 26 Mpx is 729 MiB to upload,
//!    more than the image itself. The field is a smooth radial polynomial, so a grid sampled
//!    every `s` pixels and interpolated should be exact to far below a pixel. **Should** is not
//!    a measurement, so this sweeps `s` and reports the worst error against the exact field.
//!
//! Run: `cargo run --release -p app --example probe_lens <raw> [--full]`

use core_image::ImageBuf;
use lens_profile::{Corrections, Corrector, LensShot, RowMap};
use pipeline::resample::{sample_channel, Weights};
use pipeline::{PipelineStage, Resampler};
use rayon::prelude::*;
use std::time::Instant;

fn best_of<T, F: FnMut() -> T>(n: usize, mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..n {
        let t = Instant::now();
        let out = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&out);
    }
    best
}

/// The field alone: every row's source coordinates and gains, computed and thrown away.
///
/// ⚠ Parallel over rows with `for_each_init`, exactly as the stage is, so this is the stage's
/// own field cost rather than a sequential lower bound.
fn field_only(c: &Corrector, w: usize, h: usize) -> f32 {
    (0..h)
        .into_par_iter()
        .fold(
            || (RowMap::new(w), 0.0f32),
            |(mut map, acc), y| {
                c.fill_row(y, &mut map);
                // Touch the result so the work cannot be optimised away.
                let a = acc + map.gain(w / 2) + map.source(w / 2)[1][0];
                (map, a)
            },
        )
        .map(|(_, a)| a)
        .sum()
}

/// The resampling alone: the stage's inner loop, fed a field that is already in hand.
///
/// ⚠ One `RowMap` per worker and one `fill_row` per row, as the stage does — so this row and
/// the field row above overlap by exactly the field's cost, and their difference is resampling.
fn whole_stage(image: &ImageBuf, c: &Corrector, weights: &Weights) -> ImageBuf {
    let (w, h) = (image.width(), image.height());
    const CH: usize = ImageBuf::CHANNELS;
    let stride = w * CH;
    let mut data = vec![0.0f32; w * h * CH];
    data.par_chunks_exact_mut(stride).enumerate().for_each_init(
        || RowMap::new(w),
        |map, (y, orow)| {
            c.fill_row(y, map);
            for (x, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                let src = map.source(x);
                let gain = map.gain(x);
                for (ch, p) in out.iter_mut().enumerate() {
                    *p = sample_channel(image, src[ch][0], src[ch][1], ch, weights) * gain;
                }
            }
        },
    );
    ImageBuf::from_vec(w, h, data)
}

/// A displacement grid: at every `s`-th pixel, the per-channel displacement and the gain.
///
/// ⚠ **Displacement, never the absolute source coordinate.** Bilinear interpolation reproduces
/// an affine function exactly, and the source map is `identity + displacement` — so the two are
/// mathematically identical and the displacement is the smaller number, which rounds better.
struct Grid {
    cols: usize,
    rows: usize,
    step: usize,
    /// `[dxR, dyR, dxG, dyG, dxB, dyB, gain]` per grid point.
    data: Vec<f32>,
}

const STRIDE: usize = 7;

fn build_grid(c: &Corrector, w: usize, h: usize, step: usize) -> Grid {
    let cols = w.div_ceil(step) + 1;
    let rows = h.div_ceil(step) + 1;
    let mut data = vec![0.0f32; cols * rows * STRIDE];
    data.par_chunks_exact_mut(cols * STRIDE)
        .enumerate()
        .for_each(|(gy, row)| {
            let y = (gy * step) as f32;
            for (gx, cell) in row.as_chunks_mut::<STRIDE>().0.iter_mut().enumerate() {
                let x = (gx * step) as f32;
                let s = c.source(x, y);
                for ch in 0..3 {
                    cell[2 * ch] = s[ch][0] - x;
                    cell[2 * ch + 1] = s[ch][1] - y;
                }
                cell[6] = c.gain(x, y);
            }
        });
    Grid {
        cols,
        rows,
        step,
        data,
    }
}

impl Grid {
    /// Bilinearly interpolate the seven values at an output pixel.
    fn at(&self, x: usize, y: usize) -> [f32; STRIDE] {
        let fx = x as f32 / self.step as f32;
        let fy = y as f32 / self.step as f32;
        let gx = (fx.floor() as usize).min(self.cols - 2);
        let gy = (fy.floor() as usize).min(self.rows - 2);
        let tx = fx - gx as f32;
        let ty = fy - gy as f32;
        let idx = |cx: usize, cy: usize| (cy * self.cols + cx) * STRIDE;
        let (a, b, c2, d) = (
            idx(gx, gy),
            idx(gx + 1, gy),
            idx(gx, gy + 1),
            idx(gx + 1, gy + 1),
        );
        let mut out = [0.0f32; STRIDE];
        for (k, o) in out.iter_mut().enumerate() {
            let top = self.data[a + k] * (1.0 - tx) + self.data[b + k] * tx;
            let bot = self.data[c2 + k] * (1.0 - tx) + self.data[d + k] * tx;
            *o = top * (1.0 - ty) + bot * ty;
        }
        out
    }

    fn bytes(&self) -> usize {
        self.data.len() * 4
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe_lens <raw>");
    let path = std::path::Path::new(&path);
    let full = std::env::args().any(|a| a == "--full");
    let opts = if full {
        raw_decode::DecodeOptions::linear_scene()
    } else {
        raw_decode::DecodeOptions::preview()
    };
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let image = d.image;
    let (w, h) = (image.width(), image.height());
    println!(
        "buffer {w}x{h} ({:.1} Mpx), resampler {:?}, {} threads",
        (w * h) as f64 / 1e6,
        Resampler::default(),
        rayon::current_num_threads()
    );

    let shot: LensShot = d
        .metadata
        .lens
        .as_ref()
        .and_then(|l| {
            lens_profile::Catalogue::shared()
                .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
                .map(|profile| LensShot {
                    profile,
                    shot: lens_profile::Shot {
                        focal: l.focal,
                        aperture: l.aperture,
                    },
                })
        })
        .expect("fixture check: this probe needs a matched lens profile");
    println!("lens: {}", shot.profile.lens_name());

    let want = Corrections {
        distortion: true,
        tca: true,
        vignetting: true,
        scale: 0.0,
    };
    let c = Corrector::new(&shot.profile, &shot.shot, w, h, want)
        .expect("fixture check: the profile must supply a correction");
    println!(
        "autoscale {:.4}, applied {:.4}",
        c.autoscale(),
        c.applied_scale()
    );

    // ---- Question 1: where does the time go? ----
    let weights = Weights::new(Resampler::default());
    let stage = pipeline::LensCorrectionStage::new(shot.clone(), want, Resampler::default());
    let whole = best_of(3, || stage.apply(image.clone()));
    let inner = best_of(3, || whole_stage(&image, &c, &weights));
    let fld = best_of(3, || field_only(&c, w, h));
    let clone_ms = best_of(5, || image.clone());
    println!("\n  where the time goes");
    println!("{whole:8.1} ms   the real stage (includes one clone and one allocation)");
    println!("{inner:8.1} ms   the same loop, reached directly");
    println!("{fld:8.1} ms   the FIELD alone (fill_row over every row, nothing sampled)");
    println!("{:8.1} ms   the RESAMPLING, by difference", inner - fld);
    println!("{clone_ms:8.1} ms   (a bare clone, for scale)");
    println!(
        "           field is {:.0}% of the stage's inner loop",
        100.0 * fld / inner
    );

    // ---- Question 2: does a coarse grid reproduce the field? ----
    println!("\n  a coarse displacement grid against the exact field");
    println!("  step    points      bytes    build      worst px     worst gain");
    for step in [4usize, 8, 16, 32, 64, 128] {
        let build = best_of(3, || build_grid(&c, w, h, step));
        let g = build_grid(&c, w, h, step);
        // ⚠ Compared against the exact field at EVERY pixel, not at a sample of them: the worst
        // error is what matters and it need not sit anywhere obvious.
        let (worst_px, worst_gain) = (0..h)
            .into_par_iter()
            .fold(
                || (RowMap::new(w), 0.0f32, 0.0f32),
                |(mut map, mut wp, mut wg), y| {
                    c.fill_row(y, &mut map);
                    for x in 0..w {
                        let got = g.at(x, y);
                        let exact = map.source(x);
                        for ch in 0..3 {
                            let dx = (got[2 * ch] + x as f32) - exact[ch][0];
                            let dy = (got[2 * ch + 1] + y as f32) - exact[ch][1];
                            wp = wp.max(dx.hypot(dy));
                        }
                        wg = wg.max((got[6] - map.gain(x)).abs());
                    }
                    (map, wp, wg)
                },
            )
            .map(|(_, p, gn)| (p, gn))
            .reduce(|| (0.0, 0.0), |a, b| (a.0.max(b.0), a.1.max(b.1)));
        println!(
            "  {step:4}  {:8}  {:8.2} MiB  {build:6.1} ms   {worst_px:9.4}   {worst_gain:12.6}",
            g.cols * g.rows,
            g.bytes() as f64 / (1024.0 * 1024.0),
        );
    }
    // ⚠ What the per-pixel field would cost to upload, for contrast with the rows above.
    println!(
        "\n  per pixel, for contrast: {:.0} MiB of field for a {:.0} MiB image",
        (w * h * STRIDE * 4) as f64 / (1024.0 * 1024.0),
        (w * h * 3 * 4) as f64 / (1024.0 * 1024.0),
    );

    // ---- Question 3: how wide a margin does an `input_rect_for` need? ----
    //
    // ⚠⚠ **A radial map BENDS LINES, so `PerspectiveStage`'s four-corner bound is unsound here.**
    // A homography sends lines to lines, so a rect's image is a quadrilateral and the corners
    // bound it; a radial correction bows every edge, and the extreme can sit at an edge midpoint.
    // The bound that *is* sound is the largest displacement anywhere in the frame, since no pixel
    // can move further than the furthest-moving pixel. This measures it, so the margin is a
    // number rather than an assumption.
    let worst_disp = (0..h)
        .into_par_iter()
        .fold(
            || (RowMap::new(w), 0.0f32),
            |(mut map, mut wd), y| {
                c.fill_row(y, &mut map);
                for x in 0..w {
                    let s = map.source(x);
                    for sc in s.iter() {
                        wd = wd.max((sc[0] - x as f32).hypot(sc[1] - y as f32));
                    }
                }
                (map, wd)
            },
        )
        .map(|(_, d)| d)
        .reduce(|| 0.0f32, f32::max);
    let pad = Resampler::default().radius();
    let margin = worst_disp.ceil() as usize + pad;
    println!("\n  a whole-frame displacement bound");
    println!("  worst displacement anywhere: {worst_disp:.2} px");
    println!("  margin = ceil(that) + resampler radius {pad} = {margin} px");

    // What that margin costs on the region a cold 1:1 entry actually asks for.
    for (rw, rh) in [(1792usize, 1394usize), (1432, 1034)] {
        let (rw, rh) = (rw.min(w), rh.min(h));
        let mw = (rw + 2 * margin).min(w);
        let mh = (rh + 2 * margin).min(h);
        println!(
            "  a {rw}x{rh} region margins out to {mw}x{mh} — {:.1}% of the frame",
            100.0 * (mw * mh) as f64 / (w * h) as f64
        );
    }

    // ---- Question 4: what would a scoped lens correction cost? ----
    //
    // ⚠ The same inner loop, producing only a rect. Not the shipped code — there is none yet —
    // but the cost is what it measures, and the cost is what the decision turns on.
    println!("\n  the same stage, producing only a rect");
    for (rw, rh) in [(1792usize, 1394usize), (1432, 1034)] {
        let (rw, rh) = (rw.min(w), rh.min(h));
        let ms = best_of(3, || {
            const CH: usize = ImageBuf::CHANNELS;
            let stride = rw * CH;
            let mut data = vec![0.0f32; rw * rh * CH];
            data.par_chunks_exact_mut(stride).enumerate().for_each_init(
                || RowMap::new(w),
                |map, (j, orow)| {
                    // ⚠ The field is still filled for the WHOLE row: `fill_row` starts at x = 0.
                    // A shipped `apply_region` would start at the rect instead, so this row is
                    // pessimistic by exactly that — and the field is 5%, so barely at all.
                    c.fill_row(j, map);
                    for (i, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                        let src = map.source(i);
                        let gain = map.gain(i);
                        for (ch, p) in out.iter_mut().enumerate() {
                            *p =
                                sample_channel(&image, src[ch][0], src[ch][1], ch, &weights) * gain;
                        }
                    }
                },
            );
            ImageBuf::from_vec(rw, rh, data)
        });
        println!(
            "{ms:8.1} ms   {rw}x{rh} ({:.1} Mpx)",
            (rw * rh) as f64 / 1e6
        );
    }

    // ---- Question 5: the REAL prefix chain, cold, both ways ----
    //
    // ⚠⚠ **This is the shipped code path, not a reproduction of it** — `build_prefix_steps`,
    // `region_plan` from step **0**, `run_region_plan` — so it prices the thing a cold entry into
    // 1:1 actually does rather than the stage in isolation. It is still headless, so it is not a
    // session: the row that decides the milestone is a cold entry measured through
    // `RUSTROOM_DEBUG_RENDER`, and this is what that row should land near.
    let mut edits = edit_model::Edits {
        perspective: edit_model::Perspective {
            vertical: 12.0,
            ..Default::default()
        },
        noise_reduction: edit_model::NoiseReduction {
            luminance: 50.0,
            color: 50.0,
            ..Default::default()
        },
        dehaze: edit_model::Dehaze { amount: 40.0 },
        ..Default::default()
    };
    edits.geometry.angle = 3.0;
    edits.lens_correction = Some(edit_model::LensCorrection {
        distortion: true,
        tca: true,
        vignetting: true,
        scale: 0.0,
    });
    let airlight =
        pipeline::prefix_airlight(&edits, &image, Resampler::default(), Some(shot.clone()));
    let steps = pipeline::build_prefix_steps(
        &edits,
        pipeline::RenderScale::FULL,
        Some(shot.clone()),
        Resampler::default(),
        Some(airlight),
    );
    println!("\n  the real prefix chain, cold (resuming at 0)");
    println!(
        "  steps: {:?}",
        steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>()
    );
    // The viewport a cold entry into 1:1 asks for, from M8.7's own measured session.
    let visible = pipeline::Rect {
        x: (w.saturating_sub(1432)) / 2,
        y: (h.saturating_sub(1034)) / 2,
        w: 1432.min(w),
        h: 1034.min(h),
    };
    match pipeline::region_plan(&steps, 0, visible, w, h) {
        None => println!("  ⚠ the chain ABANDONED at step 0 — the pair is not wired up"),
        Some(plan) => {
            let margined = plan.out_rects[0];
            println!(
                "  SCOPED {}x{} -> {}x{} of {}x{}  ({:.1}% of the frame)",
                margined.w,
                margined.h,
                visible.w,
                visible.h,
                plan.frame.0,
                plan.frame.1,
                100.0 * (margined.w * margined.h) as f64 / (plan.frame.0 * plan.frame.1) as f64,
            );
            let scoped_ms = best_of(3, || {
                pipeline::run_region_plan(&steps, 0, &image, &plan).expect("the chain runs")
            });
            let whole_ms = best_of(3, || {
                let mut cur = image.clone();
                for st in &steps {
                    cur = st.stage.apply(cur);
                }
                cur
            });
            println!("{whole_ms:8.1} ms   whole-buffer, every step");
            println!("{scoped_ms:8.1} ms   region-scoped from step 0");
            println!("           {:.1}x", whole_ms / scoped_ms);

            // ⚠ Per step, because the GPU-port decision turns on what lens correction costs
            // INSIDE the scoped chain rather than whole-buffer. Each step is run on its own
            // against the plan's own rects, so the sum is the chain with the carried buffers
            // rebuilt — slightly pessimistic, and the shape is what matters.
            println!("\n  the scoped chain, per step");
            let mut carried: Option<ImageBuf> = None;
            for (i, st) in steps.iter().enumerate() {
                let rsrc_owner = carried.clone();
                let ms = best_of(3, || {
                    let rsrc = match &rsrc_owner {
                        None => pipeline::RegionSrc::whole(&image),
                        Some(buf) => pipeline::RegionSrc {
                            buf,
                            origin: (plan.out_rects[i - 1].x, plan.out_rects[i - 1].y),
                            full: plan.dims[i],
                        },
                    };
                    st.stage
                        .apply_region(&rsrc, plan.out_rects[i])
                        .expect("every step in the plan accepts its rect")
                });
                let r = plan.out_rects[i];
                println!(
                    "{ms:8.1} ms   [{i}] {:16} -> {}x{} ({:.1} Mpx)",
                    st.stage.name(),
                    r.w,
                    r.h,
                    (r.w * r.h) as f64 / 1e6
                );
                let rsrc = match &rsrc_owner {
                    None => pipeline::RegionSrc::whole(&image),
                    Some(buf) => pipeline::RegionSrc {
                        buf,
                        origin: (plan.out_rects[i - 1].x, plan.out_rects[i - 1].y),
                        full: plan.dims[i],
                    },
                };
                carried = st.stage.apply_region(&rsrc, plan.out_rects[i]);
            }
        }
    }

    // ---- Question 4: what does the GPU stage actually cost, and does it agree? ----
    //
    // ⚠⚠ **The row the milestone is judged by, and the reason the spec projects nothing.**
    // Composing component measurements has missed in this crate three times — M8.3g over-predicted
    // a whole frame by ~2x, M8.3f was right in magnitude and wrong by 8x in attribution, M8.3o was
    // optimistic by ~18%. So this prices the real thing through the real seam rather than adding
    // up transfer costs.
    if let Some(ctx) = gpu_pipeline::shared() {
        println!(
            "\n  the GPU stage, on this photograph ({})",
            ctx.adapter_name()
        );
        let edits = edit_model::Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: want.distortion,
                tca: want.tca,
                vignetting: want.vignetting,
                scale: want.scale,
            }),
            ..edit_model::Edits::default()
        };
        match gpu_pipeline::accelerate_prefix_stage(
            "lens_correction",
            &edits,
            pipeline::RenderScale::FULL,
            &image,
            gpu_pipeline::SeamFacts {
                lens: Some(&shot),
                resampler: Resampler::default(),
                // ⚠ This probe accelerates lens correction, not dehaze, so the airlight is inert
                // here — but the seam takes it for every stage, so it is supplied honestly rather
                // than faked.
                airlight: pipeline::prefix_airlight(
                    &edits,
                    &image,
                    Resampler::default(),
                    Some(shot.clone()),
                ),
            },
            ctx,
        ) {
            None => println!("           —    the seam declined (no corrector, or below the gate)"),
            Some(g) => {
                let gpu_ms = best_of(3, || g.apply(image.clone()));
                println!("{whole:8.1} ms   the CPU stage");
                println!("{gpu_ms:8.1} ms   the same stage across the seam");
                println!("           {:.2}x", whole / gpu_ms);
                // ⚠ And whether it is the same photograph, in 8-bit levels — the only unit a
                // judgement about a picture is made in.
                let cpu_out = stage.apply(image.clone());
                let gpu_out = g.apply(image.clone());
                let d = gpu_pipeline::compare(&cpu_out, &gpu_out);
                println!(
                    "           worst {:.3} levels over {:.3}% of samples",
                    d.worst_levels,
                    d.differing_percent()
                );
            }
        }
    } else {
        println!("\n  no GPU adapter on this machine");
    }

    // ---- Question 5: what does a whole EXPORT cost, the three ways it can go? ----
    //
    // ⚠⚠ **The row the milestone is actually judged by.** The stage's own timing above is not an
    // export: an export also decodes, resamples the tail, encodes and writes. And the first row
    // here is the behaviour Rustroom shipped until today — `lens: None`, no correction at all —
    // so the middle row is deliberately **slower than before**, which is the fix rather than a
    // regression.
    {
        use rustroom_app::batch_export::{export_one, ExportRender, ExportSource};
        let dir = std::env::temp_dir().join("rr-probe-export");
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join("out.jpg");
        let o = rustroom_app::batch_export::ExportOptions {
            dest_dir: dir.clone(),
            quality: 90,
            ..Default::default()
        };
        let edits = edit_model::Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: want.distortion,
                tca: want.tca,
                vignetting: want.vignetting,
                scale: want.scale,
            }),
            ..edit_model::Edits::default()
        };
        let one = |lens: Option<LensShot>, gpu_tail: bool| {
            best_of(3, || {
                export_one(
                    ExportSource {
                        image: image.clone(),
                        baseline: color::wb::Illuminant::default(),
                        lens: lens.clone(),
                        curve: None,
                        baseline_ev: None,
                        scene_referred: true,
                    },
                    &edits,
                    &dest,
                    &o,
                    ExportRender {
                        resampler: Resampler::default(),
                        gpu_tail,
                    },
                    &export::FileMetadata::default(),
                    None,
                )
                .expect("export")
            })
        };
        // ⚠⚠ **The decode, because it is what a real export actually waits for.** Everything
        // above prices the render; a batch export decodes each file itself, and on an X-T4 that is
        // ~93% libraw's demosaic — entirely upstream of anything wgpu touches.
        let decode_ms = best_of(2, || {
            raw_decode::decode_raw(path, &opts).expect("decode").image
        });
        println!("\n  a whole export, {w}x{h}");
        println!("{decode_ms:8.1} ms   the DECODE, paid before any of the rows below");
        println!(
            "{:8.1} ms   no lens correction at all (what shipped until M8.9)",
            one(None, false)
        );
        println!(
            "{:8.1} ms   lens correction on, CPU",
            one(Some(shot.clone()), false)
        );
        if gpu_pipeline::shared().is_some() {
            println!(
                "{:8.1} ms   lens correction on, GPU",
                one(Some(shot.clone()), true)
            );
        } else {
            println!("           —    no GPU adapter on this machine");
        }
        let _ = std::fs::remove_file(&dest);
    }
}
