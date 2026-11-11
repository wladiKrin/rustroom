//! What a session starts from: the filter at startup, export width, the preview tier, and the edits a
//! fresh photo opens with.

use super::Settings;
use raw_decode::Demosaic;

impl Settings {
    /// The Library filter a session starts with: the persisted format filter, nothing else.
    ///
    /// ⚠⚠ **This is the read half of `hidden_formats`, and it did not exist.** The toggle wrote
    /// the setting on every change and `RustroomApp::new` built its filter from
    /// `LibraryFilter::default()`, so the file said "JPEGs hidden" and the app hid nothing —
    /// reported as *"the setting does not persist over sessions"*. The serde round-trip test
    /// beside it passed throughout: it could see the file and not the wiring.
    ///
    /// ⚠ The search text and the mark sets deliberately start empty — they are about the question
    /// being asked now, the filter bar's own rule; only the format is a standing preference.
    pub fn initial_filter(&self) -> crate::library_filter::LibraryFilter {
        crate::library_filter::LibraryFilter {
            format: self.hidden_formats.clone(),
            ..Default::default()
        }
    }

    /// The batch-export width, repaired: at least 1, at most the machine's core count.
    ///
    /// ⚠⚠ **Repairs rather than trusts, and the two ends fail differently.** `settings.json` is
    /// hand-editable: a **0** would spawn no workers and hang the export forever, and a **64**
    /// would ask for nineteen gigabytes of decode buffers on a 26 Mpx camera. `panel_order`'s rule
    /// — the safe state is the one you get by default, not one you have to maintain.
    ///
    /// ⚠ The ceiling is the **core count** rather than a constant, because libraw's decode is
    /// single-threaded: beyond one worker per core there is nothing left to overlap, only memory
    /// to spend.
    pub fn export_width(&self) -> usize {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        (self.export_concurrency as usize).clamp(1, cores)
    }

    /// The tier rendered previews are built at: the chosen one, else the 1:1 view's.
    pub fn preview_tier(&self) -> Demosaic {
        self.preview_demosaic.unwrap_or(self.view_demosaic)
    }

    /// The edits a newly-opened photo starts with.
    ///
    /// ⚠⚠ **A photo with a sidecar keeps EXACTLY what it stored; the preferences seed a photo
    /// that has never been edited and nothing else.** Assigning them unconditionally is the bug
    /// this replaced — it overwrote the stored value on every open, so the Camera-rendering
    /// toggle reset to off on any photo that had ever been edited, and a stored lens correction
    /// silently reverted to all-off the same way. The old code's own comment claimed this
    /// behaviour while the line beneath it did the opposite.
    ///
    /// ⚠ Extracted as a pure function so the rule is testable without a window — the decision is
    /// the part worth testing, and the session glue is not.
    pub fn initial_edits(&self, stored: Option<edit_model::Edits>) -> edit_model::Edits {
        match stored {
            Some(e) => e,
            None => edit_model::Edits {
                lens_correction: self.initial_lens_correction(),
                rendering: self.initial_rendering(),
                ..edit_model::Edits::default()
            },
        }
    }

    /// What a **fresh** photo's rendering starts as.
    ///
    /// ⚠ `None` — *undecided* — when the preference is off, **not** `Some(Rendering::None)`:
    /// decided-off is a fact worth a sidecar, and seeding it would write a file for every photo
    /// ever opened under an off preference, where today none is written.
    pub fn initial_rendering(&self) -> Option<edit_model::Rendering> {
        match self.default_rendering {
            edit_model::Rendering::None => None,
            // M10.13d: the camera rendering, matched to the camera when that is switched on.
            edit_model::Rendering::Camera if self.camera_match => {
                Some(edit_model::Rendering::CameraMatch)
            }
            r => Some(r),
        }
    }

