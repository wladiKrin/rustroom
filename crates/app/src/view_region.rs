//! Pure viewport maths for region rendering: which part of the image the canvas needs, and
//! whether what is already rendered still covers it.
//!
//! Deliberately free of `egui` types so it can be tested directly — this is where clamping
//! and off-by-one mistakes would otherwise only show up as a visibly wrong image.

pub use crate::render_engine::RegionRect;

// ⚠⚠ **There is deliberately no `REGION_MARGIN` constant, and one must not come back.**
// It was 192 px, checked against a worst case of 135. Masks now carry spatial adjustments and
// **chain**, so N masked clarities reach N x 120 px into the input: the stack is unbounded and no
// constant covers it. The margin is derived per render by `Session::render_margin`, from the same
// `pipeline::tail_kernel_radius` the pipeline itself uses — one fact, read twice, because a second
// derivation here would fail silently as a subtly wrong band at the edge of the visible area.

/// The long edge a fit-resolution preview is capped to.
///
/// ⚠ Lives here, not in `main.rs`, because both the **lib** (`render_engine`, computing a
/// frame's `fit_size`) and the **bin** read it. Two constants that must agree is the shape this
/// project has been bitten by three times; the bin re-exports this one rather than declaring its
/// own.
pub const PREVIEW_MAX_DIM: usize = 1600;

/// The closest the view can be zoomed out: fit, where the whole image is on screen.
pub const MIN_ZOOM: f32 = 1.0;
/// The furthest in. Beyond this the render ladder has no more rungs to climb.
pub const MAX_ZOOM: f32 = 8.0;

