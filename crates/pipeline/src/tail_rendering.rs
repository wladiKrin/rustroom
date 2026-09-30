//! How the tail renders: the camera rendering, the profile curve and the creative look.

use crate::lut;

/// What the **tail's rendering** needs that [`Edits`] cannot carry.
///
/// ⚠ A struct rather than more parameters: the builder already sits at seven, and an eighth trips
/// `too_many_arguments` — the same answer `FrameSpec`, `NameField`, `SourceInfo`, `SourceFacts`
/// and `RenderSpec` each gave. `Copy` despite holding references, because a shared reference is
/// `Copy`, which is what lets it sit beside `RenderSpec` rather than inside it.
///
/// ⚠ **`TailRendering`, not `ProfileRendering`** (renamed 2026-09-30, M10.7b): the old name was
/// narrower than the contents — `look` is not a camera-profile field — and it had been left alone on
/// 2026-09-18 so the rename would be its own diff rather than folded into the one that made the
/// struct required. The bindings of the type are `rendering` for the same reason.
///
/// ⚠⚠ **Everything here is loaded by the CALLER**, because `pipeline` does no I/O. A caller that
/// supplies nothing gets a render without it — which is exactly what a missing `.cube` must do.
#[derive(Clone, Copy, Default)]
pub struct TailRendering<'a> {
    /// The profile's `ProfileToneCurve`, when it has a usable one; else the built-in curve.
    pub curve: Option<&'a [[f32; 2]]>,
    /// The profile's `BaselineExposure` in stops; `None` falls back to
    /// [`stages::display_rendering::BASELINE_EV`].
    ///
    /// ⚠ `None` is not `Some(0.0)`: a profile that declares no shift has made a statement, and
    /// one that says nothing has not.
    pub baseline_ev: Option<f32>,
    /// The creative look's parsed table, when the app has resolved `edits.look.file` to a file
    /// and read it.
    ///
    /// ⚠ `&Arc` rather than `Arc`, so this struct stays `Copy` and the builder clones a refcount
    /// rather than up to 786 KB of table on every render.
    ///
    /// ⚠⚠ **`None` means the look does not run AND does not suppress the camera rendering.** A
    /// preset naming a `.cube` that is not on this machine must render *without the look*, not
    /// without a rendering — see `a_look_with_no_table_leaves_the_camera_rendering_alone`.
    pub look: Option<&'a std::sync::Arc<lut::Cube>>,
}

impl TailRendering<'_> {
    /// The primaries and the transfer curve, and nothing else: **no** camera rendering and **no**
    /// creative look.
    ///
    /// ⚠⚠ **A NAME rather than `default()`, and the name is the point.** `TailRendering::default()`
    /// at a call site reads as *"nothing special here"*; this reads as a statement a reviewer can
    /// disagree with. The builders used to apply the default **silently**, and that cost three
    /// defects in one day (2026-09-18): an export lost the profile's curve, its baseline exposure
    /// **and** the user's `.cube`, and a thumbnail lost the look. None of them failed a test,
    /// because an absence looks exactly like a decision.
    ///
    /// This is what the goldens, `rustroom render` and most tests genuinely want — and they should
    /// say so.
    pub fn colorimetric() -> Self {
        Self::default()
    }
}

/// Whether a profile's tone curve is the identity, and therefore no rendering at all.
///
/// ⚠ Checked because the X-T4's `ProfileToneCurve` is exactly `[[0,0],[1,1]]`. Applying it would
/// leave the photograph colorimetric while the setting claimed to be rendering it — the feature
/// silently doing nothing on real hardware.
fn is_identity(points: &[[f32; 2]]) -> bool {
    points.len() < 2 || points.iter().all(|p| (p[0] - p[1]).abs() < 1e-4)
}

