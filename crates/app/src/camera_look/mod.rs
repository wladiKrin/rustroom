//! Camera-matching colour (M10.13d): the look of each film simulation, learned from the camera's own JPEGs in
//! the user's RAFs. No egui.
//!
//! ⚠⚠ **Nothing is shipped**: every table is fitted on this machine from this user's photos — the legal route
//! to camera colour (`docs/superpowers/specs/2026-11-11-m10.13d-camera-matching-design.md`).

pub mod fit;
pub mod fuji;
pub mod learn;
pub mod store;
