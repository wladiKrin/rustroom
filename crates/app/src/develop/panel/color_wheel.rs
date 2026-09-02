//! The colour-grading range selector and its hue/saturation wheel.
//!
//! ⚠ egui has no colour wheel, so this one is painted: a triangle fan with per-vertex colours,
//! centre neutral and rim at constant Oklab chroma. Out-of-gamut hues clip at the rim, which
//! honestly shows which hues are reachable rather than hiding it behind a gamut-mapped rim.

use crate::develop::panel::Edited;
use edit_model::{ColorGrading, GradeWheel};
use eframe::egui;

/// Segments around the rim. At the panel's 188 px the arc between two is under 7 px, well
/// below the point where the fan reads as a polygon.
const SEGMENTS: usize = 96;
/// Oklab lightness the wheel is drawn at. Mid-light, so both ends of the hue circle stay
/// legible against the dark panel.
const RIM_L: f32 = 0.72;
/// Oklab chroma at the rim. Chosen for a legible wheel, **not** tied to the stage's `SAT_MAX`:
/// the widget's job is to map angle to hue and radius to saturation, and how far a saturation
/// of 100 actually pushes a pixel is the stage's business, not the picker's.
const RIM_CHROMA: f32 = 0.13;

/// Which grading range the wheel is showing.
///
/// ⚠ View state, never part of `Edits` — it must not reach a sidecar, a paste or a preset, for
/// the same reason `HslChannel` and `Session::selected_mask` do not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GradeRange {
    #[default]
    Shadows,
    Midtones,
    Highlights,
    Global,
}

impl GradeRange {
    pub const ALL: [GradeRange; 4] = [
        Self::Shadows,
        Self::Midtones,
        Self::Highlights,
        Self::Global,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Shadows => "Shad",
            Self::Midtones => "Mid",
            Self::Highlights => "High",
            Self::Global => "Global",
        }
    }

    /// The wheel this range edits.
    pub fn wheel_mut(self, g: &mut ColorGrading) -> &mut GradeWheel {
        match self {
            Self::Shadows => &mut g.shadows,
            Self::Midtones => &mut g.midtones,
            Self::Highlights => &mut g.highlights,
            Self::Global => &mut g.global,
        }
    }
}

/// Lightness samples across the range strip. The weight curves are smooth, so 256 across a
/// ~190 pt strip is well past the point where the sampling shows.
const STRIP_SAMPLES: usize = 256;
/// Height of the range strip, in points.
const STRIP_HEIGHT: f32 = 10.0;

/// Fixed tints for shadows / midtones / highlights, in that order.
///
/// ⚠ **Not the wheels' current hues, and not the theme accent.** The strip shows *where the
/// ranges are*; painting it with the grade would conflate the two questions it exists to keep
/// apart — and the accent is user-switchable, including a `Neutral` setting whose whole point
/// is to have no colour. Low chroma so the strip reads as a legend rather than as a picture,
/// and deliberately off the skin-tone/temperature axis for the reason `theme`'s accent list
/// excludes amber.
const RANGE_TINTS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(0x46, 0x5c, 0x7a),
    egui::Color32::from_rgb(0x6e, 0x6e, 0x6e),
    egui::Color32::from_rgb(0x78, 0x62, 0x80),
];

