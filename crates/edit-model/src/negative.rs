//! A scanned film negative (M10.11a): what the negative stage needs to turn it into a positive.
//!
//! ⚠⚠ **The film base is MEASURED, not chosen.** A colour negative's orange mask differs by film
//! stock, by scan light and by scan exposure, so `base` is sampled from the unexposed film with an
//! eyedropper, and `low`/`high` are measured from the frame — the reason this is a stage with stored
//! measurements and not a preset of fixed numbers.
//!
//! ⚠ **Densities, not levels**: `low` and `high` are `log10(base / pixel)` at the frame's shadows and
//! highlights, per channel, so changing `gamma` afterwards keeps black and white where they were.

use serde::{Deserialize, Serialize};

/// A scanned film negative, inverted by the negative stage.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Negative {
    /// Invert. ⚠ Has no effect until a base is picked — see [`Negative::is_active`].
    pub on: bool,
    /// The film base's colour, linear, in the decode's working space: the unexposed film.
    pub base: [f32; 3],
    /// The film's contrast: density per decade of exposure. 0.6 is a colour negative's typical
    /// value; lower inverts to a contrastier positive.
    pub gamma: f32,
    /// Per channel, the density above the base that becomes black — the frame's shadows.
    pub low: [f32; 3],
    /// Per channel, the density above the base that becomes (almost) white — the frame's highlights.
    pub high: [f32; 3],
    /// A black and white negative: one neutral channel.
    pub mono: bool,
}

/// ⚠ The values are finite by construction — measured, or from a ranged slider, and JSON has no NaN
/// — so equality is total, and `Edits` can stay `Eq`.
impl Eq for Negative {}

impl Default for Negative {
    /// ⚠⚠ **Hand-written, not derived** — `#[serde(default)]` fills a missing field from this, and a
    /// derived one would give a contrast of 0 and highlights at density 0. `Watermark::default`'s trap.
    fn default() -> Self {
        Self {
            on: false,
            base: [0.0; 3],
            gamma: 0.6,
            low: [0.0; 3],
            high: [2.0; 3],
            mono: false,
        }
    }
}

impl Negative {
    /// The contrast the panel offers: ±100 maps onto this, 0 onto 0.6.
    pub const GAMMA_RANGE: (f32, f32) = (0.3, 1.2);

    /// Exactly the default — what `Edits` leaves out of its serialization.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the stage does anything: on, a base picked in every channel, a contrast, and a
    /// measured range in every channel. ⚠ A pick on something darker than the film base can leave
    /// `high ≤ low`; inactive is then the honest answer, never a division by nothing.
    pub fn is_active(&self) -> bool {
        self.on
            && self.gamma > 0.0
            && self.base.iter().all(|&b| b > 0.0)
            && (0..3).all(|c| self.high[c] > self.low[c])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picked() -> Negative {
        Negative {
            on: true,
            base: [0.8, 0.5, 0.3],
            ..Negative::default()
        }
    }

    #[test]
    fn the_default_is_inert_and_its_other_fields_are_not_zero() {
        // ⚠ Both halves: a derived `Default` is inert too, and would give every file written
        // before this field a contrast of 0 and highlights at density 0.
        let d = Negative::default();
        assert!(!d.on && !d.is_active());
        assert_eq!(d.gamma, 0.6);
        assert_eq!(d.high, [2.0; 3]);
        assert_eq!(d.low, [0.0; 3]);
    }

    #[test]
    fn it_is_active_only_when_on_picked_and_measured() {
        assert!(picked().is_active());
        assert!(
            !Negative {
                on: false,
                ..picked()
            }
            .is_active(),
            "off"
        );
        assert!(
            !Negative {
                base: [0.8, 0.0, 0.3],
                ..picked()
            }
            .is_active(),
            "a channel unpicked"
        );
        assert!(
            !Negative {
                low: [0.0, 0.5, 0.0],
                high: [2.0, 0.5, 2.0],
                ..picked()
            }
            .is_active(),
            "no range in a channel"
        );
        assert!(
            !Negative {
                gamma: 0.0,
                ..picked()
            }
            .is_active(),
            "no contrast"
        );
    }

    /// ⚠ A default negative is left out of the file, so no sidecar or preview key changes for a
    /// photo that is not a negative; a set one is written and read back.
    #[test]
    fn a_default_negative_is_not_written() {
        let plain = serde_json::to_string(&crate::Edits::default()).unwrap();
        assert!(!plain.contains("negative"), "{plain}");
        let set = crate::Edits {
            negative: picked(),
            ..crate::Edits::default()
        };
        let text = serde_json::to_string(&set).unwrap();
        let back: crate::Edits = serde_json::from_str(&text).unwrap();
        assert_eq!(back.negative, picked());
    }

    #[test]
    fn an_edits_file_without_a_negative_reads_it_off() {
        let e: crate::Edits = serde_json::from_str("{}").expect("an empty edits file");
        assert_eq!(e.negative, Negative::default());
        let n: Negative = serde_json::from_str(r#"{"on":true}"#).expect("a partial one");
        assert_eq!((n.on, n.gamma), (true, 0.6));
    }
}
