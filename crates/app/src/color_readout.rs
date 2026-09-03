//! The numeric colour readout — what the status cluster shows for the pixel under the cursor.
//!
//! ⚠ Lives in the lib because this is a unit-testable rule: the `crop_tool` / `mask_tool` /
//! `panel_order` pattern. The hover that produces the position is in the canvas bin module.
//!
//! ⚠⚠ **These bytes are sRGB, and that is load-bearing.** They come from `pack`'s `srgb8`, the
//! same quantization the histogram is bucketed from — **not** from `RenderedFrame.rgba`, which is
//! in the *display's* primaries. `pack`'s own comment states the rule: "the histogram describes
//! the image; the texture describes what this monitor must be driven with."

/// How far either side of the cursor the patch reaches, in rendered pixels.
///
/// ⚠ 2, i.e. a 5×5 patch — the white-balance eyedropper's own radius and its recorded reasoning:
/// "a single sample is sensor noise, and the whole point of clicking a grey card is to average
/// over it." It also stops the numbers jittering as the pointer moves, which is what makes a
/// readout usable at a native rung.
///
/// ⚠ Stated so it is not later found as a bug: **a mean reports the neighbourhood, not the
/// extreme.** A single blown pixel can hide inside a 5×5 average; the histogram remains the
/// instrument for "is *anything* blown".
pub const PATCH_R: usize = 2;

