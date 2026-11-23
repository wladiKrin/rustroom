//! The new-version check's two ends in the app (piece 4): started at launch, its answer kept in the settings.
//! Every decision is `rustroom_app::update_check`'s.

use crate::theme::Hover as _;
use crate::theme::{self, tokens};
use crate::RustroomApp;
use eframe::egui;
use rustroom_app::update_check;

/// Unix seconds now; a clock before 1970 reads 0, which is "due".
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl RustroomApp {
    /// At launch: starts a check if this copy may check and one is due.
    pub(crate) fn start_update_check(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.update_checker = update_check::start_if_due(
            update_check::may_check(self.settings.check_updates),
            self.settings.update_checked_at,
            now(),
            update_check::live_fetch,
            Box::new(move || ctx.request_repaint()),
        );
    }

    /// Each frame: once the check answers, keep what it found and drop it.
    pub(crate) fn pump_update_check(&mut self) {
        let Some(outcome) = self.update_checker.as_ref().and_then(|c| c.poll()) else {
            return;
        };
        self.update_checker = None;
        update_check::record(&mut self.settings, &outcome, now());
        self.save_settings();
    }
}

/// The room the status line keeps beside the notice; narrower, the notice is not drawn (the row's rule: nothing
/// placed where it merely hopes to fit).
pub(crate) const STATUS_BESIDE: f32 = 120.0;

/// In the status line when no job runs: "Rustroom 0.2.0 is available" (a click opens its release page), then Hide.
/// ⚠ Plain text, not the accent — DESIGN.md §4.2 gives the accent two jobs and this is neither (§7.12).
/// ⚠ Drawn only where the status line keeps [`STATUS_BESIDE`] beside it, and Hide always reads AFTER the line —
/// in Develop's right-to-left corner the first widget lands rightmost, so Hide is added first there.
pub(crate) fn notice(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let s = &app.settings;
    if !update_check::may_check(s.check_updates) {
        return;
    }
    let Some(v) = update_check::shown(
        update_check::current(),
        s.update_latest.as_deref(),
        s.update_dismissed.as_deref(),
    ) else {
        return;
    };
    let text = format!("Rustroom {v} is available");
    let width = |t: &str, font: egui::FontId| {
        ui.painter()
            .layout_no_wrap(t.to_owned(), font, egui::Color32::PLACEHOLDER)
            .size()
            .x
    };
    let needed = width(&text, egui::FontId::proportional(tokens::CAPTION))
        + width("Hide", egui::TextStyle::Button.resolve(ui.style()))
        + 2.0 * ui.spacing().button_padding.x
        + 2.0 * ui.spacing().item_spacing.x;
    if ui.available_width() < needed + STATUS_BESIDE {
        return;
    }
    if ui.layout().prefer_right_to_left() {
        hide_button(app, ui, v);
        line(ui, &text, v);
    } else {
        line(ui, &text, v);
        hide_button(app, ui, v);
    }
}

/// The notice's text; a click opens the release page.
fn line(ui: &mut egui::Ui, text: &str, v: update_check::Version) {
    let line = ui
        .add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(tokens::CAPTION)
                    .color(theme::tokens::of(ui).text),
            )
            .sense(egui::Sense::click())
            .selectable(false),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .hover("Opens the release page in your browser");
    if line.clicked() {
        ui.ctx()
            .open_url(egui::OpenUrl::new_tab(update_check::release_page(v)));
    }
}

/// Hide — the same button as the row's Cancel (one form per control).
fn hide_button(app: &mut RustroomApp, ui: &mut egui::Ui, v: update_check::Version) {
    if ui
        .button("Hide")
        .hover("Hides this version until a newer one comes out")
        .clicked()
    {
        app.settings.update_dismissed = Some(v.to_string());
        app.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustroom_app::update_check::{release_page, Version};

    const LINE: &str = "Rustroom 99.0.0 is available";

    /// An app whose settings know of a newer version. ⚠ No Library: the harness screen is 400 pt, and the Library's
    /// right-hand cluster would leave the notice less than it needs — the row's width rule is
    /// `status_bar`'s `the_update_notice_gives_way_to_the_status_when_narrow`.
    fn app_knowing(on: bool) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.settings.check_updates = on;
        app.settings.update_latest = Some("99.0.0".into());
        app
    }

    fn row(app: &mut RustroomApp) -> impl FnMut(&mut egui::Ui) + '_ {
        move |ui| {
            let ctx = ui.ctx().clone();
            crate::status_bar::library_row(app, ui, &ctx);
        }
    }

    /// Whether this test build draws the notice at all. ⚠ Built with `RUSTROOM_NO_UPDATE_CHECK`, or run inside a
    /// Flatpak, it never does — each test then asserts exactly that, so a packager's `cargo test` still passes and
    /// still checks something.
    fn drawn_here(ctx: &egui::Context) -> bool {
        if update_check::may_check(true) {
            return true;
        }
        let mut app = app_knowing(true);
        let found = labels(ctx, &mut app);
        assert!(!found.iter().any(|l| l == LINE), "{found:?}");
        false
    }

    fn labels(ctx: &egui::Context, app: &mut RustroomApp) -> Vec<String> {
        crate::test_ui::labelled_centres(ctx, Vec::new(), row(app))
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    #[test]
    fn a_newer_version_shows_in_the_status_line() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        let mut app = app_knowing(true);
        let found = labels(&ctx, &mut app);
        assert!(found.iter().any(|l| l == LINE), "{found:?}");
    }

    #[test]
    fn clicking_it_opens_the_release_page() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        let mut app = app_knowing(true);
        let commands = crate::test_ui::click_commands(&ctx, LINE, row(&mut app));
        let opened: Vec<_> = commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, [release_page(Version(99, 0, 0))]);
    }

    #[test]
    fn hide_dismisses_this_version() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        let mut app = app_knowing(true);
        crate::test_ui::click(&ctx, "Hide", row(&mut app));
        assert_eq!(app.settings.update_dismissed.as_deref(), Some("99.0.0"));
        assert_eq!(
            app.settings_store.load().update_dismissed.as_deref(),
            Some("99.0.0"),
            "and it was saved"
        );
        let found = labels(&ctx, &mut app);
        assert!(!found.iter().any(|l| l == LINE), "{found:?}");
    }

    /// Hide reads AFTER the line it hides in either row — Develop's corner lays out right to left.
    #[test]
    fn hide_follows_the_line_in_both_rows() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        for develop in [false, true] {
            let mut app = app_knowing(true);
            let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                let ctx = ui.ctx().clone();
                if develop {
                    crate::status_bar::develop_row(&mut app, ui);
                } else {
                    crate::status_bar::library_row(&mut app, ui, &ctx);
                }
            });
            let at = |label: &str| {
                found
                    .iter()
                    .find(|(l, _)| l == label)
                    .unwrap_or_else(|| {
                        panic!("{label:?} not drawn (develop: {develop}): {found:?}")
                    })
                    .1
            };
            assert!(at("Hide").x > at(LINE).x, "develop: {develop}: {found:?}");
        }
    }

    #[test]
    fn turning_the_check_off_hides_the_line() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        let mut on = app_knowing(true);
        assert!(
            labels(&ctx, &mut on).iter().any(|l| l == LINE),
            "fixture check: on, the line shows"
        );
        let mut off = app_knowing(false);
        let found = labels(&ctx, &mut off);
        assert!(!found.iter().any(|l| l == LINE), "{found:?}");
    }
}