    /// What a **fresh** photo's lens corrections start as — `None`, undecided, when the
    /// preference is off, for `initial_rendering`'s reason.
    ///
    /// ⚠ No `has_sidecar` parameter any more: the "only when there is no sidecar" half of the
    /// rule lives in [`Self::initial_edits`], where it applies to every preference at once
    /// rather than being repeated per field and got wrong for the second one.
    pub fn initial_lens_correction(&self) -> Option<edit_model::LensCorrection> {
        self.auto_lens_correction
            .then(|| edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                ..Default::default()
            })
    }

    /// The space the *renderer* should convert into.
    ///
    /// ⚠⚠ Every read of `display_space` in the render path must go through here. When the
    /// operating system is colour-matching the window for us — macOS, once the `CAMetalLayer`
    /// carries a colorspace — converting as well would be a **double conversion**: we would
    /// produce P3 and Core Animation would then treat it as sRGB and convert again, leaving the
    /// preview undersaturated and varying per monitor. Returning `Srgb` here makes that
    /// unrepresentable rather than something to remember.
    ///
    /// ⚠ The stored value is deliberately left alone, so a settings file shared with a Linux
    /// machine keeps a sensible choice for it.
    pub fn effective_display_space(&self, os_managed: bool) -> color::OutputSpace {
        if os_managed {
            color::OutputSpace::Srgb
        } else {
            self.display_space
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ The wiring half, which the round-trip test below cannot see: a persisted format
    /// filter must reach the session's filter, or the file is written and never read — which is
    /// exactly how it shipped, and how it was reported.
    #[test]
    fn the_hidden_formats_reach_the_filter_at_startup() {
        let mut s = Settings::default();
        s.hidden_formats.toggle("jpg");
        let f = s.initial_filter();
        assert!(
            f.format.is_hidden("jpg"),
            "the persisted filter never reached the session"
        );
        assert!(
            f.text.is_empty() && f.stars.is_empty(),
            "only the format is a standing preference"
        );
    }

    /// ⚠⚠ **It repairs rather than trusts**, `panel_order`'s rule and for a sharper reason here:
    /// `settings.json` is hand-editable, so **0** must become 1 rather than spawn no workers and
    /// hang the export forever, and **64** must become the core count rather than ask for nineteen
    /// gigabytes of decode buffers.
    #[test]
    fn the_export_width_repairs_rather_than_trusting_the_file() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let w = |v: u32| {
            Settings {
                export_concurrency: v,
                ..Settings::default()
            }
            .export_width()
        };
        assert_eq!(
            w(0),
            1,
            "zero must repair to one, or the run spawns nothing"
        );
        assert_eq!(w(1), 1);
        assert_eq!(w(64), cores, "past the core count must clamp to it");
        assert_eq!(w(2), 2.min(cores));
    }

    #[test]
    fn the_os_managing_colour_overrides_the_manual_setting() {
        // ⚠⚠ The property that makes double conversion unrepresentable. If both were live we
        // would convert sRGB→P3 ourselves and Core Animation would then treat *that* as sRGB and
        // convert again — visibly undersaturated, and varying per monitor.
        let mut s = Settings::default();
        for space in color::OutputSpace::DISPLAYS {
            s.display_space = space;
            assert_eq!(
                s.effective_display_space(true),
                color::OutputSpace::Srgb,
                "{} should be ignored while the OS is managing colour",
                space.label()
            );
        }
    }

    #[test]
    fn without_the_os_managing_the_manual_setting_is_used() {
        // ⚠ The fallback, and it is not a formality: if tagging the layer ever fails, this is
        // what stops macOS being left *uncorrected* — worse than what piece D shipped.
        let mut s = Settings::default();
        for space in color::OutputSpace::DISPLAYS {
            s.display_space = space;
            assert_eq!(s.effective_display_space(false), space);
        }
    }

    #[test]
    fn the_stored_setting_survives_the_os_taking_over() {
        // ⚠ `effective_display_space` must not *write*. A settings file shared between a Mac and
        // a Linux machine has to keep a sensible value for the latter.
        let s = Settings {
            display_space: color::OutputSpace::AdobeRgb,
            ..Default::default()
        };
        let _ = s.effective_display_space(true);
        assert_eq!(s.display_space, color::OutputSpace::AdobeRgb);
    }

    /// ⚠⚠ **The default is `Camera`, and the value is what is asserted rather than the
    /// mechanism.** A rendering off by default would leave every RAW opening flat — the complaint
    /// this milestone exists to answer — and nothing else in the suite can see that, because
    /// `Edits::default()` is deliberately still `None` and every golden depends on it.
    #[test]
    fn a_fresh_photo_opens_rendered_and_corrected() {
        let s = Settings::default();
        assert_eq!(
            s.default_rendering,
            edit_model::Rendering::Camera,
            "a fresh RAW must not open colorimetric-only"
        );
        let fresh = s.initial_edits(None);
        // M10.13d: matched to the camera by default; the camera rendering without it when switched off.
        assert_eq!(fresh.rendering, Some(edit_model::Rendering::CameraMatch));
        assert!(fresh.applied_lens_correction().is_active(), "and corrected");
        let plain = Settings {
            camera_match: false,
            ..Settings::default()
        };
        assert_eq!(
            plain.initial_edits(None).rendering,
            Some(edit_model::Rendering::Camera)
        );
    }

    /// ⚠⚠ **THE BUG THIS FUNCTION EXISTS FOR, and it was reported from using the app: the
    /// Camera-rendering toggle "always reset to off" after switching photos.** The old seam
    /// assigned both preferences *unconditionally*, so every photo that had ever been edited had
    /// its stored rendering overwritten with `None` and its stored lens corrections with
    /// all-off — on every single open.
    ///
    /// ⚠ Both fields are asserted, because a fix that restored only the one that was reported
    /// would leave the other silently broken, and the two are set by the same line.
    #[test]
    fn a_photo_with_a_sidecar_keeps_exactly_what_it_stored() {
        let s = Settings::default();
        let stored = edit_model::Edits {
            // The opposite of what each preference would impose, so neither can pass by accident.
            rendering: Some(edit_model::Rendering::None),
            lens_correction: Some(edit_model::LensCorrection::default()),
            exposure: 0.75,
            ..edit_model::Edits::default()
        };
        let got = s.initial_edits(Some(stored.clone()));
        assert_eq!(
            got.rendering,
            Some(edit_model::Rendering::None),
            "the stored rendering was overwritten by the preference"
        );
        assert!(
            !got.applied_lens_correction().is_active(),
            "the stored lens correction was overwritten by the preference"
        );
        assert_eq!(got, stored, "a stored photo must come back untouched");
    }

    /// ⚠ Turning a preference off must reach a fresh photo too, or the setting is inert.
    #[test]
    fn the_preferences_off_open_a_fresh_photo_neutral() {
        let off = Settings {
            default_rendering: edit_model::Rendering::None,
            auto_lens_correction: false,
            ..Settings::default()
        };
        let fresh = off.initial_edits(None);
        assert_eq!(fresh.rendering, None, "undecided, not decided off");
        assert_eq!(fresh.lens_correction, None);
        assert_eq!(
            fresh,
            edit_model::Edits::default(),
            "and nothing else moved"
        );
    }

    #[test]
    fn corrections_are_applied_only_to_a_photo_with_no_sidecar() {
        // ⚠ The whole point of the preference living here rather than in `Edits`: a photo you
        // have already edited keeps exactly what it had, so nothing in the catalog re-renders.
        let on = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        assert!(
            on.initial_edits(None).applied_lens_correction().is_active(),
            "fresh photo"
        );
        assert!(
            !on.initial_edits(Some(edit_model::Edits::default()))
                .applied_lens_correction()
                .is_active(),
            "a photo with a sidecar must be left alone"
        );
    }

    #[test]
    fn the_preference_off_leaves_even_a_fresh_photo_neutral() {
        let off = Settings {
            auto_lens_correction: false,
            ..Settings::default()
        };
        assert_eq!(
            off.initial_lens_correction(),
            None,
            "undecided, not decided off — or every photo would get a sidecar"
        );
    }

    #[test]
    fn the_applied_default_enables_all_three_corrections() {
        let s = Settings {
            auto_lens_correction: true,
            ..Settings::default()
        };
        let l = s.initial_lens_correction().expect("the preference is on");
        assert!(l.distortion && l.tca && l.vignetting, "{l:?}");
        assert_eq!(l.scale, 100.0, "and autoscale by default");
    }
}