/// The mean sRGB value of the patch centred on `(x, y)` in an RGBA8 buffer of `w` × `h`.
///
/// `None` when the position is outside the image or the buffer does not match its dimensions.
///
/// ⚠ The dimension check is not defensive noise: the bytes and the dimensions travel together
/// from `RenderedFrame`, and a mismatch would index past the end of the buffer.
pub fn mean_at(rgba: &[u8], w: usize, h: usize, x: usize, y: usize) -> Option<[u8; 3]> {
    if w == 0 || h == 0 || rgba.len() != w * h * 4 || x >= w || y >= h {
        return None;
    }
    let (mut acc, mut n) = ([0u32; 3], 0u32);
    let y0 = y.saturating_sub(PATCH_R);
    let y1 = (y + PATCH_R).min(h - 1);
    let x0 = x.saturating_sub(PATCH_R);
    let x1 = (x + PATCH_R).min(w - 1);
    for py in y0..=y1 {
        for px in x0..=x1 {
            let i = (py * w + px) * 4;
            acc[0] += rgba[i] as u32;
            acc[1] += rgba[i + 1] as u32;
            acc[2] += rgba[i + 2] as u32;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    // ⚠ Divided by what was actually read, never by the full patch area: the window clamps at
    // the borders, so at a corner only 9 of 25 samples exist. `spatial::box_blur`'s convention —
    // "borders clamp the window, not the samples".
    Some([(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8])
}

/// What the status cluster shows for a sampled value, beside its colour swatch.
///
/// ⚠ **Unlabelled, and the swatch is what replaces the letters.** `R`/`G`/`B` prefixes cost three
/// tokens of width in the one row this project has already had to fix for running out of space,
/// and they tell you nothing the conventional channel order does not — where the swatch tells you
/// *which colour* at a glance, which no amount of labelling does.
///
/// ⚠⚠ **Every channel is padded to three characters, and the caller MUST draw it monospaced.**
/// Reported in use as the readout "jumping around and seeming wobbly": the cluster is laid out
/// right-to-left, so anything left of the numbers — the swatch — moves whenever their width does,
/// and 8 → 128 → 255 changes that width twice as the pointer crosses a gradient. ⚠ Padding alone
/// is **not** enough in a proportional font, where a space is narrower than a digit; the two
/// halves of the fix are here and at the call site, which is why this says so rather than leaving
/// it to be rediscovered.
///
/// ⚠ sRGB 0–255, which is exactly what the bytes already are — zero conversion, nothing to get
/// subtly wrong. It also answers the clipping question precisely: 255 means blown and 254 does
/// not, a distinction Lightroom's 0–100 % cannot make.
pub fn label(rgb: [u8; 3]) -> String {
    format!("{:>3} {:>3} {:>3}", rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An RGBA8 buffer of one flat colour.
    fn flat(w: usize, h: usize, c: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            v.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        v
    }

    #[test]
    fn a_flat_patch_reads_its_own_value() {
        let b = flat(32, 32, [10, 128, 250]);
        assert_eq!(mean_at(&b, 32, 32, 16, 16), Some([10, 128, 250]));
    }

    /// ⚠⚠ **This is what discriminates a mean from a single-pixel read**, and a flat-field test
    /// cannot: the two agree exactly on a uniform image. The `spot_source` lesson — "each placed
    /// its fixture where a wrong implementation had nothing to reach for."
    ///
    /// A vertical split down the middle: centred at x = 16 the patch spans columns 14..=18, so
    /// two dark and three light — mean 60, which neither a single-pixel read (100) nor a
    /// mis-centred window can produce.
    #[test]
    fn a_patch_straddling_two_values_reads_the_mean() {
        let (w, h) = (32, 32);
        let mut b = flat(w, h, [0, 0, 0]);
        for y in 0..h {
            for x in 16..w {
                let i = (y * w + x) * 4;
                b[i] = 100;
                b[i + 1] = 100;
                b[i + 2] = 100;
            }
        }
        let m = mean_at(&b, w, h, 16, 16).expect("in bounds");
        assert_eq!(m, [60, 60, 60], "got {m:?}");
    }

    /// ⚠⚠ **The edge clamp divides by the pixels that exist, not by 25.** At a corner only a 3×3
    /// is in bounds; a version dividing by the full patch area gives a plausible, *darker* number
    /// that no flat-field test can see.
    #[test]
    fn the_patch_clamps_at_the_edges_and_divides_by_what_it_read() {
        let b = flat(32, 32, [200, 200, 200]);
        // A corner: 9 of 25 samples are in bounds, all 200. The mean must still be 200.
        assert_eq!(mean_at(&b, 32, 32, 0, 0), Some([200, 200, 200]));
    }

    #[test]
    fn a_position_outside_the_image_reads_nothing() {
        let b = flat(8, 8, [1, 2, 3]);
        assert_eq!(mean_at(&b, 8, 8, 8, 4), None);
        assert_eq!(mean_at(&b, 8, 8, 4, 8), None);
    }

    /// A buffer that does not match its stated dimensions must not index out of bounds.
    #[test]
    fn a_mismatched_buffer_reads_nothing() {
        assert_eq!(mean_at(&[0, 0, 0, 255], 8, 8, 0, 0), None);
        assert_eq!(mean_at(&[], 0, 0, 0, 0), None);
    }

    /// ⚠ The label is not a rescale. 255 must read as 255 — "255 means blown and 254 does not" is
    /// the distinction that makes 0–255 the right units and Lightroom's 0–100 % the wrong ones.
    #[test]
    fn the_label_shows_the_bytes_not_a_percentage() {
        let s = label([255, 0, 128]);
        assert!(s.contains("255"), "{s}");
        assert!(s.contains("128"), "{s}");
        assert!(!s.contains('%'), "the label rescaled to a percentage: {s}");
        // ⚠ No channel letters: the swatch beside it carries that job. Asserted so the letters
        // cannot creep back in and silently widen the status cluster.
        assert!(
            !s.contains('R'),
            "the label grew channel letters again: {s}"
        );
    }

    /// ⚠ The channels stay in R G B order and are distinguishable in the string, or the readout
    /// cannot answer "which channel is blown".
    /// ⚠⚠ **Every value renders to the same length**, or the swatch to its left shifts as the
    /// pointer moves and the whole readout reads as wobbly — reported in use. The cluster is laid
    /// out right-to-left, so anything left of the numbers moves whenever their width does, and
    /// 8 → 128 → 255 changes that width twice as the pointer crosses a gradient.
    ///
    /// ⚠ This is only **half** the fix. Padding alone does nothing in a proportional font, where
    /// a space is narrower than a digit; `shell.rs` must draw the label **monospaced**, and no
    /// test here can see whether it does.
    #[test]
    fn every_value_renders_to_the_same_width() {
        let widths: std::collections::BTreeSet<usize> = [
            [0, 0, 0],
            [9, 9, 9],
            [10, 99, 100],
            [255, 255, 255],
            [1, 128, 40],
        ]
        .into_iter()
        .map(|c| label(c).chars().count())
        .collect();
        assert_eq!(
            widths.len(),
            1,
            "labels render to differing lengths, so the readout will jump: {widths:?}"
        );
    }

    #[test]
    fn the_label_keeps_the_channels_in_order() {
        let s = label([11, 22, 33]);
        let r = s.find("11").unwrap_or_else(|| panic!("no red: {s}"));
        let g = s.find("22").unwrap_or_else(|| panic!("no green: {s}"));
        let b = s.find("33").unwrap_or_else(|| panic!("no blue: {s}"));
        assert!(r < g && g < b, "channels out of order in {s}");
    }
}