/// The profile curve a [`DisplayRenderingStage`] should actually be built with.
///
/// ⚠⚠ **One rule, one place.** The tail applies [`is_identity`] before constructing the stage, and
/// anything that rebuilds that stage — `gpu-pipeline`'s `accelerate` does — must apply exactly the
/// same filter. Two callers each remembering to would be two chances to forget, and the symptom is
/// a GPU render that differs from the CPU one only on cameras whose profile declares an identity
/// curve, which is most of them.
pub fn effective_profile_curve(curve: Option<&[[f32; 2]]>) -> Option<&[[f32; 2]]> {
    curve.filter(|p| !is_identity(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        build_pipeline, build_pipeline_split_in_region_with_brush, RenderScale, RenderSpec,
    };
    use core_image::ImageBuf;
    use edit_model::Edits;

    /// ⚠⚠ `Rendering::None` must insert **no stage at all**, not a curve that happens to be the
    /// identity. Bit-exactness is the rule the entire golden harness rests on.
    #[test]
    fn the_default_rendering_inserts_no_stage() {
        let names = build_pipeline(
            &Edits::default(),
            RenderScale::new(1.0),
            TailRendering::colorimetric(),
        )
        .stage_names();
        assert!(
            !names.contains(&"display_rendering"),
            "a neutral photo got a rendering stage: {names:?}"
        );
    }

    /// ⚠ And it lands immediately before the output transform — after every capture stage, so
    /// their scene-linear tuning survives, and before the transform so the histogram (taken from
    /// `pack`'s quantization of this output) describes the rendered image.
    #[test]
    fn the_rendering_runs_last_before_the_output_transform() {
        let edits = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let names = build_pipeline(&edits, RenderScale::new(1.0), TailRendering::colorimetric())
            .stage_names();
        let r = names
            .iter()
            .position(|n| *n == "display_rendering")
            .expect("no rendering stage");
        let o = names
            .iter()
            .position(|n| *n == "output_transform")
            .expect("no output transform");
        assert_eq!(
            o,
            r + 1,
            "something got between the rendering and the transform: {names:?}"
        );
    }

    /// The tail's stage names for `edits`, with `cube` supplied as the look's table.
    ///
    /// ⚠ The **only** order helper these look tests use. Six tests already name stage order and a
    /// second helper is how two come to disagree, so this one delegates to the same builder every
    /// other order assertion goes through rather than reconstructing the list.
    #[cfg(test)]
    fn look_stage_names(
        edits: &Edits,
        cube: Option<&std::sync::Arc<lut::Cube>>,
    ) -> Vec<&'static str> {
        let (_, tail) = build_pipeline_split_in_region_with_brush(
            edits,
            RenderScale::FULL,
            None,
            None,
            RenderSpec::default(),
            None,
            TailRendering {
                look: cube,
                ..TailRendering::default()
            },
        );
        tail.stage_names()
    }

    fn a_cube() -> std::sync::Arc<lut::Cube> {
        std::sync::Arc::new(
            lut::Cube::parse(
                "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
            )
            .expect("parses"),
        )
    }

    /// ⚠⚠ **Assert on the STAGE LIST, not on pixels.** The two placements are indistinguishable
    /// from output whenever a LUT happens to be gentle, so a pixel test would pass against the
    /// wrong one — and double-rendering a film emulation (our S-curve, then the LUT's, giving
    /// crushed shadows and a clipped roll-off) is precisely the bug this prevents.
    #[test]
    fn a_linear_lut_replaces_the_camera_rendering_and_a_display_lut_follows_it() {
        let cube = a_cube();
        let with = |space| {
            let edits = Edits {
                rendering: Some(edit_model::Rendering::Camera),
                look: edit_model::Look {
                    file: Some("x.cube".into()),
                    space,
                    strength: 100.0,
                },
                ..Default::default()
            };
            look_stage_names(&edits, Some(&cube))
        };

        let display = with(edit_model::LutSpace::SrgbDisplay);
        assert!(
            display
                .windows(2)
                .any(|w| w[0] == "display_rendering" && w[1] == "creative_look"),
            "a display-referred look must FOLLOW the rendering: {display:?}"
        );
        // Rec.709 is display-referred too, and must place identically.
        assert_eq!(with(edit_model::LutSpace::Rec709), display);

        let linear = with(edit_model::LutSpace::Linear);
        assert!(
            !linear.contains(&"display_rendering"),
            "a linear look IS the rendering and must replace it: {linear:?}"
        );
        assert!(linear.contains(&"creative_look"), "{linear:?}");
    }

    /// ⚠ Either way the look runs **before** the output transform: it is a display-referred
    /// decision on working-space data, not something applied to encoded output.
    #[test]
    fn a_look_runs_before_the_output_transform() {
        let cube = a_cube();
        for space in edit_model::LutSpace::ALL {
            let edits = Edits {
                rendering: Some(edit_model::Rendering::Camera),
                look: edit_model::Look {
                    file: Some("x.cube".into()),
                    space,
                    strength: 100.0,
                },
                ..Default::default()
            };
            let names = look_stage_names(&edits, Some(&cube));
            let l = names.iter().position(|n| *n == "creative_look").unwrap();
            let o = names.iter().position(|n| *n == "output_transform").unwrap();
            assert_eq!(o, l + 1, "{space:?}: {names:?}");
        }
    }

    /// ⚠⚠ **A look whose file is missing renders WITHOUT the look — and the camera rendering
    /// stays.** Suppressing the rendering on the strength of a `Linear` declaration alone would
    /// leave a photo with a missing `.cube` rendering colorimetrically flat, which is worse than
    /// the missing look. The suppression is therefore conditioned on the table actually being
    /// there, not on what `Edits` claims.
    #[test]
    fn a_look_with_no_table_leaves_the_camera_rendering_alone() {
        for space in edit_model::LutSpace::ALL {
            let edits = Edits {
                rendering: Some(edit_model::Rendering::Camera),
                look: edit_model::Look {
                    file: Some("gone.cube".into()),
                    space,
                    strength: 100.0,
                },
                ..Default::default()
            };
            let names = look_stage_names(&edits, None);
            assert!(!names.contains(&"creative_look"), "{space:?}: {names:?}");
            assert!(
                names.contains(&"display_rendering"),
                "{space:?} lost the camera rendering to a look that is not there: {names:?}"
            );
        }
    }

    /// ⚠ An inactive look inserts no stage even when a table is supplied — a whole-image pass
    /// that does nothing is not free.
    #[test]
    fn an_inactive_look_inserts_no_stage() {
        let cube = a_cube();
        let no_file = Edits::default();
        assert!(!look_stage_names(&no_file, Some(&cube)).contains(&"creative_look"));

        let zero_strength = Edits {
            look: edit_model::Look {
                file: Some("x.cube".into()),
                strength: 0.0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!look_stage_names(&zero_strength, Some(&cube)).contains(&"creative_look"));
    }

    /// ⚠⚠ An identity profile curve is **ignored** — the X-T4's is exactly that, so honouring it
    /// would leave the setting silently doing nothing on real hardware. Both directions, because
    /// a test of one alone passes against a version that always uses the built-in.
    #[test]
    fn an_identity_profile_curve_is_ignored_and_a_real_one_is_not() {
        assert!(
            is_identity(&[[0.0, 0.0], [1.0, 1.0]]),
            "the X-T4's curve reads as real"
        );
        assert!(is_identity(&[[0.5, 0.5]]), "a single point is not a curve");
        assert!(!is_identity(&[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]));
    }

    /// ⚠⚠ **And the pipeline actually consults it** — a test of `is_identity` alone went
    /// **uncaught** when the `.filter()` was removed, because nothing observed which curve the
    /// builder picked. Rendered output is the only thing that can see it.
    #[test]
    fn the_builder_falls_back_to_the_built_in_for_an_identity_curve() {
        let edits = Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let render = |curve: Option<&[[f32; 2]]>| {
            let (prefix, tail) = build_pipeline_split_in_region_with_brush(
                &edits,
                RenderScale::new(1.0),
                None,
                None,
                RenderSpec::default(),
                None,
                TailRendering {
                    curve,
                    // The transfer is what this test is about; a baseline shift would move the
                    // probe along it and blur what the assertion is comparing.
                    baseline_ev: Some(0.0),
                    ..TailRendering::default()
                },
            );
            let mut img = ImageBuf::new(1, 1);
            img.set_pixel(0, 0, [0.05, 0.05, 0.05]);
            tail.run(prefix.run(img)).pixel(0, 0)[0]
        };

        let identity: [[f32; 2]; 2] = [[0.0, 0.0], [1.0, 1.0]];
        let real: [[f32; 3 - 1]; 3] = [[0.0, 0.0], [0.5, 0.9], [1.0, 1.0]];
        let built_in = render(None);

        assert!(
            (render(Some(&identity)) - built_in).abs() < 1e-6,
            "an identity profile curve was honoured instead of ignored"
        );
        assert!(
            (render(Some(&real)) - built_in).abs() > 0.01,
            "a real profile curve was ignored"
        );
    }
}
