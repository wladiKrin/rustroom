//! Lens Blur (M10.16d): a synthetic shallow depth of field — each pixel blurred by its distance from the
//! focal plane on the estimated depth map, in three layers so nothing smears across the subject's
//! outline, with discs in the aperture's shape. The spec: `docs/superpowers/specs/2026-10-14-m10.16d-
//! lens-blur-design.md`.

pub mod aperture;
