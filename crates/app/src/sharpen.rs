//! Rendering the fit view at the resolution the display can actually show — but only once the
//! user has stopped interacting, so sliders stay fast.
//!
//! Pure and `egui`-free, like `view_region` and `full_res`: this is resolution arithmetic, and
//! a wrong answer here shows up as a soft image or a sluggish drag rather than as a failure.

use std::time::Duration;

/// How long after the last interaction the sharper render runs.
///
/// ⚠ Whatever schedules this must also `request_repaint_after`: egui does not redraw an idle
/// window, so the timer would otherwise expire with nothing to notice it — the same lesson the
/// full-resolution decode's dwell taught.
pub const SETTLE: Duration = Duration::from_millis(200);

/// Quantisation of [`sharp_cap`], in texels.
///
/// ⚠ Without it, dragging a window edge mints a new `max_dim` every frame and thrashes the
/// prefix cache — the same reasoning as the zoom ladder's doubling rungs.
const SHARP_STEP: usize = 256;

/// The fit resolution the canvas can actually display, in texels.
///
/// ⚠ It depends only on the viewport and the DPI — **never on a rendered size**. That is
/// load-bearing rather than incidental: deriving a render cap from a rendered size is exactly
/// the feedback loop that shrank a cropped photo to a few pixels, so this function is built so
/// it cannot participate in one.
///
/// Deliberately generous: the image can never be drawn larger than the viewport, so the
/// viewport's long edge bounds the image's on-screen long edge. For a portrait image in a wide
/// window this over-renders. Using the image's own aspect would be tighter, but the displayed
/// aspect is *post-geometry* — and reaching for a post-geometry quantity here is precisely the
/// mistake above.
pub fn sharp_cap(
    avail_long_points: f32,
    pixels_per_point: f32,
    fit_cap: usize,
    buffer_long: usize,
) -> usize {
    let physical = avail_long_points.max(0.0) * pixels_per_point.max(0.0);
    if !physical.is_finite() {
        return fit_cap;
    }
    let steps = (physical / SHARP_STEP as f32).ceil().max(0.0) as usize;
    let wanted = steps.saturating_mul(SHARP_STEP);
    wanted.clamp(fit_cap, buffer_long.max(fit_cap))
}