/// The new zoom after a gesture, clamped to the usable range.
///
/// ⚠ `factor` is **multiplicative and 1.0 when idle**, matching `egui::InputState::zoom_delta`,
/// which folds trackpad pinch together with ctrl+scroll. It is not a delta: treating it as one
/// would collapse the zoom to nothing on every idle frame.
pub fn zoom_step(zoom: f32, factor: f32) -> f32 {
    (zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The pan correction that keeps the point under the cursor fixed while zooming.
///
/// `pointer` and `anchor` are `(x, y)` in the same screen space; the result is `(dx, dy)` to add
/// to the pan. Magnifying by `r = new/old` moves a point at offset `d` from the anchor to `d*r`,
/// so `d*(1-r)` puts it back.
///
/// ⚠ Plain tuples rather than `egui::Vec2`: this module is deliberately free of `egui` types so
/// it can be tested without a window, and every other function here holds to that. The caller
/// converts at the boundary, as it already does for `RegionRect`.
pub fn zoom_anchor_shift(
    pointer: (f32, f32),
    anchor: (f32, f32),
    old: f32,
    new: f32,
) -> (f32, f32) {
    if old <= f32::EPSILON {
        return (0.0, 0.0);
    }
    let r = new / old;
    (
        (pointer.0 - anchor.0) * (1.0 - r),
        (pointer.1 - anchor.1) * (1.0 - r),
    )
}

/// The working resolution to render at, chosen from a ladder of rungs that double from fit
/// resolution up to the buffer's own.
///
/// `fit` is the scale that makes the whole image fit the viewport and `zoom` the user's factor
/// on top, so `fit * zoom` is screen pixels per *fit-render* pixel. The chosen rung is the
/// smallest one at which the texture is not being magnified.
///
/// Rungs **double** so the on-screen scale can never fall below ~0.5 — rendering much more than
/// is displayed is what makes a region render expensive. And they are **quantized** rather than
/// fitted exactly, because a `max_dim` varying continuously with zoom would rebuild the prefix
/// cache on every scroll tick.
///
/// The ladder falls out of the buffer rather than being enumerated, so a half-size buffer
/// yields two rungs and a full-resolution one yields three, on any sensor, with nothing to
/// extend when the buffer is upgraded underneath it.
pub fn tier_max_dim(fit_long: usize, buffer_long: usize, fit: f32, zoom: f32) -> usize {
    let fit_long = fit_long.max(1);
    let needed = (fit * zoom * fit_long as f32).ceil().max(1.0) as usize;
    let mut rung = fit_long;
    while rung < needed && rung < buffer_long {
        rung = (rung * 2).min(buffer_long);
    }
    rung
}

/// Whether a chosen rung renders a *region* rather than the whole image.
pub fn is_native(max_dim: usize, fit_long: usize) -> bool {
    max_dim > fit_long
}

/// The rung `n` steps below `rung` on the ladder [`tier_max_dim`] walks, floored at `fit_long`.
///
/// ⚠⚠ **It rebuilds the ladder and steps back along it; it must never halve.** The ladder doubles
/// from `fit_long` and **clamps** at `buffer_long`, so its top rung is uneven — 6246, not 6400 —
/// and halving that gives 3123, a value [`tier_max_dim`] can never produce. Feeding one to the
/// renderer mints a new `max_dim`, hence a new `CacheKey`, and grows the prefix cache's key set
/// without bound while looking entirely correct on screen.
///
/// ⚠ `n == 0` returns `rung` unchanged, so the disabled path is the **same code** rather than a
/// branch around it — there is no second route through the renderer to keep in step.
///
/// ⚠ A `rung` that is not on the ladder is returned unchanged rather than snapped to one. That
/// means a bug upstream, and inventing a nearby rung would hide it behind a fabricated `max_dim`.
pub fn rung_below(rung: usize, fit_long: usize, buffer_long: usize, n: u32) -> usize {
    if n == 0 {
        return rung;
    }
    let fit_long = fit_long.max(1);
    let mut ladder = vec![fit_long];
    let mut r = fit_long;
    while r < buffer_long {
        r = (r * 2).min(buffer_long);
        ladder.push(r);
    }
    let Some(i) = ladder.iter().position(|&v| v == rung) else {
        return rung;
    };
    ladder[i.saturating_sub(n as usize)]
}

/// The long edge a fit render is *capped* to — the ladder's bottom rung.
///
/// ⚠ **This is not the same as the size a fit render comes out at, and confusing the two is a
/// feedback loop.** `downscale_to` caps the pre-geometry image at this value; geometry then
/// crops, so the resulting post-geometry frame is `fit_cap × crop_factor`, which is smaller.
/// Feeding *that* back in as the next render's cap shrinks it again every frame — with a 0.59
/// crop, 1600 → 941 → 553 → 325 → a few pixels within a second. Uncropped photos have a crop
/// factor of 1 and sit at a fixed point, so the bug hides until someone crops something.
///
/// Depends only on the buffer, so it is stable across crops, straightens and the
/// full-resolution upgrade alike.
pub fn fit_cap(buffer_long: usize, preview_max: usize) -> usize {
    buffer_long.min(preview_max)
}

/// The post-geometry image size **at the fit cap**, in exact floats.
///
/// ⚠⚠ **This is what `Session::fit_image_size` means, and deriving it from a rendered frame is
/// what made that value depend on which rung last landed.** A frame's dimensions come from
/// [`core_image::ImageBuf::downscale_to`], which rounds; scaling them back up by
/// `fit_cap / max_dim` multiplies that rounding by `rung / fit_cap`. Measured on an uncropped
/// X-T4 frame, the height read 1068.000 at rungs 1600 and 3200 against a true 1068.204 at 6246.
/// Computed here from the buffer and the geometry there is no rounding to amplify, and the result
/// is identical at every rung by construction.
///
/// ⚠ `geometry` must be the **view** geometry — what the render was asked for — not
/// `Session::edits.geometry`. Geometry mode resets the crop for the preview, so the two differ
/// exactly when the crop overlay is on screen and depending on it.
///
/// ⚠ The orientation is applied before the crop, matching `GeometryStage`: a quarter turn
/// transposes the frame and the crop fractions then apply to the turned one.
pub fn fit_size(
    buffer_w: usize,
    buffer_h: usize,
    fit_cap: usize,
    geometry: &edit_model::Geometry,
) -> (f32, f32) {
    let long = buffer_w.max(buffer_h).max(1) as f32;
    let ratio = fit_cap.max(1) as f32 / long;
    let (mut w, mut h) = (buffer_w as f32 * ratio, buffer_h as f32 * ratio);
    if geometry.orientation.transposes() {
        std::mem::swap(&mut w, &mut h);
    }
    let c = geometry.crop;
    // ⚠ Floored at one pixel rather than trusted: every consumer divides by this, and a zero
    // buffer or a degenerate crop would otherwise produce an infinity that `clamp` cannot repair
    // — the NaN trap `crop_tool` already documents.
    (
        (w * (c.right - c.left)).max(1.0),
        (h * (c.bottom - c.top)).max(1.0),
    )
}

/// The sub-rect of an `image_w` x `image_h` image visible in a viewport of `avail_w` x
/// `avail_h` screen pixels, drawn at `scale` screen pixels per image pixel and panned by
/// `pan_x`/`pan_y` screen pixels.
///
/// Always returns an in-bounds, non-empty rect: panning far past an edge clamps rather than
/// producing an empty or negative region.
pub fn visible_rect(
    image_w: usize,
    image_h: usize,
    avail_w: f32,
    avail_h: f32,
    scale: f32,
    pan_x: f32,
    pan_y: f32,
) -> RegionRect {
    let axis = |image: usize, avail: f32, pan: f32| -> (usize, usize) {
        let image_f = image as f32;
        // Screen-space span of the image, centred in the viewport then panned.
        let drawn = image_f * scale;
        let left = (avail - drawn) * 0.5 + pan;
        // Image-space coordinate at each viewport edge. The low edge floors and the high edge
        // ceils, so the rect always *covers* the viewport rather than falling short of it.
        let lo = (-left / scale).floor().max(0.0);
        let hi = ((avail - left) / scale).ceil().min(image_f);
        let lo = lo.min(image_f - 1.0) as usize;
        let hi = (hi as usize).max(lo + 1).min(image);
        (lo, hi - lo)
    };
    let (x, w) = axis(image_w, avail_w, pan_x);
    let (y, h) = axis(image_h, avail_h, pan_y);
    RegionRect { x, y, w, h }
}

/// Expand by `margin` on every side, clipped to the image. Rendering a little more than is
/// visible means small pans reuse the existing texture instead of re-rendering.
pub fn with_margin(r: RegionRect, image_w: usize, image_h: usize, margin: usize) -> RegionRect {
    let x = r.x.saturating_sub(margin);
    let y = r.y.saturating_sub(margin);
    let right = (r.x + r.w + margin).min(image_w);
    let bottom = (r.y + r.h + margin).min(image_h);
    RegionRect {
        x,
        y,
        w: right - x,
        h: bottom - y,
    }
}

/// Whether a visible rect spans the entire image, so there is no point cropping to it.
///
/// ⚠ This, not "the rung is above the fit cap", is what decides a *region* render. The two were
/// conflated while `fit` was clamped to 1.0 — a higher rung then only ever happened when zoomed,
/// so the visible area was always a strict subset. Once `fit` may exceed 1.0 (a window larger
/// than the fit texture) the whole image can be visible *at* a higher rung, and rendering it as
/// a region would stop `fit_image_size` ever refreshing: it is only taken from whole-image
/// frames, so a crop applied outside Geometry mode would leave it stale and drift the placement.
pub fn covers_whole_image(vis: RegionRect, image_w: usize, image_h: usize) -> bool {
    vis.x == 0 && vis.y == 0 && vis.w >= image_w && vis.h >= image_h
}

/// Whether `inner` lies entirely within `outer` — the test for "the rendered region still
/// covers what is on screen, so nothing needs re-rendering".
pub fn contains(outer: RegionRect, inner: RegionRect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_doubles_from_fit_to_the_buffer() {
        // Pre-upgrade: a 3123 buffer gives two rungs — exactly the behaviour before the
        // full-resolution work existed.
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 0.5), 1600);
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 1.0), 1600); // scale exactly 1.0 still fits
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 1.01), 3123);
        assert_eq!(tier_max_dim(1600, 3123, 1.0, 9.0), 3123); // capped: no more data

        // Post-upgrade: a 6246 buffer gives three, with no enum to extend.
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 1.0), 1600);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 1.01), 3200);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 2.0), 3200);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 2.01), 6246);
        assert_eq!(tier_max_dim(1600, 6246, 1.0, 9.0), 6246);
    }

    #[test]
    fn rung_below_is_the_identity_at_zero() {
        // ⚠ The disabled path is the SAME CODE, not a branch around it — there is no second
        // route through the renderer to keep in step.
        for rung in [1600, 3200, 6246] {
            assert_eq!(rung_below(rung, 1600, 6246, 0), rung);
        }
    }

    #[test]
    fn rung_below_steps_down_the_uneven_top_rung() {
        // ⚠⚠ **The halving trap.** The ladder doubles from fit_cap and *clamps* at buffer_long,
        // so its top rung is 6246, not 6400. Halving gives 3123 — a value `tier_max_dim` can
        // never produce, which would mint a new max_dim, a new CacheKey, and grow the prefix
        // cache's key set without bound while looking entirely correct on screen.
        assert_eq!(rung_below(6246, 1600, 6246, 1), 3200);
        assert_ne!(rung_below(6246, 1600, 6246, 1), 3123);
        assert_eq!(rung_below(3200, 1600, 6246, 1), 1600);
    }

    #[test]
    fn rung_below_floors_at_the_fit_cap() {
        assert_eq!(rung_below(6246, 1600, 6246, 2), 1600);
        // Deeper than the ladder is deep still lands on the fit rung rather than below it.
        assert_eq!(rung_below(6246, 1600, 6246, 9), 1600);
        assert_eq!(rung_below(1600, 1600, 6246, 1), 1600);
        // A buffer no larger than the fit cap has a one-rung ladder and nowhere to go.
        assert_eq!(rung_below(1200, 1200, 1200, 2), 1200);
    }

    #[test]
    fn rung_below_returns_a_real_ladder_rung() {
        // The property the halving trap violates, asserted over the whole ladder: every result
        // must be a value `tier_max_dim` itself can produce.
        for &buffer in &[3123usize, 6246, 7728] {
            let reachable: Vec<usize> = (0..=48)
                .map(|i| tier_max_dim(1600, buffer, 1.0, 1.0 + i as f32 * 0.25))
                .collect();
            for &rung in &reachable {
                for n in 0..=4 {
                    let got = rung_below(rung, 1600, buffer, n);
                    assert!(
                        reachable.contains(&got),
                        "rung_below({rung}, 1600, {buffer}, {n}) = {got}, \
                         which tier_max_dim cannot produce"
                    );
                }
            }
        }
    }

    #[test]
    fn rung_below_leaves_a_rung_it_does_not_recognise_alone() {
        // A rung off the ladder means a bug elsewhere. Returning it unchanged is the answer that
        // invents nothing; stepping back from a guessed position would fabricate a max_dim.
        assert_eq!(rung_below(2500, 1600, 6246, 1), 2500);
    }

    #[test]
    fn a_rung_below_the_buffer_never_magnifies() {
        // ⚠ The invariant the ladder exists for, in the precise form — the loose version
        // ("scale is always in (0.5, 1.0]") is false twice over and would fail here. At the
        // fit rung there is no lower bound (zoomed out, fit is arbitrarily small), and at the
        // top rung magnification is unavoidable because the file holds no more data.
        for buffer in [3123_usize, 6246] {
            let mut z = 0.01_f32;
            while z <= 12.0 {
                let m = tier_max_dim(1600, buffer, 1.0, z);
                let scale = z * 1600.0 / m as f32;
                if m > 1600 {
                    assert!(
                        scale > 0.5 - 1e-4,
                        "buffer {buffer} zoom {z}: renders {:.1}x the displayed pixels",
                        1.0 / scale
                    );
                }
                if m > 1600 && m < buffer {
                    assert!(
                        scale <= 1.0 + 1e-4,
                        "buffer {buffer} zoom {z}: magnifies its own texture (scale {scale})"
                    );
                }
                z += 0.01;
            }
        }
    }

    #[test]
    fn the_ladder_is_quantized_not_continuous() {
        // A `max_dim` varying continuously with zoom would rebuild the prefix cache on every
        // scroll tick, which is the whole reason for rungs rather than an exact fit.
        let rung = tier_max_dim(1600, 6246, 1.0, 1.2);
        for z in [1.05_f32, 1.3, 1.6, 1.9, 2.0] {
            assert_eq!(
                tier_max_dim(1600, 6246, 1.0, z),
                rung,
                "zoom {z} left the rung"
            );
        }
    }

    #[test]
    fn a_tiny_image_has_only_one_rung() {
        // An image smaller than the preview cap: `fit_long == buffer_long`, so there is
        // nothing to climb to.
        for z in [0.1_f32, 1.0, 5.0] {
            assert_eq!(tier_max_dim(800, 800, 1.0, z), 800);
        }
        // Degenerate inputs must neither divide by zero nor loop forever.
        assert!(tier_max_dim(0, 0, 1.0, 2.0) >= 1);
        assert!(tier_max_dim(1600, 0, 1.0, 4.0) >= 1);
    }

    #[test]
    fn a_fully_visible_image_is_not_worth_cropping_to() {
        // Zoomed out or exactly filling: the visible rect is the whole image, so a region
        // render would gain nothing and would cost the fit size its only source of truth.
        let whole = visible_rect(100, 60, 400.0, 300.0, 1.0, 0.0, 0.0);
        assert!(covers_whole_image(whole, 100, 60));

        // Zoomed in: a strict subset, so a region render is exactly right.
        let part = visible_rect(100, 60, 40.0, 30.0, 2.0, 0.0, 0.0);
        assert!(!covers_whole_image(part, 100, 60));
    }

    #[test]
    fn the_fit_cap_is_stable_while_a_cropped_render_shrinks() {
        // ⚠ The bug this exists to prevent, as a property. The cap must depend only on the
        // buffer — feeding a *rendered* (post-geometry) size back in compounds the crop every
        // frame. Simulated here: one photo cropped to 59% of its long edge.
        const P: usize = 1600;
        assert_eq!(fit_cap(3123, P), P);
        assert_eq!(
            fit_cap(6246, P),
            P,
            "the upgrade must not move the base rung"
        );
        assert_eq!(
            fit_cap(800, P),
            800,
            "an image smaller than the cap fits whole"
        );

        // What the broken version did, for contrast: the rendered size as the next cap. 0.588
        // is DSCF6237's actual crop of the long edge — the one photo in the test folder with
        // a crop, which is exactly why it alone broke while uncropped ones sat at a fixed
        // point.
        let mut rendered = P;
        for _ in 0..10 {
            rendered = (rendered as f32 * 0.588) as usize;
        }
        assert!(
            rendered < 20,
            "the feedback loop should collapse within a few frames, got {rendered}"
        );
        // ...against the fixed cap, which does not move at all.
        assert_eq!(fit_cap(3123, P), P);
    }

    #[test]
    fn is_native_is_anything_above_the_fit_rung() {
        assert!(!is_native(1600, 1600));
        assert!(is_native(3200, 1600));
        assert!(is_native(6246, 1600));
    }

    #[test]
    fn a_fully_visible_image_needs_all_of_it() {
        // Viewport larger than the image at 1:1 — the whole image is visible.
        let v = visible_rect(100, 60, 400.0, 300.0, 1.0, 0.0, 0.0);
        assert_eq!(
            v,
            RegionRect {
                x: 0,
                y: 0,
                w: 100,
                h: 60
            }
        );
    }

    #[test]
    fn zooming_in_needs_only_the_centre() {
        // 100x60 image at 4x in a 200x120 viewport shows 50x30 image pixels, centred.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, 0.0, 0.0);
        assert_eq!(
            v,
            RegionRect {
                x: 25,
                y: 15,
                w: 50,
                h: 30
            }
        );
    }

    #[test]
    fn panning_moves_the_visible_rect_and_clamps_at_the_edges() {
        // Panning the image right by 100 screen px at 4x moves the view 25 image px left.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, 100.0, 0.0);
        assert_eq!(
            v,
            RegionRect {
                x: 0,
                y: 15,
                w: 50,
                h: 30
            }
        );
        // Far past the edge still yields a valid in-bounds rect.
        let v = visible_rect(100, 60, 200.0, 120.0, 4.0, -10_000.0, 0.0);
        assert_eq!(v.x + v.w, 100);
    }

    #[test]
    fn the_margin_expands_and_clips_at_the_image_edge() {
        let r = with_margin(
            RegionRect {
                x: 40,
                y: 30,
                w: 20,
                h: 10,
            },
            100,
            60,
            8,
        );
        assert_eq!(
            r,
            RegionRect {
                x: 32,
                y: 22,
                w: 36,
                h: 26
            }
        );
        // At a corner the margin is clipped, not wrapped or negative.
        let r = with_margin(
            RegionRect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            100,
            60,
            8,
        );
        assert_eq!(
            r,
            RegionRect {
                x: 0,
                y: 0,
                w: 18,
                h: 18
            }
        );
    }

    #[test]
    fn containment_decides_whether_to_re_render() {
        let rendered = RegionRect {
            x: 10,
            y: 10,
            w: 50,
            h: 50,
        };
        assert!(contains(
            rendered,
            RegionRect {
                x: 20,
                y: 20,
                w: 10,
                h: 10
            }
        ));
        assert!(contains(rendered, rendered));
        assert!(!contains(
            rendered,
            RegionRect {
                x: 5,
                y: 20,
                w: 10,
                h: 10
            }
        ));
        assert!(!contains(
            rendered,
            RegionRect {
                x: 20,
                y: 20,
                w: 100,
                h: 10
            }
        ));
    }

    #[test]
    fn zoom_step_clamps_to_the_usable_range() {
        assert_eq!(zoom_step(1.0, 0.5), MIN_ZOOM, "cannot zoom out past fit");
        assert_eq!(zoom_step(8.0, 2.0), MAX_ZOOM, "cannot zoom in past the cap");
        assert!((zoom_step(2.0, 1.5) - 3.0).abs() < 1e-6);
    }

    #[test]
    fn an_idle_gesture_does_not_change_the_zoom() {
        // ⚠ `egui::InputState::zoom_delta()` returns 1.0 when idle, NOT 0.0 — it is a
        // multiplicative factor, not a delta. Treating it as a delta would collapse the zoom
        // to nothing on every frame in which the user does nothing at all.
        assert_eq!(zoom_step(3.5, 1.0), 3.5);
    }

    #[test]
    fn zoom_anchor_shift_keeps_the_cursor_point_fixed() {
        // Magnifying by r moves a point at offset d from the anchor to d*r, so the pan must
        // move by d*(1-r) to put it back. Sign errors here are invisible in review and
        // obvious in use, which is the whole reason this is a pure function.
        let shift = zoom_anchor_shift((110.0, 220.0), (100.0, 200.0), 1.0, 2.0);
        assert!((shift.0 - -10.0).abs() < 1e-6, "{shift:?}");
        assert!((shift.1 - -20.0).abs() < 1e-6, "{shift:?}");
    }

    #[test]
    fn zooming_out_shifts_the_opposite_way() {
        // ⚠ The discriminating case: a sign error that negates the whole expression passes the
        // test above only if it also passes this one with the same sign, which it cannot.
        let shift = zoom_anchor_shift((110.0, 220.0), (100.0, 200.0), 2.0, 1.0);
        assert!((shift.0 - 5.0).abs() < 1e-6, "{shift:?}");
        assert!((shift.1 - 10.0).abs() < 1e-6, "{shift:?}");
    }

    #[test]
    fn a_cursor_on_the_anchor_needs_no_shift() {
        let shift = zoom_anchor_shift((100.0, 200.0), (100.0, 200.0), 1.0, 4.0);
        assert_eq!(shift, (0.0, 0.0));
    }

    #[test]
    fn the_fit_size_is_the_buffer_scaled_to_the_cap() {
        // An uncropped X-T4 frame: 6246x4170 capped at 1600.
        let g = edit_model::Geometry::default();
        let (w, h) = fit_size(6246, 4170, 1600, &g);
        assert!((w - 1600.0).abs() < 1e-3, "width {w}");
        // ⚠ The value the shipped code gets *wrong*: 4170 * 1600 / 6246 = 1068.204, where
        // deriving it from a rounded 2136-row frame at rung 3200 gives exactly 1068.0.
        assert!((h - 1068.2036).abs() < 1e-3, "height {h}");
    }

    #[test]
    fn the_fit_size_follows_the_crop() {
        let g = edit_model::Geometry {
            crop: edit_model::CropRect {
                left: 0.25,
                top: 0.0,
                right: 0.75,
                bottom: 0.5,
            },
            ..edit_model::Geometry::default()
        };
        let (w, h) = fit_size(6246, 4170, 1600, &g);
        assert!((w - 800.0).abs() < 1e-3, "width {w}");
        assert!((h - 534.1018).abs() < 1e-3, "height {h}");
    }

    /// ⚠⚠ A quarter turn **transposes**, and forgetting it is invisible on a square fixture and
    /// on any test that only checks the long edge. `GeometryStage` applies the orientation before
    /// the crop, so the crop fractions apply to the *turned* frame.
    #[test]
    fn a_quarter_turn_transposes_the_fit_size() {
        let g = edit_model::Geometry {
            orientation: edit_model::Orientation {
                rotate: edit_model::Quarter::Cw90,
                mirror: false,
            },
            ..edit_model::Geometry::default()
        };
        let (w, h) = fit_size(6246, 4170, 1600, &g);
        assert!((w - 1068.2036).abs() < 1e-3, "width {w}");
        assert!((h - 1600.0).abs() < 1e-3, "height {h}");
        // A half turn does not transpose, which is what stops "any rotation swaps" passing.
        let g2 = edit_model::Geometry {
            orientation: edit_model::Orientation {
                rotate: edit_model::Quarter::Cw180,
                mirror: false,
            },
            ..edit_model::Geometry::default()
        };
        let (w2, h2) = fit_size(6246, 4170, 1600, &g2);
        assert!((w2 - 1600.0).abs() < 1e-3, "width {w2}");
        assert!((h2 - 1068.2036).abs() < 1e-3, "height {h2}");
    }

    /// A degenerate buffer must not produce a zero or a NaN: every consumer divides by this.
    #[test]
    fn a_degenerate_buffer_still_yields_a_usable_size() {
        let g = edit_model::Geometry::default();
        let (w, h) = fit_size(0, 0, 1600, &g);
        assert!(w >= 1.0 && h >= 1.0, "{w}x{h}");
        assert!(w.is_finite() && h.is_finite(), "{w}x{h}");
    }
}
