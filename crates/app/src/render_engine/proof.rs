//! The soft proof: marking colours the proof space cannot hold.

use super::frame::Proof;
use core_image::ImageBuf;

/// The colour an out-of-gamut pixel is marked with, as **encoded output values** so it lands in
/// the texture exactly as written rather than being converted into the proof space on the way.
///
/// ⚠⚠ **Fixed, never the theme accent.** The accent is user-switchable and includes a `Neutral`
/// setting whose whole point is to have no colour — which would defeat the warning precisely when
/// somebody had chosen it. The crop overlay records the same reasoning for the same red
/// `(255, 60, 60)`, and reusing it keeps one rule rather than adding a second.
const GAMUT_WARNING: [f32; 3] = [1.0, 60.0 / 255.0, 60.0 / 255.0];

/// The texture for a soft proof: the ProPhoto-run frame taken back to the working space, marked
/// where the proof space cannot hold it, and encoded into that space.
///
/// ⚠⚠ **The decode is what makes the warning able to fire at all.** `OutputSpace::encode` clamps
/// to `[0, 1]` in every space, so the ordinary sRGB tail hands `pack` data that is already gamut
/// clipped — against which an exact out-of-gamut test is identically false for every target,
/// because sRGB fits inside all of them. The tail is run into ProPhoto — the documented bit-exact
/// identity, which converts nothing — precisely so this round trip recovers the working-space
/// colour instead.
///
/// ⚠ "Exact" therefore means exact *within ProPhoto*: a colour outside it was clipped by that
/// encode and cannot be recovered here. ProPhoto holds essentially every real colour and the
/// display rendering bounds the data before it, so the residue is negligible — stated so it is not
/// later found as a bug.
pub(super) fn proof_texture(out: &ImageBuf, proof: Proof) -> Vec<u8> {
    out.map_pixels(move |c| {
        let working = c.map(|v| color::OutputSpace::ProPhoto.decode(v));
        if proof.warn && color::is_out_of_gamut(working, proof.space) {
            return GAMUT_WARNING;
        }
        let conv = proof
            .space
            .from_working(color::GamutMap::for_space(proof.space).map(proof.intent, working));
        [
            proof.space.encode(conv[0]),
            proof.space.encode(conv[1]),
            proof.space.encode(conv[2]),
        ]
    })
    .to_rgba8()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefix_cache::PrefixCache;
    use crate::render_engine::brush_cache::BrushCache;
    use crate::render_engine::frame::{FrameSpec, RenderedFrame};
    use crate::render_engine::render::{render_cached, render_once};
    use edit_model::Edits;
    use std::sync::Arc;

    // ---- soft proofing -------------------------------------------------------------------
    /// A neutral grey: in gamut in every space, so it must never be marked.
    const IN_GAMUT: [f32; 3] = [0.5, 0.5, 0.5];

    /// Display P3's green primary, pulled a tenth of the way toward mid grey **in P3
    /// coordinates**, expressed in the working space.
    ///
    /// ⚠⚠ **This is the discriminating fixture, and it is off the boundary on purpose.** P3's
    /// primary itself lands at a P3 coordinate of exactly 1.0, where a rounding change either way
    /// decides whether it is "out of gamut" — a fixture that flips with the last f32 bit proves
    /// nothing. Blended, it sits at P3 ≈ (0.05, 0.95, 0.05): comfortably inside P3, and still far
    /// outside sRGB, where its red component is around −0.4. `the_fixture_is_in_one_gamut_and_out
    /// _of_another` asserts that premise rather than trusting this comment.
    const P3_GREEN: [f32; 3] = [0.242_51, 0.847_31, 0.095_72];

    /// ProPhoto's own green: outside sRGB *and* outside P3.
    const WIDE_GREEN: [f32; 3] = [0.0, 1.0, 0.0];

    /// Three working-space colours the proof must actually move, one pixel each.
    fn gamut_fixture() -> Arc<ImageBuf> {
        let mut buf = ImageBuf::new(3, 1);
        buf.set_pixel(0, 0, IN_GAMUT);
        buf.set_pixel(1, 0, P3_GREEN);
        buf.set_pixel(2, 0, WIDE_GREEN);
        Arc::new(buf)
    }

    /// Render `buf` through the cached path at its own resolution, with the given proof.
    fn proofed(buf: &Arc<ImageBuf>, soft_proof: Option<Proof>) -> RenderedFrame {
        proofed_with(buf, soft_proof, None)
    }

    fn proofed_with(
        buf: &Arc<ImageBuf>,
        soft_proof: Option<Proof>,
        visualize: Option<f32>,
    ) -> RenderedFrame {
        intended(
            buf,
            soft_proof,
            visualize,
            color::RenderingIntent::default(),
        )
    }

    fn intended(
        buf: &Arc<ImageBuf>,
        soft_proof: Option<Proof>,
        visualize: Option<f32>,
        intent: color::RenderingIntent,
    ) -> RenderedFrame {
        render_cached(
            buf,
            &Edits::default(),
            FrameSpec {
                // The fixture's own long edge, so nothing is downscaled and each pixel stays one
                // pixel — the texture is read back by index below.
                max_dim: 3,
                region: None,
                display_space: color::OutputSpace::Srgb,
                intent,
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                visualize,
                soft_proof,
                grade_view: None,
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::TailRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        )
        .0
    }

    /// The RGBA of pixel `i` of a 3×1 frame.
    fn px(f: &RenderedFrame, i: usize) -> [u8; 4] {
        [
            f.rgba[i * 4],
            f.rgba[i * 4 + 1],
            f.rgba[i * 4 + 2],
            f.rgba[i * 4 + 3],
        ]
    }

    fn proof(space: color::OutputSpace, warn: bool) -> Option<Proof> {
        Some(Proof {
            space,
            intent: color::RenderingIntent::default(),
            warn,
        })
    }

    /// ⚠⚠ The canvas intent must reach the pixels, on the plain canvas AND through the proof — a
    /// request field nobody reads compiles perfectly. Grey is asserted unmoved so a render that
    /// changed everything cannot pass; the P3 green is asserted to move.
    ///
    /// ⚠ Not `WIDE_GREEN`: that is ProPhoto's own primary, on the working space's edge, and
    /// perceptual maps that edge exactly onto the target's — where relative puts it too. Measured
    /// identical under both intents, correctly. The discriminating colour is saturated but inside.
    #[test]
    fn the_canvas_intent_reaches_the_canvas_and_the_proof() {
        let buf = gamut_fixture();
        let (rel, per) = (
            color::RenderingIntent::RelativeColorimetric,
            color::RenderingIntent::Perceptual,
        );
        let p3 = |intent| {
            Some(Proof {
                space: color::OutputSpace::DisplayP3,
                intent,
                warn: false,
            })
        };
        for (what, a, b) in [
            (
                "canvas",
                intended(&buf, None, None, rel),
                intended(&buf, None, None, per),
            ),
            (
                "proof",
                intended(&buf, p3(rel), None, rel),
                intended(&buf, p3(per), None, per),
            ),
        ] {
            assert_eq!(
                px(&a, 0),
                px(&b, 0),
                "{what}: grey must not depend on the intent"
            );
            assert_ne!(px(&a, 1), px(&b, 1), "{what}: the P3 green must");
        }
    }

    #[test]
    fn the_fixture_is_in_one_gamut_and_out_of_another() {
        // ⚠⚠ The fixture's own premise, asserted before anything is built on it. A colour flagged
        // in *every* space cannot tell a working proof from one that ignores the space argument
        // entirely — which is the shape of bug the whole feature is exposed to. If this fails, the
        // tests below are measuring something other than what they name.
        assert!(
            color::is_out_of_gamut(P3_GREEN, color::OutputSpace::Srgb),
            "fixture check: the green must be outside sRGB"
        );
        assert!(
            !color::is_out_of_gamut(P3_GREEN, color::OutputSpace::DisplayP3),
            "fixture check: the green must be inside Display P3"
        );
        assert!(!color::is_out_of_gamut(IN_GAMUT, color::OutputSpace::Srgb));
        assert!(color::is_out_of_gamut(WIDE_GREEN, color::OutputSpace::Srgb));
    }

    /// ⚠⚠ **The histogram describes the PHOTOGRAPH, never the view mode.** It is derived from
    /// `pack`'s sRGB quantisation, so a proof applied before that point silently makes it
    /// display-referred — the bug the display-profile work shipped and caught by re-reading what
    /// consumes the bytes.
    ///
    /// ⚠⚠ It holds **byte for byte and by construction**, not by luck: while proofing the tail
    /// runs twice, and the histogram comes from an unmodified sRGB run. A single ProPhoto run was
    /// tried first and measured to move 12 of 12 real frames' histograms, worst five levels.
    #[test]
    fn the_histogram_is_unaffected_by_the_soft_proof() {
        let buf = gamut_fixture();
        let plain = proofed(&buf, None);
        let mut moved = false;
        for space in color::OutputSpace::DISPLAYS {
            for warn in [false, true] {
                let p = proofed(&buf, proof(space, warn));
                assert_eq!(
                    plain.hist, p.hist,
                    "the proof reached the histogram ({space:?}, warn={warn})"
                );
                // ⚠ The sRGB instrument channel too. A colour readout built on the *texture*
                // would report the proof's numbers, which look entirely plausible and are wrong.
                assert_eq!(
                    *plain.srgb, *p.srgb,
                    "the proof reached the readout's bytes ({space:?}, warn={warn})"
                );
                moved |= *plain.rgba != *p.rgba;
            }
        }
        assert!(
            moved,
            "fixture check: no proof changed the texture, so this test proves nothing"
        );
    }

    /// ⚠ With the proof off, `pack` is bit-exactly what it was — so nothing about the ordinary
    /// path moved. `render_once` is the reference precisely because it has no proof concept at
    /// all: it always passes `None`, so it cannot drift with this feature.
    #[test]
    fn the_proof_off_path_is_unchanged() {
        let buf = gamut_fixture();
        let cold = render_once(
            &buf,
            &Edits::default(),
            3,
            1.0,
            color::OutputSpace::Srgb,
            pipeline::Resampler::default(),
            None,
        );
        let off = proofed(&buf, None);
        assert_eq!(
            *cold.rgba, *off.rgba,
            "the texture moved with the proof off"
        );
        assert_eq!(*cold.srgb, *off.srgb, "the sRGB channel moved");
        assert_eq!(cold.hist, off.hist, "the histogram moved");
        // ⚠ And the fixture is one a proof genuinely changes, or "unchanged" would be trivially
        // true of an image nothing could move.
        let on = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_ne!(
            *cold.rgba, *on.rgba,
            "fixture check: the proof changed nothing, so this test proves nothing"
        );
    }

    /// ⚠⚠ **Both halves, and the second is the one that matters.** A warning that flags every
    /// pixel and one that flags none each pass a one-sided assertion.
    #[test]
    fn a_colour_the_proof_space_cannot_hold_is_marked_and_one_it_can_is_not() {
        let buf = gamut_fixture();
        let f = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_ne!(
            px(&f, 0),
            marked(),
            "a neutral grey is in gamut everywhere and must not be marked"
        );
        assert_eq!(px(&f, 1), marked(), "sRGB cannot hold this green");
        assert_eq!(px(&f, 2), marked(), "sRGB cannot hold ProPhoto green");
    }

    /// The warning as it must appear in the texture.
    fn marked() -> [u8; 4] {
        [255, 60, 60, 255]
    }

    /// ⚠ The colour is **fixed**, asserted, so it cannot drift onto the theme accent — which is
    /// user-switchable and includes a `Neutral` setting whose whole point is to have no colour,
    /// defeating the warning exactly when someone had chosen it.
    #[test]
    fn the_warning_colour_is_fixed() {
        let buf = gamut_fixture();
        let f = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_eq!(
            px(&f, 2),
            marked(),
            "the warning must be the crop overlay's fixed red (255, 60, 60)"
        );
    }

    /// ⚠ The warning is a **separate toggle** from the proof: proofing without it is the common
    /// case. Without this, a version that marked unconditionally would pass every other test here.
    #[test]
    fn the_warning_only_fires_when_it_is_switched_on() {
        let buf = gamut_fixture();
        let quiet = proofed(&buf, proof(color::OutputSpace::Srgb, false));
        let loud = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        assert_ne!(
            px(&quiet, 2),
            marked(),
            "the warning fired with the flag off"
        );
        assert_eq!(px(&loud, 2), marked(), "fixture check: nothing was marked");
        // The proof itself still applies with the warning off — only the marking is suppressed.
        //
        // ⚠⚠ Measured against **Display P3**, not sRGB, and the reason is a finding rather than a
        // convenience: an sRGB proof with the warning off came back *bit-identical* to no proof on
        // this fixture, which is correct. The ordinary render already clips into sRGB, so proofing
        // into sRGB re-does what the pipeline just did. **An sRGB proof is only informative with
        // the warning on** — the marking is the whole of what it adds — and a test asserting
        // otherwise would have been asserting something false about correct code.
        let plain = proofed(&buf, None);
        let p3 = proofed(&buf, proof(color::OutputSpace::DisplayP3, false));
        assert_ne!(
            *plain.rgba, *p3.rgba,
            "the proof did nothing with the warning off"
        );
    }

    /// ⚠⚠ **The test that catches a warning ignoring its space argument.** The green is inside
    /// Display P3 and outside sRGB, so proofing the same frame into the two spaces must mark it in
    /// one and not the other. A fixture flagged everywhere cannot see this, and neither can a test
    /// that only ever proofs into one space.
    #[test]
    fn the_proof_space_decides_what_is_marked() {
        let buf = gamut_fixture();
        let srgb = proofed(&buf, proof(color::OutputSpace::Srgb, true));
        let p3 = proofed(&buf, proof(color::OutputSpace::DisplayP3, true));
        assert_eq!(px(&srgb, 1), marked(), "sRGB cannot hold this green");
        assert_ne!(px(&p3, 1), marked(), "Display P3 can hold this green");
        // ⚠ And a colour outside *both* is marked either way, so "P3 marks nothing" is excluded.
        assert_eq!(px(&p3, 2), marked(), "P3 cannot hold ProPhoto green");
    }

    /// ⚠ The proof reaches the texture at all, and reaches it *differently* per space — the
    /// failure this whole task risks is a menu that changes a field nothing reads.
    #[test]
    fn the_proof_reaches_the_texture() {
        let buf = gamut_fixture();
        let plain = proofed(&buf, None);
        let srgb = proofed(&buf, proof(color::OutputSpace::Srgb, false));
        let p3 = proofed(&buf, proof(color::OutputSpace::DisplayP3, false));
        assert_ne!(*plain.rgba, *p3.rgba, "the proof never reached `pack`");
        assert_ne!(
            *srgb.rgba, *p3.rgba,
            "the two proof spaces produced the same texture"
        );
        // ⚠ The exact value, not merely a difference: the P3 green is INSIDE P3, so relative
        // colorimetric leaves it untouched and the texture must be its P3 encoding. An inverted
        // conversion would also differ, and be wrong in exactly the way this feature exists to
        // avoid. ⚠ This used to assert "P3's green below sRGB's", which held only because the old
        // sRGB proof clipped green to 255; gamut-mapped at constant lightness it reads 245 (M6.4c).
        let sp = color::OutputSpace::DisplayP3;
        let want = sp
            .from_working(P3_GREEN)
            .map(|v| (sp.encode(v) * 255.0).round() as i32);
        let got = px(&p3, 1);
        for i in 0..3 {
            assert!(
                (got[i] as i32 - want[i]).abs() <= 1,
                "P3 proof of the P3 green: got {got:?}, want {want:?}"
            );
        }
    }

    /// ⚠ Visualize Spots and the proof are both view modes applied at the same point in `pack`,
    /// and the diagnostic one wins: its edge view is achromatic, so proofing it would mark nothing
    /// and show nothing while still paying for a second tail run. Asserted so the suppression is a
    /// decision rather than something a later reader removes as redundant.
    #[test]
    fn visualize_spots_suppresses_the_proof() {
        let buf = gamut_fixture();
        let vis = proofed_with(&buf, None, Some(12.0));
        let both = proofed_with(&buf, proof(color::OutputSpace::Srgb, true), Some(12.0));
        assert_eq!(
            *vis.rgba, *both.rgba,
            "the proof reached the edge view's texture"
        );
    }
}