/// Whether the fit view should currently be rendered at the sharper resolution.
///
/// ⚠ **A state, not an event.** It deliberately does *not* consider the resolution already
/// showing. An earlier version required `current_max_dim != sharp`, which made the answer flip
/// to `false` the instant the sharp frame landed — so the canvas reverted to the fast rung,
/// counted that as interaction, reset the settle timer, and oscillated forever. The visible
/// symptom was no sharpening at all. Not re-requesting what is already on screen is the
/// caller's `max_dim != render_rung` check, and it was already doing that job.
///
/// ⚠ `showing_whole_image`, not "at the base rung". The earlier version tested the latter, which
/// was only ever true because `fit` was clamped at 1.0; once the clamp went and the ladder began
/// climbing at fit, the condition became unreachable on any display larger than the fit texture
/// and this never fired. "The whole image is visible" is `zoom <= 1` — non-circular, and the
/// thing actually meant.
///
/// ⚠ `current_rung`, not the fit cap. The ladder may already have climbed past the fit cap on a
/// large window, so the question is whether the display can show more than *the rung already
/// chosen* — otherwise this proposes a resolution below what is on screen.
///
/// ⚠ `buffer_is_full_res` is not a refinement, it is the difference between the feature working
/// and being pure cost. While a RAW is still on its **half-size** buffer, that buffer *skips
/// demosaic entirely* — so rendering at its native size shows binned pixels 1:1 instead of a
/// clean downscale of them. There is no detail to gain (downscaling was in fact averaging the
/// binning artifacts away), and it costs ~4x the pixels. Measured in use: no visible change, a
/// noticeably slower Exposure slider.
pub fn wants_sharpen(
    showing_whole_image: bool,
    buffer_is_full_res: bool,
    sharp: usize,
    current_rung: usize,
    idle: Duration,
) -> bool {
    showing_whole_image && buffer_is_full_res && sharp > current_rung && idle >= SETTLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharp_cap_follows_the_display() {
        // A Retina laptop: ~1400 points of canvas at 2x is 2800 physical pixels, rounded up to
        // the next 256. Today's 1600-texel fit view is upscaled 1.75x across that.
        assert_eq!(sharp_cap(1400.0, 2.0, 1600, 6246), 2816);
        // ⚠ A non-HiDPI display gets nothing, correctly: 1400 physical pixels is *less* than
        // the fit cap, so the clamp collapses it to the fast rung and nothing changes.
        assert_eq!(sharp_cap(1400.0, 1.0, 1600, 6246), 1600);
    }

    #[test]
    fn sharp_cap_is_quantised() {
        // Dragging a window edge must not mint a new `max_dim` every frame.
        let a = sharp_cap(1400.0, 2.0, 1600, 6246);
        for w in [1290.0_f32, 1330.0, 1380.0, 1400.0] {
            assert_eq!(sharp_cap(w, 2.0, 1600, 6246), a, "width {w} left the step");
        }
        // ...but a big enough change does move it.
        assert_ne!(sharp_cap(1800.0, 2.0, 1600, 6246), a);
    }

    #[test]
    fn sharp_cap_never_exceeds_the_buffer() {
        // A 5K display against a half-size buffer: there is no more data to render.
        assert_eq!(sharp_cap(2800.0, 2.0, 1600, 3123), 3123);
        // And never drops below the fast rung.
        assert_eq!(sharp_cap(10.0, 2.0, 1600, 6246), 1600);
        // Degenerate inputs give a usable answer rather than a panic or a zero.
        assert_eq!(sharp_cap(0.0, 0.0, 1600, 6246), 1600);
        assert_eq!(sharp_cap(f32::NAN, 2.0, 1600, 6246), 1600);
    }

    #[test]
    fn a_sharpen_is_only_wanted_when_the_whole_image_shows() {
        let long = SETTLE + Duration::from_secs(1);
        assert!(wants_sharpen(true, true, 2816, 1600, long));
        // Never while zoomed into a subset: the ladder already renders more pixels there.
        assert!(!wants_sharpen(false, true, 2816, 1600, long));
        // ⚠ Never on a half-size RAW buffer: its pixels are binned, so more texels are not
        // more detail — only more cost. This is the case that shipped broken.
        assert!(!wants_sharpen(true, false, 2816, 1600, long));
        // Never when there is nothing sharper to render (a non-HiDPI display).
        assert!(!wants_sharpen(true, true, 1600, 1600, long));
    }

    #[test]
    fn a_sharpen_is_measured_against_the_rung_already_chosen() {
        // ⚠ The gate that made this unreachable. On a large window the ladder has already
        // climbed past the fit cap, so comparing against the cap would propose a resolution
        // *below* what is on screen. Real numbers from a 2302x1233-point canvas at 2x.
        let long = SETTLE + Duration::from_secs(1);
        assert!(
            wants_sharpen(true, true, 4608, 3200, long),
            "4608 beats the 3200 rung, so there is still detail to gain"
        );
        assert!(
            !wants_sharpen(true, true, 4608, 6246, long),
            "the rung already exceeds what the display can show"
        );
    }

    #[test]
    fn a_sharpen_waits_for_the_settle() {
        assert!(!wants_sharpen(true, true, 2816, 1600, Duration::ZERO));
        assert!(!wants_sharpen(
            true,
            true,
            2816,
            1600,
            SETTLE - Duration::from_millis(1)
        ));
        assert!(wants_sharpen(true, true, 2816, 1600, SETTLE));
    }

    #[test]
    fn the_wanted_resolution_stabilises_once_sharp() {
        // ⚠ Models the canvas loop, and pins the bug that shipped: gating on "the resolution
        // showing is not yet sharp" made this flip false the moment the sharp frame landed, so
        // the canvas reverted to the fast rung, counted that as interaction, reset the timer,
        // and oscillated — visible as no sharpening whatsoever.
        let (fit, sharp) = (1600usize, 2816usize);

        // The shipped rule, run over successive frames.
        let mut idle = SETTLE;
        for frame in 0..8 {
            let rung = if wants_sharpen(true, true, sharp, fit, idle) {
                sharp
            } else {
                fit
            };
            assert_eq!(rung, sharp, "reverted to the fast rung on frame {frame}");
            // A sharpen pass must not count as interaction, so idle keeps growing.
            idle += Duration::from_millis(16);
        }

        // The buggy rule, for contrast: it additionally required the showing resolution to
        // differ from `sharp`, which is false from the second frame onwards.
        let buggy =
            |current: usize, idle: Duration| sharp > fit && current != sharp && idle >= SETTLE;
        let mut rung = fit;
        let mut flips = 0;
        for _ in 0..8 {
            let next = if buggy(rung, SETTLE) { sharp } else { fit };
            if next != rung {
                flips += 1;
            }
            rung = next;
        }
        assert!(
            flips > 1,
            "the old rule should oscillate, got {flips} flips"
        );
        assert_eq!(
            rung, fit,
            "and lands back on the fast rung, showing nothing"
        );
    }
}
