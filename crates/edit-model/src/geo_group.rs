//! The geometry, partitioned into the three sections the Crop panel shows it under.
//!
//! ⚠ The Crop & Geometry panel draws Crop, Straighten and Perspective as sections, each with a
//! modified dot and a reset for exactly that section. Both go through **one** function that
//! copies a section's fields from one `Edits` to another, so the two can never disagree about
//! what a section contains — the `LocalGroup` pattern.
//!
//! ⚠⚠ **`copy_geo_group` destructures `Geometry` exhaustively, with no `..`.** A field added to
//! the geometry then fails to compile until it is assigned to a section.

use crate::{Edits, Geometry};

/// The three sections of the Crop & Geometry panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoGroup {
    /// The crop rectangle and the orientation (rotation, flips).
    Crop,
    /// The straighten angle.
    Straighten,
    /// The perspective correction.
    Perspective,
}

impl GeoGroup {
    pub const ALL: [GeoGroup; 3] = [GeoGroup::Crop, GeoGroup::Straighten, GeoGroup::Perspective];

    /// The header's name.
    pub fn name(self) -> &'static str {
        match self {
            GeoGroup::Crop => "Crop",
            GeoGroup::Straighten => "Straighten",
            GeoGroup::Perspective => "Perspective",
        }
    }
}

impl Edits {
    /// Whether `group` differs from its default.
    pub fn geo_group_differs(&self, group: GeoGroup) -> bool {
        let mut reset = self.clone();
        reset.reset_geo_group(group);
        reset != *self
    }

    /// Put `group` back to its default, leaving the other two — and everything else — alone.
    pub fn reset_geo_group(&mut self, group: GeoGroup) {
        self.copy_geo_group(&Edits::default(), group);
    }

    fn copy_geo_group(&mut self, from: &Edits, group: GeoGroup) {
        let Geometry {
            orientation,
            angle,
            crop,
        } = &mut self.geometry;
        match group {
            GeoGroup::Crop => {
                *crop = from.geometry.crop;
                *orientation = from.geometry.orientation;
            }
            GeoGroup::Straighten => {
                *angle = from.geometry.angle;
            }
            GeoGroup::Perspective => {
                self.perspective = from.perspective;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(group: GeoGroup) -> Edits {
        let mut e = Edits::default();
        match group {
            GeoGroup::Crop => e.geometry.crop.left = 0.25,
            GeoGroup::Straighten => e.geometry.angle = 3.0,
            GeoGroup::Perspective => e.perspective.vertical = 40.0,
        }
        e
    }

    #[test]
    fn one_moved_field_differs_in_exactly_its_group() {
        for g in GeoGroup::ALL {
            let e = moved(g);
            for h in GeoGroup::ALL {
                assert_eq!(
                    e.geo_group_differs(h),
                    g == h,
                    "{} moved; asked about {}",
                    g.name(),
                    h.name()
                );
            }
        }
    }

    #[test]
    fn reset_group_clears_exactly_its_group_and_nothing_outside_the_geometry() {
        for g in GeoGroup::ALL {
            let mut e = Edits::default();
            e.geometry.crop.left = 0.25;
            e.geometry.orientation.rotate_cw();
            e.geometry.angle = 3.0;
            e.perspective.vertical = 40.0;
            e.exposure = 1.0;
            e.reset_geo_group(g);
            for h in GeoGroup::ALL {
                assert_eq!(
                    e.geo_group_differs(h),
                    g != h,
                    "reset {}: {}",
                    g.name(),
                    h.name()
                );
            }
            assert_eq!(e.exposure, 1.0, "a geometry reset touched an adjustment");
        }
    }

    #[test]
    fn resetting_every_group_reaches_the_default_geometry() {
        // ⚠ Proves the three groups cover `Geometry` and `Perspective`: a field in no group
        // would survive all three resets.
        let mut e = Edits::default();
        e.geometry.crop.left = 0.25;
        e.geometry.orientation.rotate_cw();
        e.geometry.angle = 3.0;
        e.perspective.vertical = 40.0;
        e.perspective.scale = -50.0;
        for g in GeoGroup::ALL {
            e.reset_geo_group(g);
        }
        assert_eq!(e.geometry, Geometry::default());
        assert_eq!(e.perspective, crate::Perspective::default());
    }
}
