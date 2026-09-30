//! Region rendering: which part of the source a region of the output needs, planned backwards through the prefix.

use crate::PrefixStep;
use core_image::ImageBuf;

/// Where a cropped render sits inside the full frame.
///
/// `None` at a call site means "this buffer *is* the full frame" — what every existing caller
/// implicitly passes, and what position-dependent stages already assume. Only stages whose
/// output depends on absolute position need it; today that is the vignette alone, but any
/// future graduated or radial filter will want the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub full_w: usize,
    pub full_h: usize,
    pub x0: usize,
    pub y0: usize,
}

/// A pixel rectangle, in whichever coordinate space the caller names.
///
/// ⚠ Deliberately **not** [`Region`]: that type carries an origin plus the *frame's* size and takes
/// its own width and height from the buffer it describes. A stage producing a sub-rect cannot do
/// that, because the buffer does not exist yet — the same trap `render_cached`'s `clip` comment
/// already records for deriving a size as "frame minus origin".
///
/// ⚠⚠ **Deliberately not called `Rect` either, which is what it was until M8.7.**
/// [`PipelineStage::apply_region`] takes an *output* rect and [`PipelineStage::input_rect_for`]
/// returns an *input* one; a type whose name asserts one of those reads as a bug in the other
/// position. It also replaced `app::RegionRect`, which was this same struct with different field
/// names — two types that could not disagree because they were the same type twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    /// This rect clipped into a `w` x `h` image, or `None` if nothing survives.
    ///
    /// ⚠ Moved here from `app::RegionRect` so the one clipping rule lives beside the type rather
    /// than in whichever crate happened to need it first.
    pub fn clamped(self, w: usize, h: usize) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        let x = self.x.min(w - 1);
        let y = self.y.min(h - 1);
        let cw = self.w.min(w - x);
        let ch = self.h.min(h - y);
        (cw > 0 && ch > 0).then_some(Self { x, y, w: cw, h: ch })
    }

    /// This rect grown by `m` pixels on every side, clipped into `w` x `h`.
    ///
    /// ⚠ Saturating on the low side and clipping on the high side, so a margin larger than the
    /// image yields the whole image rather than an empty rect or a panic. A margin larger than the
    /// image is **ordinary** at low render scales, not a pathological case.
    pub fn grown(self, m: usize, w: usize, h: usize) -> Self {
        let x = self.x.saturating_sub(m);
        let y = self.y.saturating_sub(m);
        let x1 = (self.x + self.w + m).min(w);
        let y1 = (self.y + self.h + m).min(h);
        Self {
            x,
            y,
            w: x1.saturating_sub(x),
            h: y1.saturating_sub(y),
        }
    }
}

/// A stage's input when only part of it exists.
///
/// ⚠⚠ **`buf` is a sub-rect of a larger logical image, and both other fields are needed.**
/// `origin` says where `buf` sits in that image and `full` says how big the image is — a stage
/// whose result depends on *where* a pixel is (the rotation's centre, the homography's
/// normalisation) is wrong without them, and wrong as a plausible photograph of the wrong part of
/// the frame rather than as an error. That is M8.6's centre sabotage, which all 63 goldens were
/// blind to.
///
/// ⚠ The whole-buffer case is `origin == (0, 0)` with `full` equal to `buf`'s dimensions, which is
/// what makes M8.6's two implementations generalise rather than be rewritten.
#[derive(Debug, Clone, Copy)]
pub struct RegionSrc<'a> {
    pub buf: &'a ImageBuf,
    pub origin: (usize, usize),
    pub full: (usize, usize),
}

impl<'a> RegionSrc<'a> {
    /// The whole-buffer case: this buffer *is* the whole image.
    pub fn whole(buf: &'a ImageBuf) -> Self {
        Self {
            buf,
            origin: (0, 0),
            full: (buf.width(), buf.height()),
        }
    }

    /// The sample at a point in **full-image** coordinates, or `None` if `buf` does not hold it.
    ///
    /// ⚠ Returning `None` rather than clamping is deliberate: a stage reaching outside what
    /// [`region_plan`] promised is a bug in some `input_rect_for`, and clamping would hide it
    /// behind a plausible edge pixel.
    pub fn at(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        let lx = x.checked_sub(self.origin.0)?;
        let ly = y.checked_sub(self.origin.1)?;
        (lx < self.buf.width() && ly < self.buf.height()).then(|| self.buf.pixel(lx, ly))
    }
}

