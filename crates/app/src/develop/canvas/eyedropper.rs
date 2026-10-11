//! The two eyedroppers: white balance and defringe, each a click on the photo.

use crate::Session;
use eframe::egui;
use rustroom_app::tool::Tool;

/// Set the white balance from a pixel the user clicked.
///
/// ⚠⚠ **Reads the DECODE BUFFER, not the rendered texture.** The texture already has the white
/// balance applied, so solving from it would make the eyedropper chase its own correction: pick a
/// grey, the picture shifts, pick the same grey again and it shifts further. The same trap the
/// luminance range mask keys on `base` to avoid.
///
/// ⚠ The click is mapped through `FrameMap`, which un-does orientation, straighten and crop — so
/// it works on a cropped, rotated photo. It does **not** un-do lens correction or perspective,
/// which is the caveat the brush already carries: a few pixels of displacement, and this averages
/// a patch anyway.
pub(super) fn pick_white_balance(
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    tool: Tool,
) -> bool {
    if !resp.clicked() {
        return false;
    }
    let Some(p) = resp.interact_pointer_pos() else {
        return false;
    };
    let map = pipeline::brush::FrameMap::new(
        &s.edits.geometry,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
    );
    let [u, v] = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
    let (bw, bh) = (s.buffer.width(), s.buffer.height());
    if bw == 0 || bh == 0 || !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return false;
    }
    // ⚠ A patch, not one pixel. A single sample is sensor noise, and the whole point of clicking
    // a grey card is to average over it — every eyedropper in every editor does this.
    let cx = ((u * bw as f32) as usize).min(bw - 1);
    let cy = ((v * bh as f32) as usize).min(bh - 1);
    let r = 2usize;
    let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
    for y in cy.saturating_sub(r)..=(cy + r).min(bh - 1) {
        for x in cx.saturating_sub(r)..=(cx + r).min(bw - 1) {
            let px = s.buffer.pixel(x, y);
            for (a, v) in acc.iter_mut().zip(px) {
                *a += v;
            }
            n += 1.0;
        }
    }
    if n == 0.0 {
        return false;
    }
    let mean = [acc[0] / n, acc[1] / n, acc[2] / n];
    // ⚠ M10.11a: through the negative, so a scanned negative is balanced on its positive.
    let mean = rustroom_app::negative_tool::view(mean, &s.edits);
    s.edits.white_balance = rustroom_app::wb_tool::solve_pick(mean, s.wb_baseline);
    // ⚠ Disarmed after one pick: an eyedropper that stayed armed would turn every subsequent
    // click — including one meant to select a mask or to dismiss something — into a white-balance
    // change. Lightroom's behaves the same way.
    s.picking_wb = false;
    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    true
}
/// The film-base eyedropper's click (M10.11a): the clicked film becomes the base, the frame is
/// measured, the negative turns on. Returns whether the photo changed.
///
/// ⚠ The decode buffer, `pick_white_balance`'s rule — the rendered texture is already the positive,
/// so a second pick on it would chase the first. Disarmed after one click, whatever it found.
pub(super) fn pick_negative(
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    tool: Tool,
) -> Option<(String, bool)> {
    if !resp.clicked() {
        return None;
    }
    let p = resp.interact_pointer_pos()?;
    let map = pipeline::brush::FrameMap::new(
        &s.edits.geometry,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
    );
    let at = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
    s.picking_negative = false;
    let buffer = std::sync::Arc::clone(&s.buffer);
    if !rustroom_app::negative_tool::pick(&mut s.edits, &buffer, at) {
        return Some((
            "No film there to pick — click the clear film between frames".to_string(),
            false,
        ));
    }
    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    Some(("Film base picked and the frame balanced".to_string(), true))
}

