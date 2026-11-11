//! Camera-matching colour (M10.13d): the look of each film simulation, learned from the camera's own JPEGs in
//! the user's RAFs. No egui.
//!
//! ⚠⚠ **Nothing is shipped**: every table is fitted on this machine from this user's photos — the legal route
//! to camera colour (`docs/superpowers/specs/2026-11-11-m10.13d-camera-matching-design.md`).

pub mod fit;
pub mod fuji;
pub mod learn;
pub mod store;

use std::path::Path;
use std::sync::Arc;

/// What Camera matching applies to one photo: the film simulation's label, and its learned table when there is
/// one (`None`: nothing learned for this film yet — the photo renders as Camera).
#[derive(Clone)]
pub struct Resolved {
    pub label: String,
    pub cube: Option<Arc<pipeline::lut::Cube>>,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Resolved({:?}, learned: {})",
            self.label,
            self.cube.is_some()
        )
    }
}

/// The camera look for the photo at `path`, shot on `make` `model` — `None` for a file that names no film
/// simulation (another make, a JPEG). ⚠ The body is named as the catalog names it
/// (`core_image::Capture::camera_name`), so what is learned under a name is found under it.
pub fn resolve(path: &Path, make: &str, model: &str, store: &store::Store) -> Option<Resolved> {
    let look = fuji::read(path)?;
    let body = core_image::Capture::camera_name(make, model)?;
    Some(Resolved {
        label: store::label(&body, &look.film),
        cube: store.cube(&body, &look.film),
    })
}