/// What each scoped step must produce, and the frame the whole path would have produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionPlan {
    /// One output rect per step in `steps[start..]`, in that order.
    pub out_rects: Vec<Rect>,
    /// The **full-image** dimensions each step's input has, `dims[i]` for `steps[start + i]`.
    ///
    /// ⚠⚠ The executor needs these to set each chained source's `full`, and they are *logical*
    /// dimensions rather than the size of the slice in hand. Deriving them from the buffers as the
    /// chain runs is precisely the bug `RegionSrc` exists to prevent.
    pub dims: Vec<(usize, usize)>,
    /// The dimensions the **whole-buffer** path would end with.
    ///
    /// ⚠⚠ Composed through [`PipelineStage::output_dims`] and read **before** any step runs.
    /// These feed `CoverageSpec { fw, fh }` and the tail's [`Region`], so taking them off a
    /// region-scoped buffer would rasterise every brush mask at the region's size and thrash the
    /// coverage cache on every pan. M8.6's sharpest hazard, generalised to a chain.
    pub frame: (usize, usize),
}

/// Plan a region-scoped run of `steps[start..]` that ends by producing `visible`.
///
/// Walks **backwards** from `visible`, asking each step what input it needs
/// ([`PipelineStage::input_rect_for`]) and making that the previous step's output.
///
/// ⚠⚠ **Any decline abandons the whole plan**, and that is structural rather than conservative: a
/// step that must run whole-buffer needs a whole-buffer input, so every step above it must produce
/// the full frame too. One `None` anywhere at or after `start` therefore kills everything. Two
/// consequences on real chains: spots block a Perspective drag from scoping above `geometry`, and
/// a masked capture — which is **last** — switches the feature off for that photo entirely.
///
/// ⚠ An empty range is `None` as well. Nothing to scope is **no plan**, not an empty one, so the
/// caller takes the whole-buffer path rather than believing it scoped something.
///
/// ⚠ Pure, and takes plain rects on purpose: margins, back-projection, composition and the abandon
/// rule are then testable with no render engine and no window.
pub fn region_plan(
    steps: &[PrefixStep],
    start: usize,
    visible: Rect,
    in_w: usize,
    in_h: usize,
) -> Option<RegionPlan> {
    if start >= steps.len() || visible.w == 0 || visible.h == 0 {
        return None;
    }
    // Forward first: the dimensions each step sees, and the frame the path ends at.
    let mut dims = Vec::with_capacity(steps.len() - start + 1);
    let (mut w, mut h) = (in_w, in_h);
    dims.push((w, h));
    for s in &steps[start..] {
        let (ow, oh) = s.stage.output_dims(w, h);
        dims.push((ow, oh));
        w = ow;
        h = oh;
    }
    let frame = (w, h);

    // Backwards: each step's required output, from the last to `start`.
    let n = steps.len() - start;
    let mut out_rects = vec![visible.clamped(frame.0, frame.1)?; n];
    for i in (0..n).rev() {
        let (iw, ih) = dims[i];
        let need = steps[start + i]
            .stage
            .input_rect_for(out_rects[i], iw, ih)?;
        if i > 0 {
            out_rects[i - 1] = need;
        }
    }
    Some(RegionPlan {
        out_rects,
        dims,
        frame,
    })
}