/// The defringe eyedropper's click: the most saturated pixel of a 5×5 patch of the SOURCE buffer
/// under the pointer — the as-shot colour the defringe stage selects on, before any tail stage has
/// moved its hue — through `defringe_tool::pick`. Returns the status line and whether the photo
/// changed; `None` when there was no click on the photo.
///
/// ⚠ The source buffer, `pick_white_balance`'s reasoning: the rendered texture has every tone and
/// colour stage applied, and would read a hue the defringe step never sees.
pub(super) fn pick_fringe(
    s: &mut Session,
    resp: &egui::Response,
    frame_rect: egui::Rect,
    tool: Tool,
) -> Option<(String, bool)> {
    if !resp.clicked() {
        return None;
    }
    let p = resp.interact_pointer_pos()?;
    let map = pipeline::brush::FrameMap::new(
        &s.edits.geometry,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
    );
    let [u, v] = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
    let (bw, bh) = (s.buffer.width(), s.buffer.height());
    if bw == 0 || bh == 0 || !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    let cx = ((u * bw as f32) as usize).min(bw - 1);
    let cy = ((v * bh as f32) as usize).min(bh - 1);
    let r = rustroom_app::color_readout::PATCH_R;
    let patch = (cy.saturating_sub(r)..=(cy + r).min(bh - 1))
        .flat_map(|y| (cx.saturating_sub(r)..=(cx + r).min(bw - 1)).map(move |x| (x, y)));
    let buffer = std::sync::Arc::clone(&s.buffer);
    // ⚠ M10.11a: the defringe step runs on the positive, so its hue is picked on the positive.
    let edits = s.edits.clone();
    let hue = rustroom_app::defringe_tool::most_chromatic_hue(
        patch.map(|(x, y)| rustroom_app::negative_tool::view(buffer.pixel(x, y), &edits)),
    );
    // ⚠ Disarmed after one pick, whatever it found — the white-balance eyedropper's rule.
    s.picking_fringe = false;
    let Some(h) = hue else {
        return Some((
            "No colour there to pick — click on the fringe itself".to_string(),
            false,
        ));
    };
    match rustroom_app::defringe_tool::pick(s.edits.defringe, h) {
        Some((d, f)) => {
            s.edits.defringe = d;
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
            Some((
                format!("Defringe: {} range set around {h:.0}°", f.label()),
                true,
            ))
        }
        None => Some((
            format!("{h:.0}° is not a fringe colour — purple or green only"),
            false,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ The wiring test, and it is the half this project has repeatedly shipped broken: the
    /// solve has its own unit tests, but nothing else can see that a click on the canvas ever
    /// reaches it. Arming the eyedropper and clicking must change `Edits`.
    const BASE: [f32; 3] = [0.85, 0.55, 0.30];

    /// A scanned frame: a band of unexposed film down the middle, film densities 0.2..1.6 either side.
    fn film_frame(w: usize, h: usize) -> core_image::ImageBuf {
        let mut img = core_image::ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let px = if (w * 2 / 5..w * 3 / 5).contains(&x) {
                    BASE
                } else {
                    let d = 0.2 + 1.4 * (x + y) as f32 / (w + h) as f32;
                    BASE.map(|b| b * 10f32.powf(-d))
                };
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    /// M10.11a: the armed film-base picker takes the clicked film as the base, measures the frame,
    /// turns the negative on, disarms, and reports an edit.
    #[test]
    fn an_armed_film_base_picker_picks_and_balances_from_a_click() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            let (w, h) = (s.buffer.width(), s.buffer.height());
            s.buffer = std::sync::Arc::new(film_frame(w, h));
            s.picking_negative = true;
        }
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        let n = s.edits.negative;
        assert!(n.on && n.is_active(), "{n:?}");
        for (got, want) in n.base.iter().zip(BASE) {
            assert!((got - want).abs() < 0.01, "{:?}", n.base);
        }
        assert!(!s.picking_negative, "the picker stayed armed after a pick");
    }

    /// M10.11a: on an inverted photo the white-balance eyedropper solves from the positive the
    /// photo shows — the patch through `negative_tool::view`.
    #[test]
    fn the_white_balance_eyedropper_reads_the_positive() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        let want = {
            let s = app.session.as_mut().expect("a session");
            let (w, h) = (s.buffer.width(), s.buffer.height());
            let mut img = core_image::ImageBuf::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    img.set_pixel(x, y, BASE.map(|b| b * 10f32.powf(-0.9)));
                }
            }
            s.buffer = std::sync::Arc::new(img);
            s.edits.negative = edit_model::Negative {
                on: true,
                base: BASE,
                low: [0.1, 0.3, 0.5],
                high: [1.6, 1.8, 2.0],
                ..edit_model::Negative::default()
            };
            s.picking_wb = true;
            let px = BASE.map(|b| b * 10f32.powf(-0.9));
            let positive = rustroom_app::negative_tool::view(px, &s.edits);
            let raw = rustroom_app::wb_tool::solve_pick(px, s.wb_baseline);
            let pos = rustroom_app::wb_tool::solve_pick(positive, s.wb_baseline);
            assert_ne!(raw, pos, "the two must differ for this to mean anything");
            pos
        };
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        // ⚠ Within rounding — the eyedropper averages a 5 × 5 patch, this solved one pixel — and the
        // negative's own answer is thousands of kelvin away, so the tolerance still discriminates.
        let got = app.session.as_ref().unwrap().edits.white_balance;
        let (gk, wk) = (got.kelvin.unwrap(), want.kelvin.unwrap());
        assert!(
            (gk - wk).abs() < 0.5 && (got.tint - want.tint).abs() < 0.01,
            "{got:?} against {want:?}"
        );
    }

    #[test]
    fn an_armed_eyedropper_sets_the_white_balance_from_a_click() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            s.picking_wb = true;
            assert_eq!(s.edits.white_balance, edit_model::WhiteBalance::default());
        }
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        assert!(
            s.edits.white_balance.kelvin.is_some(),
            "the click set no temperature"
        );
        // ⚠ And it disarms, or every later click — one meant to select a mask, or to dismiss
        // something — would silently re-set the white balance.
        assert!(!s.picking_wb, "the eyedropper stayed armed after a pick");
    }

    /// ⚠⚠ The defringe eyedropper's wiring test (M9.6), `an_armed_eyedropper_sets_the_white_balance
    /// _from_a_click`'s twin: the pick rule has unit tests, and nothing else can see that a click
    /// reaches it. A purple source sets the purple range and amount and disarms; a grey one changes
    /// nothing, says so, and disarms too.
    #[test]
    fn an_armed_fringe_picker_sets_the_purple_range_from_a_click() {
        let run = |colour: [f32; 3]| {
            let ctx = egui::Context::default();
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
            let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
            {
                let s = app.session.as_mut().expect("a session");
                let (w, h) = (s.buffer.width(), s.buffer.height());
                let mut img = core_image::ImageBuf::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        img.set_pixel(x, y, colour);
                    }
                }
                s.buffer = std::sync::Arc::new(img);
                s.picking_fringe = true;
            }
            crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
            let s = app.session.as_ref().expect("a session");
            assert!(!s.picking_fringe, "the picker stayed armed after a click");
            (s.edits.defringe, app.status.clone())
        };
        let (d, status) = run(color::srgb_to_prophoto([0.35, 0.12, 0.55]));
        assert!(
            d.purple_amount > 0.0,
            "the click set no purple amount: {d:?}"
        );
        assert_ne!(d.purple_hue, edit_model::Defringe::DEFAULT_PURPLE_HUE);
        assert_eq!(d.green_amount, 0.0);
        assert!(status.contains("purple"), "{status}");

        let (d, status) = run([0.3; 3]);
        assert_eq!(d, edit_model::Defringe::default());
        assert!(status.contains("No colour"), "{status}");
    }

    /// ⚠ The guard on the guard: "a click changed the edits" means nothing unless a click with
    /// the eyedropper **off** leaves them alone. Without this, a `pick_white_balance` that ran
    /// unconditionally would pass the test above.
    #[test]
    fn a_click_without_the_eyedropper_leaves_the_white_balance_alone() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let centre = crate::develop::canvas::test_support::canvas_centre(&mut app, &ctx);
        crate::develop::canvas::test_support::click_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        assert_eq!(s.edits.white_balance, edit_model::WhiteBalance::default());
    }
}