/// The strip's colour at lightness `l`: the three tints mixed by the stage's own weights.
///
/// ⚠⚠ **The weights come from `ColorGradingStage::range_weights`, never from a copy.** They
/// sum to exactly 1, so this is a convex mix and the result stays inside the three tints —
/// which is what makes an overlap read as a blend rather than as a fourth colour.
fn strip_color(l: f32, blending: f32, balance: f32) -> egui::Color32 {
    let w = pipeline::ColorGradingStage::range_weights(l, blending, balance);
    let mix = |c: fn(&egui::Color32) -> u8| {
        (0..3)
            .map(|k| w[k] * c(&RANGE_TINTS[k]) as f32)
            .sum::<f32>()
            .round()
            .clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgb(mix(|c| c.r()), mix(|c| c.g()), mix(|c| c.b()))
}

/// The strip's pixels, left (black) to right (white).
fn strip_image(blending: f32, balance: f32) -> egui::ColorImage {
    let pixels: Vec<egui::Color32> = (0..STRIP_SAMPLES)
        .map(|i| strip_color(i as f32 / (STRIP_SAMPLES - 1) as f32, blending, balance))
        .collect();
    egui::ColorImage {
        size: [STRIP_SAMPLES, 1],
        pixels,
        source_size: egui::vec2(STRIP_SAMPLES as f32, 1.0),
    }
}

/// A strip showing where the three tonal ranges sit and how far they overlap — the feedback
/// Blending and Balance had none of, so both were judged only by their effect on the photo.
///
/// ⚠ One textured quad, never a mesh: the brush tint's lesson, where a per-sample mesh across
/// a soft gradient built a 711 MB vertex buffer. 256 samples stretched by the GPU costs the
/// same at any width.
pub fn range_strip(ui: &mut egui::Ui, blending: f32, balance: f32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STRIP_HEIGHT),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    // Re-uploaded only when a slider actually moved — the panel body runs sixty times a
    // second, and the values are the whole of what the image depends on.
    let key = egui::Id::new("grade_range_strip");
    let stamp = (blending.to_bits(), balance.to_bits());
    let cached: Option<((u32, u32), egui::TextureHandle)> = ui.data(|d| d.get_temp(key));
    let tex = match cached {
        Some((s, tex)) if s == stamp => tex,
        _ => {
            let tex = ui.ctx().load_texture(
                "grade_range_strip",
                strip_image(blending, balance),
                egui::TextureOptions::LINEAR,
            );
            ui.data_mut(|d| d.insert_temp(key, (stamp, tex.clone())));
            tex
        }
    };
    ui.painter().image(
        tex.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}

/// An sRGB colour for the rim at hue `deg`.
fn rim_color(deg: f32) -> egui::Color32 {
    let h = deg.to_radians();
    let lin =
        color::oklab::oklab_to_linear_srgb([RIM_L, RIM_CHROMA * h.cos(), RIM_CHROMA * h.sin()]);
    let enc = |c: f32| (color::linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8;
    egui::Color32::from_rgb(enc(lin[0]), enc(lin[1]), enc(lin[2]))
}

/// Screen offset for a point at Oklab hue `deg` and radius fraction `r`.
///
/// ⚠ Screen `y` runs **down** while the hue angle runs counterclockwise from the +a axis, so
/// the `y` term is negated. Getting this wrong mirrors the wheel and every hue reads reflected.
fn offset(deg: f32, r: f32, radius: f32) -> egui::Vec2 {
    let h = deg.to_radians();
    egui::vec2(r * radius * h.cos(), -r * radius * h.sin())
}

/// The range strip, then the selected range's wheel. Dragging the wheel sets hue from the
/// angle and saturation from the radius.
///
/// ⚠ Takes the whole `ColorGrading` rather than one wheel, because the strip is drawn from
/// `blending`/`balance` — which belong to the grade, not to any one range. The strip is drawn
/// here rather than beside the call so it cannot be forgotten by a second caller.
pub fn show(
    ui: &mut egui::Ui,
    grade: &mut ColorGrading,
    range: GradeRange,
    diameter: f32,
    ed: &mut Edited,
) {
    // Read before the wheel is borrowed mutably out of the same struct.
    let (blending, balance) = (grade.blending, grade.balance);
    range_strip(ui, blending, balance);
    let wheel = range.wheel_mut(grade);

    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(diameter, diameter),
        egui::Sense::click_and_drag(),
    );
    let centre = rect.center();
    let radius = diameter * 0.5;

    if ui.is_rect_visible(rect) {
        let mut mesh = egui::Mesh::default();
        // Vertex 0 is the hub; the rim follows. A fan rather than a texture: no atlas upload,
        // and it stays crisp at any size.
        mesh.colored_vertex(centre, egui::Color32::from_gray(120));
        for i in 0..=SEGMENTS {
            let deg = i as f32 / SEGMENTS as f32 * 360.0;
            mesh.colored_vertex(centre + offset(deg, 1.0, radius), rim_color(deg));
        }
        for i in 0..SEGMENTS {
            mesh.add_triangle(0, 1 + i as u32, 2 + i as u32);
        }
        ui.painter().add(egui::Shape::mesh(mesh));

        // The handle.
        let p = centre
            + offset(
                wheel.hue,
                (wheel.saturation / 100.0).clamp(0.0, 1.0),
                radius,
            );
        ui.painter()
            .circle_filled(p, 5.0, egui::Color32::from_gray(250));
        ui.painter()
            .circle_filled(p, 3.0, egui::Color32::from_gray(30));
    }

    // ⚠ `dragged_by(Primary)`, never bare `dragged()`. egui reports a drag for **every**
    // button — `interaction.rs` ignores which one when setting `potential_drag_id` — so a bare
    // check would let the middle-drag pan binding rewrite the grade as it passes over.
    let acted = resp.dragged_by(egui::PointerButton::Primary)
        || resp.clicked_by(egui::PointerButton::Primary);
    if acted {
        if let Some(pos) = resp.interact_pointer_pos() {
            let d = pos - centre;
            wheel.saturation = ((d.length() / radius).min(1.0) * 100.0).clamp(0.0, 100.0);
            // Only re-read the hue while off-centre: at the exact centre `atan2(0, 0)` is 0,
            // which would snap the hue to red every time saturation is dragged to zero.
            if d.length() > 1.0 {
                wheel.hue = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0);
            }
            ed.changed = true;
        }
    }
    ed.settled |= resp.drag_stopped_by(egui::PointerButton::Primary)
        || resp.clicked_by(egui::PointerButton::Primary);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one drag from the wheel's centre out to `offset_from_centre`, and report the wheel.
    ///
    /// ⚠ Three passes: lay out with the pointer in place, press, then move-and-hold. egui
    /// resolves interaction against the widgets registered on the *previous* pass, so a
    /// single-pass harness reads zeros and passes vacuously — the bug that made
    /// `a_left_drag_does_not_pan` inert once already.
    fn drag_to(offset_from_centre: egui::Vec2, button: egui::PointerButton) -> GradeWheel {
        let ctx = egui::Context::default();
        let mut grade = ColorGrading::default();
        let mut ed = Edited::default();
        let mut centre = egui::Pos2::ZERO;
        const D: f32 = 200.0;

        for pass in 0..3 {
            let target = centre + offset_from_centre;
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 600.0),
                )),
                ..Default::default()
            };
            match pass {
                1 => {
                    input.events.push(egui::Event::PointerMoved(centre));
                    input.events.push(egui::Event::PointerButton {
                        pos: centre,
                        button,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                2 => input.events.push(egui::Event::PointerMoved(target)),
                _ => {}
            }
            let _ = ctx.run_ui(input, |ui| {
                let before = ui.cursor().min;
                let gap = ui.spacing().item_spacing.y;
                show(ui, &mut grade, GradeRange::Shadows, D, &mut ed);
                // ⚠ The strip is allocated before the wheel, so the wheel's centre is a
                // strip's height and one gap further down. The existing drag tests are the
                // check on this arithmetic: get it wrong and every drag misses the wheel.
                centre = before + egui::vec2(D * 0.5, STRIP_HEIGHT + gap + D * 0.5);
            });
        }
        grade.shadows
    }

    #[test]
    fn each_range_edits_its_own_wheel() {
        // Every value distinct, and each read back from the *other three* as well, so a
        // swapped match arm cannot pass by coincidence.
        let mut g = ColorGrading::default();
        GradeRange::Shadows.wheel_mut(&mut g).hue = 11.0;
        GradeRange::Midtones.wheel_mut(&mut g).hue = 22.0;
        GradeRange::Highlights.wheel_mut(&mut g).hue = 33.0;
        GradeRange::Global.wheel_mut(&mut g).hue = 44.0;
        assert_eq!(g.shadows.hue, 11.0, "Shadows wrote the wrong wheel");
        assert_eq!(g.midtones.hue, 22.0, "Midtones wrote the wrong wheel");
        assert_eq!(g.highlights.hue, 33.0, "Highlights wrote the wrong wheel");
        assert_eq!(g.global.hue, 44.0, "Global wrote the wrong wheel");
    }

    #[test]
    fn all_lists_every_variant_with_a_distinct_label() {
        for r in GradeRange::ALL {
            // No wildcard: a fifth variant breaks this and forces a decision.
            match r {
                GradeRange::Shadows
                | GradeRange::Midtones
                | GradeRange::Highlights
                | GradeRange::Global => {}
            }
        }
        let mut seen: Vec<&str> = GradeRange::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(seen.len(), 4);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 4, "two ranges share a label");
    }

    #[test]
    fn the_angle_convention_matches_oklab() {
        // Right is hue 0, and **up** is hue 90 — screen y runs down, so the widget negates it.
        // A mirrored wheel is the single easiest error to ship here and the hardest to see.
        let r = 10.0;
        let right = offset(0.0, 1.0, r);
        let up = offset(90.0, 1.0, r);
        assert!(
            right.x > 9.9 && right.y.abs() < 0.1,
            "hue 0 is not right: {right:?}"
        );
        assert!(up.y < -9.9 && up.x.abs() < 0.1, "hue 90 is not up: {up:?}");
    }

    #[test]
    fn dragging_right_sets_hue_zero_and_dragging_up_sets_ninety() {
        let right = drag_to(egui::vec2(60.0, 0.0), egui::PointerButton::Primary);
        assert!(
            right.hue < 1.0 || right.hue > 359.0,
            "drag right gave hue {}",
            right.hue
        );
        let up = drag_to(egui::vec2(0.0, -60.0), egui::PointerButton::Primary);
        assert!((up.hue - 90.0).abs() < 1.0, "drag up gave hue {}", up.hue);
    }

    #[test]
    fn the_radius_sets_the_saturation_and_clamps_at_the_rim() {
        // 60 px on a 200 px wheel is 60% of the 100 px radius.
        let half = drag_to(egui::vec2(60.0, 0.0), egui::PointerButton::Primary);
        assert!(
            (half.saturation - 60.0).abs() < 2.0,
            "saturation {}",
            half.saturation
        );
        // Well beyond the rim: still exactly 100, never more.
        let past = drag_to(egui::vec2(400.0, 0.0), egui::PointerButton::Primary);
        assert_eq!(past.saturation, 100.0);
    }

    #[test]
    fn the_handle_returns_to_where_you_dragged() {
        // ⚠ The drag reads an angle with `(-d.y).atan2(d.x)` while the paint writes one with
        // `offset` — two independent computations that must agree. Negate one and not the
        // other and the handle renders mirrored from the point you dragged to, with every
        // *value* still correct. Neither the angle test nor the drag test can see that on its
        // own; only the round trip can. Same shape as the `render_max_dim` and
        // `fit_image_size` disagreements this project has already paid for twice.
        for d in [
            egui::vec2(60.0, 0.0),
            egui::vec2(0.0, -60.0),
            egui::vec2(-40.0, 35.0),
            egui::vec2(30.0, 45.0),
        ] {
            let w = drag_to(d, egui::PointerButton::Primary);
            let back = offset(w.hue, (w.saturation / 100.0).clamp(0.0, 1.0), 100.0);
            assert!(
                (back - d).length() < 1.0,
                "dragged to {d:?} but the handle paints at {back:?}"
            );
        }
    }

    /// Draw one frame into `ctx` and return the strip's pixels **if it was uploaded** — so a
    /// caller can tell "re-uploaded with new pixels" from "served from the cache".
    fn strip_upload(
        ctx: &egui::Context,
        blending: f32,
        balance: f32,
    ) -> Option<Vec<egui::Color32>> {
        let mut grade = ColorGrading {
            blending,
            balance,
            ..ColorGrading::default()
        };
        let mut ed = Edited::default();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| show(ui, &mut grade, GradeRange::Shadows, 200.0, &mut ed),
        );
        out.textures_delta.set.iter().find_map(|(_, delta)| {
            let egui::ImageData::Color(img) = &delta.image;
            (img.size == [STRIP_SAMPLES, 1]).then(|| img.pixels.clone())
        })
    }

    /// The strip's pixels as actually uploaded, read back out of the frame's texture delta —
    /// so the assertion is about what was drawn, not about what `strip_image` returns.
    fn drawn_strip(blending: f32, balance: f32) -> Vec<egui::Color32> {
        // A fresh context per call: the strip caches its texture on the values, so a shared
        // context would serve the first call's pixels to the second and the comparison would
        // be vacuous.
        strip_upload(&egui::Context::default(), blending, balance)
            .expect("the range strip was never uploaded")
    }

    #[test]
    fn the_strip_is_re_uploaded_when_a_value_changes_and_cached_when_it_does_not() {
        // ⚠⚠ **Every other test here uses a fresh context, so none of them can see the cache
        // key at all** — stubbing it to a constant left them all green. That is a real bug,
        // not a harmless one: the strip would freeze at whatever Blending it was first drawn
        // with, and nothing on screen would say so. This is the only test that holds ONE
        // context across two frames, which is the only way the key is reachable.
        let ctx = egui::Context::default();
        let first = strip_upload(&ctx, 0.0, 0.0).expect("the strip was never uploaded");

        // Same values: served from the cache, so nothing is uploaded. Without this half a key
        // that never matched would pass the other half while re-uploading every frame.
        assert!(
            strip_upload(&ctx, 0.0, 0.0).is_none(),
            "the strip re-uploaded although nothing changed"
        );

        let moved = strip_upload(&ctx, 100.0, 0.0)
            .expect("the strip was not re-uploaded after Blending changed");
        assert_ne!(first, moved, "the re-upload carried the old pixels");

        let moved_again = strip_upload(&ctx, 100.0, 60.0)
            .expect("the strip was not re-uploaded after Balance changed");
        assert_ne!(moved, moved_again, "the re-upload carried the old pixels");
    }

    #[test]
    fn the_strip_is_painted_with_the_stages_own_range_weights() {
        // ⚠⚠ The guard the whole feature rests on. A strip that re-derived the Gaussians —
        // or hardcoded them — would come to disagree with the grade it claims to describe,
        // which is the failure it exists to prevent. Compared against `range_weights` mixed
        // independently here, at settings where the three weights are all distinct.
        for &(blending, balance) in &[(0.0f32, 0.0f32), (50.0, 0.0), (50.0, 70.0), (100.0, -50.0)] {
            let drawn = drawn_strip(blending, balance);
            assert_eq!(drawn.len(), STRIP_SAMPLES);
            for (i, got) in drawn.iter().enumerate() {
                let l = i as f32 / (STRIP_SAMPLES - 1) as f32;
                let w = pipeline::ColorGradingStage::range_weights(l, blending, balance);
                let want = |c: fn(&egui::Color32) -> u8| {
                    (0..3)
                        .map(|k| w[k] * c(&RANGE_TINTS[k]) as f32)
                        .sum::<f32>()
                        .round() as u8
                };
                assert_eq!(
                    (got.r(), got.g(), got.b()),
                    (want(|c| c.r()), want(|c| c.g()), want(|c| c.b())),
                    "sample {i} (l={l}) at blending={blending} balance={balance}"
                );
            }
        }
    }

    #[test]
    fn at_blending_zero_the_ends_of_the_strip_are_the_pure_range_tints() {
        // The readable consequence of a near-hard selection: black is all shadows, white all
        // highlights, mid-grey all midtones. If this fails the strip is drawn over the wrong
        // lightness axis, or the tints are in the wrong order.
        let drawn = drawn_strip(0.0, 0.0);
        assert_eq!(
            drawn[0], RANGE_TINTS[0],
            "the black end is not the shadow tint"
        );
        assert_eq!(
            drawn[STRIP_SAMPLES - 1],
            RANGE_TINTS[2],
            "the white end is not the highlight tint"
        );
        assert_eq!(
            drawn[STRIP_SAMPLES / 2],
            RANGE_TINTS[1],
            "the middle is not the midtone tint"
        );
    }

    #[test]
    fn the_strip_changes_when_blending_or_balance_moves() {
        // Without this the strip could be a fixed picture and every other assertion here
        // would still hold — it is the only test that says the controls reach it.
        let base = drawn_strip(50.0, 0.0);
        assert_ne!(
            base,
            drawn_strip(100.0, 0.0),
            "Blending did not move the strip"
        );
        assert_ne!(
            base,
            drawn_strip(50.0, 80.0),
            "Balance did not move the strip"
        );
    }

    #[test]
    fn a_non_primary_drag_leaves_the_wheel_alone() {
        // ⚠ egui reports `dragged()` for **every** button, so without the `dragged_by`
        // gating the middle-drag pan binding would rewrite the grade whenever it crossed
        // the wheel. Exactly the trap the canvas-navigation work recorded.
        let w = drag_to(egui::vec2(60.0, 0.0), egui::PointerButton::Middle);
        assert_eq!(w.saturation, 0.0, "a middle-drag moved the wheel");
        assert_eq!(w.hue, 0.0);
    }
}