/// Execute a [`RegionPlan`]: run `steps[start..]` region-scoped, producing only the visible rect.
///
/// ⚠⚠ **The counterpart to [`region_plan`], and it belongs here rather than in the render engine.**
/// Chain execution is pure — only the caching and the gate need an engine — and putting it here is
/// what lets the whole mechanism be tested against the whole-buffer path **byte for byte with no
/// window**. Without that, `region_plan` could return plausible rects that no stage ever honours,
/// and every test would still pass: the W5 failure mode.
///
/// ⚠ Each chained source carries the **logical** full dimensions from the plan, never the size of
/// the slice in hand. That is the line M8.6's centre sabotage lives on.
///
/// Returns `None` if any step declines, which should not happen for a plan [`region_plan`]
/// produced — the two ask the same questions — but declining is always safe.
pub fn run_region_plan(
    steps: &[PrefixStep],
    start: usize,
    src: &ImageBuf,
    plan: &RegionPlan,
) -> Option<ImageBuf> {
    let n = plan.out_rects.len();
    if n == 0 || start + n != steps.len() {
        return None;
    }
    // ⚠⚠ Kept permanently, the `RUSTROOM_DEBUG_MASK` / `RUSTROOM_DEBUG_BRUSH` precedent, and added
    // because this path was a blind spot: `render_engine` prints a whole-render figure and a plan
    // summary for a scoped render, but its per-step decomposition sits in the WHOLE-BUFFER branch,
    // so a scoped drag reported a total with nothing under it. A scoped 0.7 Mpx dehaze drag
    // measured 118 ms against a 9-11 ms cache-hit frame (2026-09-16) and no line said where it
    // went.
    //
    // ⚠ The `margin` column is the point of it. `region_plan` grows the request by each stage's
    // `input_rect_for` on EVERY side and propagates that backward through the whole chain, so the
    // earliest rect is the largest and the work is NOT the visible region's area. Dehaze alone
    // reaches `2 * REFINE_RADIUS + PATCH_RADIUS` per side.
    let debug = std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some();
    let t_all = std::time::Instant::now();
    let visible = plan.out_rects[n - 1];
    let mut carried: Option<ImageBuf> = None;
    for i in 0..n {
        let region_src = match &carried {
            None => RegionSrc::whole(src),
            Some(buf) => RegionSrc {
                buf,
                origin: (plan.out_rects[i - 1].x, plan.out_rects[i - 1].y),
                full: plan.dims[i],
            },
        };
        let t = std::time::Instant::now();
        carried = Some(
            steps[start + i]
                .stage
                .apply_region(&region_src, plan.out_rects[i])?,
        );
        if debug {
            let r = plan.out_rects[i];
            let px = r.w as f64 * r.h as f64;
            eprintln!(
                "    scoped step {:>2} {:<18} out {:>5}x{:<5} {:6.3} Mpx  margin {:5.2}x  {:8.2} ms",
                start + i,
                steps[start + i].stage.name(),
                r.w,
                r.h,
                px / 1e6,
                px / (visible.w as f64 * visible.h as f64).max(1.0),
                t.elapsed().as_secs_f64() * 1e3,
            );
        }
    }
    if debug {
        let first = plan.out_rects[0];
        eprintln!(
            "    scoped TOTAL {} step(s)  visible {}x{} ({:.3} Mpx)  widest {}x{} ({:.3} Mpx, {:.2}x)  frame {}x{}  {:.2} ms",
            n,
            visible.w,
            visible.h,
            (visible.w as f64 * visible.h as f64) / 1e6,
            first.w,
            first.h,
            (first.w as f64 * first.h as f64) / 1e6,
            (first.w as f64 * first.h as f64) / (visible.w as f64 * visible.h as f64).max(1.0),
            plan.frame.0,
            plan.frame.1,
            t_all.elapsed().as_secs_f64() * 1e3,
        );
    }
    carried
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::structured;
    use crate::{PipelineStage, RenderScale};
    use edit_model::Edits;

    #[test]
    fn a_stage_with_no_region_support_declines_to_name_an_input_rect() {
        // ⚠ The default is what makes lens correction, spot removal and the masked captures
        // decline without the caller name-checking any of them.
        let s = crate::stages::exposure::ExposureStage::new(1.0);
        assert_eq!(
            s.input_rect_for(
                Rect {
                    x: 0,
                    y: 0,
                    w: 4,
                    h: 4
                },
                64,
                64
            ),
            None
        );
    }

    #[test]
    fn noise_reduction_grows_the_rect_by_its_reach_and_clips_to_the_image() {
        let p = edit_model::NoiseReduction {
            luminance: 0.0,
            color: 50.0,
            detail: 50.0,
            smoothness: 100.0,
        };
        let s = crate::stages::noise_reduction::NoiseReductionStage::new(&p, RenderScale::FULL);
        // Reach 64 (CHROMA_PASSES 2 x radius 32), interior rect: grows on every side.
        let got = s
            .input_rect_for(
                Rect {
                    x: 100,
                    y: 100,
                    w: 10,
                    h: 10,
                },
                1000,
                1000,
            )
            .unwrap();
        assert_eq!(
            got,
            Rect {
                x: 36,
                y: 36,
                w: 138,
                h: 138
            }
        );
        // ⚠ At the image's own corner it must CLIP rather than underflow or overrun. A margin
        // larger than the image is ordinary at low render scales.
        let got = s
            .input_rect_for(
                Rect {
                    x: 0,
                    y: 0,
                    w: 10,
                    h: 10,
                },
                20,
                20,
            )
            .unwrap();
        assert_eq!(
            got,
            Rect {
                x: 0,
                y: 0,
                w: 20,
                h: 20
            },
            "clipped to the whole image"
        );
    }

    #[test]
    fn dehaze_grows_the_rect_by_its_reach() {
        let s = crate::stages::dehaze::DehazeStage::new(
            &edit_model::Dehaze { amount: 60.0 },
            RenderScale::FULL,
        );
        let got = s
            .input_rect_for(
                Rect {
                    x: 500,
                    y: 500,
                    w: 10,
                    h: 10,
                },
                2000,
                2000,
            )
            .unwrap();
        assert_eq!(
            got,
            Rect {
                x: 360,
                y: 360,
                w: 290,
                h: 290
            },
            "140 on every side"
        );
    }

    #[test]
    fn the_back_projections_are_reachable_through_the_trait_object() {
        // ⚠⚠ **A test on the concrete type cannot tell a trait method from an inherent one.**
        // `input_rect_for` was written into the inherent impl of the (since deleted) `PerspectiveStage` once by mistake; both its own
        // tests passed, because `s.input_rect_for(..)` resolved to the inherent method — while the
        // TRAIT method still returned the default `None`. `region_plan` calls through
        // `dyn PipelineStage`, so it would have seen the feature as simply absent. Only clippy's
        // dead-code pass caught it, which is luck rather than a guard.
        //
        // ⚠ This asks the way `region_plan` asks. A stage that answers here answers there.
        let g = edit_model::Geometry {
            angle: 5.0,
            ..Default::default()
        };
        let geo: Box<dyn PipelineStage> = Box::new(crate::stages::geometry::GeometryStage::new(
            &g,
            crate::resample::Resampler::CatmullRom,
        ));
        assert!(
            geo.input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some(),
            "geometry must answer through the trait, not only on the concrete type"
        );

        let p = edit_model::Perspective {
            vertical: 40.0,
            ..Default::default()
        };
        // ⚠ Since M7.3b the keystone is `GeometryStage`'s fused path, a different `input_rect_for`
        // branch from the one above, so it is asked separately.
        let per: Box<dyn PipelineStage> = Box::new(
            crate::stages::geometry::GeometryStage::new(
                &edit_model::Geometry::default(),
                crate::resample::Resampler::CatmullRom,
            )
            .with_perspective(p),
        );
        assert!(
            per.input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some(),
            "perspective must answer through the trait, not only on the concrete type"
        );

        let nr = edit_model::NoiseReduction {
            luminance: 50.0,
            color: 50.0,
            detail: 50.0,
            smoothness: 50.0,
        };
        let n: Box<dyn PipelineStage> = Box::new(
            crate::stages::noise_reduction::NoiseReductionStage::new(&nr, RenderScale::FULL),
        );
        assert!(n
            .input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some());

        let d: Box<dyn PipelineStage> = Box::new(crate::stages::dehaze::DehazeStage::new(
            &edit_model::Dehaze { amount: 60.0 },
            RenderScale::FULL,
        ));
        assert!(d
            .input_rect_for(
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 20
                },
                400,
                300
            )
            .is_some());
    }

    #[test]
    fn a_region_src_reads_in_full_image_coordinates() {
        // ⚠⚠ Full-image coordinates, not the buffer's. A reader taking buffer coordinates would
        // shift every read by the origin — M8.6's centre sabotage, which the goldens cannot see.
        let mut buf = core_image::ImageBuf::new(4, 4);
        buf.set_pixel(1, 1, [0.5, 0.25, 0.125]);
        let src = RegionSrc {
            buf: &buf,
            origin: (10, 20),
            full: (100, 100),
        };
        assert_eq!(src.at(11, 21), Some([0.5, 0.25, 0.125]));
        assert_eq!(src.at(9, 21), None, "left of the region");
        assert_eq!(src.at(11, 19), None, "above the region");
        assert_eq!(src.at(14, 21), None, "right of the region");
        assert_eq!(src.at(11, 24), None, "below the region");
    }

    #[test]
    fn the_whole_buffer_case_is_the_identity() {
        // ⚠ This is what makes M8.6's two implementations generalise rather than be rewritten.
        let buf = core_image::ImageBuf::new(8, 6);
        let src = RegionSrc::whole(&buf);
        assert_eq!(src.origin, (0, 0));
        assert_eq!(src.full, (8, 6));
        assert_eq!(src.at(7, 5), Some(buf.pixel(7, 5)));
    }

    fn steps_for(e: &Edits) -> Vec<super::PrefixStep> {
        crate::build_prefix_steps(
            e,
            RenderScale::FULL,
            None,
            crate::resample::Resampler::CatmullRom,
            None,
        )
    }

    #[test]
    fn the_plan_walks_backwards_growing_the_rect_at_each_local() {
        let e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 50.0,
                detail: 50.0,
                smoothness: 100.0,
            },
            dehaze: edit_model::Dehaze { amount: 60.0 },
            ..Default::default()
        };
        let steps = steps_for(&e);
        assert_eq!(
            steps.len(),
            2,
            "fixture check: NR then dehaze, and nothing else"
        );
        let visible = Rect {
            x: 1000,
            y: 1000,
            w: 200,
            h: 200,
        };
        let plan = super::region_plan(&steps, 0, visible, 4000, 3000)
            .expect("every step here supports a region");

        // The LAST step produces exactly the visible rect.
        assert_eq!(*plan.out_rects.last().unwrap(), visible);
        // Dehaze reads 140 beyond it, so NR must produce that.
        assert_eq!(plan.out_rects[0], visible.grown(140, 4000, 3000));
    }

    #[test]
    fn a_declining_step_abandons_the_whole_plan() {
        // ⚠⚠ THE structural rule: a whole-buffer step needs a whole-buffer input, so one decline
        // anywhere at or after `start` kills everything above it too.
        //
        // ⚠ The straighten gives spot removal a step BELOW it: since M7.3b spots come after dehaze,
        // and without the geometry they would be last, leaving no contained case to start from.
        let e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            spots: vec![edit_model::Spot {
                centre: [0.5, 0.5],
                source: [0.3, 0.3],
                radius: 0.02,
                feather: 50.0,
                mode: Default::default(),
            }],
            ..Default::default()
        };
        let steps = steps_for(&e);
        let spot_at = steps
            .iter()
            .position(|s| s.stage.name() == "spot_removal")
            .expect("fixture check: the spot step must be in the prefix");
        let visible = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        assert!(
            super::region_plan(&steps, spot_at, visible, 400, 300).is_none(),
            "spot removal declines, so nothing from there on can be scoped"
        );
        assert!(
            super::region_plan(&steps, spot_at + 1, visible, 400, 300).is_some(),
            "starting below it is fine — that is the contained case"
        );
    }

    #[test]
    fn a_masked_capture_blocks_everything_because_it_is_last() {
        // ⚠⚠ The caveat as a test. Masked captures sit at the END of the prefix, so one declining
        // step there switches the feature off for the whole photo — there is nothing after it to
        // scope. A test covering only spots would miss this, and spots are the *contained* case.
        let m = edit_model::MaskedAdjustment {
            adjustments: edit_model::LocalAdjustments {
                dehaze: edit_model::Dehaze { amount: 40.0 },
                ..Default::default()
            },
            ..Default::default()
        };
        let e = Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            masks: vec![m],
            ..Default::default()
        };
        let steps = steps_for(&e);
        assert!(
            steps.iter().any(|s| s.stage.name() == "mask"),
            "fixture check: the masked capture step must actually be in the prefix"
        );
        let visible = Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        };
        assert!(
            super::region_plan(&steps, 0, visible, 400, 300).is_none(),
            "a masked capture is last, so nothing above it can be scoped either"
        );
    }

    #[test]
    fn the_plan_reports_the_frame_dimensions_the_whole_path_would_produce() {
        // ⚠⚠ M8.6's sharpest hazard, generalised. These feed CoverageSpec { fw, fh } and the
        // tail's Region { full_w, full_h }; taking them off a region-scoped buffer rasterises
        // every brush mask at the region's size and thrashes the coverage cache on every pan.
        let mut e = Edits::default();
        // ⚠ A field OF a field, which clippy's field_reassign_with_default allows.
        e.geometry.crop = edit_model::CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 0.5,
        };
        let steps = steps_for(&e);
        let plan = super::region_plan(
            &steps,
            0,
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            400,
            300,
        )
        .unwrap();
        assert_eq!(
            plan.frame,
            (200, 150),
            "the CROPPED frame, not the region and not the input"
        );
    }

    #[test]
    fn an_empty_step_list_has_no_plan() {
        // Nothing to scope is not a plan of length zero — it is no plan, so the caller takes the
        // whole-buffer path rather than believing it scoped something.
        assert!(super::region_plan(
            &[],
            0,
            Rect {
                x: 0,
                y: 0,
                w: 4,
                h: 4
            },
            64,
            64
        )
        .is_none());
    }

    /// Run a chain both ways and report `(differing samples, worst absolute difference)`.
    ///
    /// ⚠ `min_steps` is the fixture check: a chain shorter than the fixture means some stage the
    /// test depends on was not inserted, and it would then prove less than its name says.
    fn chain_vs_whole(
        e: &Edits,
        src: &core_image::ImageBuf,
        visible: Rect,
        min_steps: usize,
    ) -> (usize, f32) {
        let steps = steps_for(e);
        assert!(
            steps.len() >= min_steps,
            "fixture check: need a real chain, got {}",
            steps.len()
        );
        let plan = super::region_plan(&steps, 0, visible, src.width(), src.height())
            .expect("fixture check: every step must support a region");
        assert!(
            plan.out_rects[0].x > 0 || plan.out_rects[0].y > 0,
            "fixture check: the chain must carry a NON-ZERO origin, else it cannot see the centre \
             — got {:?}",
            plan.out_rects[0]
        );
        let scoped = super::run_region_plan(&steps, 0, src, &plan).expect("the chain runs");
        let mut whole = src.clone();
        for st in &steps {
            whole = st.stage.apply(whole);
        }
        let want = whole.crop(visible.x, visible.y, visible.w, visible.h);
        assert_eq!(
            (scoped.width(), scoped.height()),
            (want.width(), want.height())
        );
        let (mut n, mut worst) = (0usize, 0.0f32);
        for y in 0..want.height() {
            for x in 0..want.width() {
                let (a, b) = (scoped.pixel(x, y), want.pixel(x, y));
                for c in 0..3 {
                    worst = worst.max((a[c] - b[c]).abs());
                }
                if a != b {
                    n += 1;
                }
            }
        }
        (n, worst)
    }

    /// A synthetic lens with a **barrel** correction, which is the sign whose bulge a corner
    /// bound would miss — see `LensCorrectionStage`'s own margin test.
    fn test_lens() -> lens_profile::LensShot {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="-0.05" c="0.0"/>
                    <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        let profile = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        lens_profile::LensShot {
            profile,
            shot: lens_profile::Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        }
    }

    /// ⚠⚠ **THE CARRIER, and it asserts the plan's LENGTH rather than merely that scoping
    /// happened.** A scoped render and a whole-buffer one produce the same picture by design, so
    /// every correctness test in this workspace — all 63 goldens included — passes with lens
    /// correction's `input_rect_for` deleted. That is exactly how creative LUTs (W5) shipped fully
    /// tested and 100% non-functional.
    ///
    /// ⚠⚠ **And a test asking only "did it scope" would pass against M8.7**, which is the version
    /// that left a cold entry into 1:1 at 2392-2528 ms and `scoped no`: there `region_plan` was
    /// only ever reached with `start >= 1`, because step 0 is `lens_correction` and it declined,
    /// and one decline anywhere abandons the whole plan. The whole of M8.8 is that `start == 0`
    /// now succeeds, so the assertion has to be that **every** step is covered.
    ///
    /// ⚠ All gathers, so the bytes can be compared exactly — `chain_vs_whole`'s own standard.
    #[test]
    fn a_cold_chain_scopes_from_lens_correction() {
        let mut e = Edits {
            perspective: edit_model::Perspective {
                vertical: 20.0,
                ..Default::default()
            },
            ..Default::default()
        };
        e.geometry.angle = 6.0;
        e.geometry.crop = edit_model::CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        e.lens_correction = Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        });
        let steps = crate::build_prefix_steps(
            &e,
            RenderScale::FULL,
            Some(test_lens()),
            crate::resample::Resampler::CatmullRom,
            None,
        );
        assert_eq!(
            steps[0].stage.name(),
            "lens_correction",
            "fixture check: lens correction must be STEP 0, which is what made the chain abandon"
        );
        assert_eq!(
            steps.len(),
            2,
            "fixture check: lens, then geometry with the keystone fused into it"
        );

        let src = structured(220, 180);
        let visible = Rect {
            x: 40,
            y: 30,
            w: 50,
            h: 40,
        };
        let plan = super::region_plan(&steps, 0, visible, src.width(), src.height())
            .expect("a COLD chain must scope from step 0");
        assert_eq!(
            plan.out_rects.len(),
            steps.len(),
            "every step from the resume point must be scoped"
        );
        // ⚠ The margin must actually have grown the request, or the chain is scoping a rect that
        // needs no margin and the test says nothing about the bound.
        let lens_in = plan.out_rects[0];
        assert!(
            lens_in.w > visible.w && lens_in.h > visible.h,
            "fixture check: the margin must grow the request, got {lens_in:?}"
        );

        let scoped = super::run_region_plan(&steps, 0, &src, &plan).expect("the chain runs");
        let mut whole = src.clone();
        for st in &steps {
            whole = st.stage.apply(whole);
        }
        let want = whole.crop(visible.x, visible.y, visible.w, visible.h);
        assert_eq!(
            (scoped.width(), scoped.height()),
            (want.width(), want.height())
        );
        let mut differing = 0usize;
        let mut worst = 0.0f32;
        for y in 0..want.height() {
            for x in 0..want.width() {
                let (a, b) = (scoped.pixel(x, y), want.pixel(x, y));
                for c in 0..3 {
                    worst = worst.max((a[c] - b[c]).abs());
                }
                if a != b {
                    differing += 1;
                }
            }
        }
        assert_eq!(
            differing, 0,
            "a chain of gathers must be byte for byte; worst {worst:e}"
        );
    }

    #[test]
    fn a_chain_of_gathers_is_byte_for_byte() {
        // ⚠⚠ **THE test the centre sabotage needs, and it needs no render engine.** Chain
        // execution is pure; only the caching and the gate need one. Without this, `region_plan`
        // could return plausible rects that no stage honours and every other test would pass —
        // the W5 failure mode, which is why this was pulled forward out of plan 2.
        //
        // ⚠ Gathers only, so bit-identity is achievable: an output pixel reads its source and
        // writes only itself, with no accumulation whose order could differ. That is M8.6's own
        // standard, and `a_chain_with_a_local_agrees_to_within_a_rounding` records why a chain
        // carrying noise reduction cannot meet it.
        let mut e = Edits {
            perspective: edit_model::Perspective {
                vertical: 20.0,
                ..Default::default()
            },
            ..Default::default()
        };
        e.geometry.angle = 6.0;
        e.geometry.crop = edit_model::CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        let src = structured(220, 180);
        // ⚠ ONE step since M7.3b: perspective, straighten and crop are a single fused gather now,
        // which is what M7.3b is for. The multi-gather chain is still pinned byte for byte by
        // `a_cold_chain_scopes_from_lens_correction` (lens, then the fused geometry).
        let (differing, worst) = chain_vs_whole(
            &e,
            &src,
            Rect {
                x: 40,
                y: 30,
                w: 50,
                h: 40,
            },
            1,
        );
        assert_eq!(
            differing, 0,
            "a chain of gathers must be byte for byte; worst {worst:e}"
        );
    }

    #[test]
    fn a_chain_with_a_local_agrees_to_within_a_rounding() {
        // ⚠⚠ **Byte-for-byte is NOT achievable here, and that is a property of the operator rather
        // than a defect.** `box_blur` keeps a running sum, so it reaches each position by a
        // different history of adds and subtracts depending on where the row starts — the third
        // CPU divergence class this project records, which shows up between two CPU runs too.
        //
        // ⚠ Measured at **3.6e-7 absolute, about ONE ULP and 0.00009 of an 8-bit level**. The
        // bound below is ~28x that, which is loose enough to survive a re-tune of the radii and far
        // tighter than anything visible. ⚠ A margin bug reads as a *band at one edge* and would
        // blow this by orders of magnitude, so the bound still catches the failure it matters for.
        let mut e = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 0.0,
                color: 40.0,
                detail: 50.0,
                smoothness: 0.0,
            },
            ..Default::default()
        };
        e.geometry.angle = 6.0;
        e.geometry.crop = edit_model::CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        let src = structured(220, 180);
        let (_, worst) = chain_vs_whole(
            &e,
            &src,
            Rect {
                x: 40,
                y: 30,
                w: 50,
                h: 40,
            },
            2,
        );
        assert!(
            worst < 1e-5,
            "a running sum may wobble in the last bits; a margin bug may not. worst {worst:e}"
        );
    }
}
