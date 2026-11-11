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

/// The camera look a render applies: the photo's own — unless the user chose a learned look in the Look menu,
/// which REPLACES it (a Provia shot given Classic Chrome gets Classic Chrome, never both). ⚠ The preview's
/// engine and the export both ask here, so the two cannot disagree.
pub fn applied<'a>(
    edits: &edit_model::Edits,
    own: Option<&'a Arc<pipeline::lut::Cube>>,
) -> Option<&'a Arc<pipeline::lut::Cube>> {
    let chose_a_film = edits.look.is_active()
        && edits
            .look
            .file
            .as_deref()
            .is_some_and(crate::looks::is_learned);
    own.filter(|_| !chose_a_film)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A learned film chosen in the Look menu replaces the photo's own; the user's own look does not.
    #[test]
    fn a_chosen_film_replaces_the_photos_own_and_a_creative_look_does_not() {
        let own =
            Arc::new(pipeline::lut::Cube::parse(&fit::fit(&[]).to_cube("Provia (X-T4)")).unwrap());
        let with = |file: &str, strength: f32| edit_model::Edits {
            look: edit_model::Look {
                file: Some(file.into()),
                strength,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            applied(&edit_model::Edits::default(), Some(&own)).is_some(),
            "no look: the photo's own"
        );
        assert!(
            applied(&with("portra.cube", 100.0), Some(&own)).is_some(),
            "a creative look stacks"
        );
        assert!(applied(
            &with("camera/Classic Chrome _X-T4_.cube", 100.0),
            Some(&own)
        )
        .is_none());
        // At strength 0 the chosen film does nothing, so the photo keeps its own.
        assert!(applied(&with("camera/Classic Chrome _X-T4_.cube", 0.0), Some(&own)).is_some());
    }
}
