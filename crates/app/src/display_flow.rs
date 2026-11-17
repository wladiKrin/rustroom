//! The display's own colours, followed (2026-11-18): every two seconds `display_detect` gathers its facts for the
//! monitor holding the window; when they change it decides again, and when the display that means changes it is
//! applied as a Settings change is (`apply_display`). So moving the window to another monitor, or switching
//! Windows' Auto Color Management on, is followed within a couple of seconds of the next frame.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::display_detect;
use std::time::{Duration, Instant};

/// How often the facts are gathered — cheap ones (a registry value, a profile's name, a few sysfs files).
const EVERY: Duration = Duration::from_secs(2);

impl RustroomApp {
    /// What the preview, the thumbnails and the rendered previews convert into now.
    pub(crate) fn display(&self) -> color::Display {
        self.settings
            .effective_display(self.os_manages_colour, &self.display_detected)
    }

    pub(crate) fn pump_display(&mut self, ctx: &egui::Context) {
        if self.display_checked.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.display_checked = Some(Instant::now());
        let facts = (self.display_gather)(self.window_handle);
        if facts == self.display_facts {
            return;
        }
        let before = self.display();
        self.display_detected = display_detect::decide(&facts);
        self.display_facts = facts;
        if std::env::var_os("RUSTROOM_DEBUG_DISPLAY").is_some() {
            eprintln!("display: {:?}", self.display_detected);
        }
        if self.display() != before {
            self.apply_display(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustroom_app::display_detect::{Detected, Facts};

    /// CAPTURED — the author's M27Q (`color/testdata/README.md`).
    const M27Q: &[u8] = include_bytes!("../../color/testdata/m27q.edid");

    /// The pump's whole wiring, headless: a monitor whose colours appear is followed — by the app's display AND the
    /// open session's — and an OS that starts matching colour itself turns the conversion off again.
    #[test]
    fn a_monitors_colours_are_followed_and_the_os_taking_over_is_too() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 8, 6);
        app.pump_display(&ctx);
        assert!(
            app.display().is_srgb(),
            "positive control: no facts, no conversion"
        );

        app.display_gather = |_| Facts {
            edids: vec![M27Q.to_vec()],
            ..Facts::default()
        };
        app.display_checked = None;
        app.pump_display(&ctx);
        assert!(matches!(app.display_detected, Detected::Profile(..)));
        assert!(!app.display().is_srgb());
        assert_eq!(
            app.session.as_ref().unwrap().display_space,
            app.display(),
            "the open session follows"
        );

        app.display_gather = |_| Facts {
            os_managed: true,
            edids: vec![M27Q.to_vec()],
            ..Facts::default()
        };
        app.pump_display(&ctx);
        assert!(
            !app.display().is_srgb(),
            "not re-gathered before two seconds have passed"
        );
        app.display_checked = None;
        app.pump_display(&ctx);
        assert_eq!(app.display_detected, Detected::OsManaged);
        assert!(app.display().is_srgb());
        assert!(app.session.as_ref().unwrap().display_space.is_srgb());
    }
}
