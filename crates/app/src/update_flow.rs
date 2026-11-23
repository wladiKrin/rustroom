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

const DONT_SHOW: &str = "Don't show again";

/// In the status line when no job runs: "Rustroom 0.2.0 is available" (a click opens its release page), then Hide
/// and Don't show again.
/// ⚠ Plain text, not the accent — DESIGN.md §4.2 gives the accent two jobs and this is neither (§7.12).
/// ⚠ Drawn only where the status line keeps [`STATUS_BESIDE`] beside it, and both buttons always read AFTER the line, Hide first —
/// in Develop's right-to-left corner the first widget lands rightmost, so they are added in reverse there.
pub(crate) fn notice(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let s = &app.settings;
    if app.update_hidden || !update_check::may_check(s.check_updates) {
        return;
    }
    let Some(v) = update_check::shown(update_check::current(), s.update_latest.as_deref()) else {
        return;
    };
    let text = format!("Rustroom {v} is available");
    let width = |t: &str, font: egui::FontId| {
        ui.painter()
            .layout_no_wrap(t.to_owned(), font, egui::Color32::PLACEHOLDER)
            .size()
            .x
    };
    let button = |t: &str| {
        width(t, egui::TextStyle::Button.resolve(ui.style())) + 2.0 * ui.spacing().button_padding.x
    };
    let needed = width(&text, egui::FontId::proportional(tokens::CAPTION))
        + button("Hide")
        + button(DONT_SHOW)
        + 3.0 * ui.spacing().item_spacing.x;
    if ui.available_width() < needed + STATUS_BESIDE {
        return;
    }
    if ui.layout().prefer_right_to_left() {
        dont_show_button(app, ui);
        hide_button(app, ui);
        line(ui, &text, v);
    } else {
        line(ui, &text, v);
        hide_button(app, ui);
        dont_show_button(app, ui);
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

/// Hide — the same button as the row's Cancel (one form per control). For this run only: nothing is saved, and the
/// next start shows the line again (the author's call, 2026-11-24).
fn hide_button(app: &mut RustroomApp, ui: &mut egui::Ui) {
    if ui
        .button("Hide")
        .hover("Hides the notice until Rustroom starts again")
        .clicked()
    {
        app.update_hidden = true;
    }
}

/// Don't show again — turns Settings → Behaviour's Check for new versions off, saved; that switch turns it back on.
fn dont_show_button(app: &mut RustroomApp, ui: &mut egui::Ui) {
    if ui
        .button(DONT_SHOW)
        .hover("Stops checking for new versions, as the switch in Settings does")
        .clicked()
    {
        app.settings.check_updates = false;
        app.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustroom_app::update_check::{release_page, Version};

    const LINE: &str = "Rustroom 99.0.0 is available";
    const DONT: &str = "Don't show again";

    /// An app whose settings know of a newer version. ⚠ No Library, on an 800 pt [`screen`]: the Library's right-hand
    /// cluster would leave the notice less than it needs — the row's width rule is `status_bar`'s
    /// `the_update_notice_gives_way_to_the_status_when_narrow`.
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

    /// The row's width: the notice keeps [`STATUS_BESIDE`] beside it, which the harness's 400 pt screen cannot.
    fn screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 200.0))
    }

    fn labels(ctx: &egui::Context, app: &mut RustroomApp) -> Vec<String> {
        crate::test_ui::labelled_centres_on(ctx, screen(), Vec::new(), row(app))
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
        let commands = crate::test_ui::click_commands_on(&ctx, screen(), LINE, row(&mut app));
        let opened: Vec<_> = commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, [release_page(Version(99, 0, 0))]);
    }

    /// Hide is for this run only: the line goes, nothing is saved, and the next start shows it again.
    #[test]
    fn hide_hides_until_the_next_start() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        let mut app = app_knowing(true);
        let before = app.settings.clone();
        crate::test_ui::click_on(&ctx, screen(), "Hide", row(&mut app));
        let found = labels(&ctx, &mut app);
        assert!(!found.iter().any(|l| l == LINE), "hidden now: {found:?}");
        assert_eq!(app.settings, before, "Hide changes no setting");
        let mut next_start = crate::test_app::for_test();
        next_start.settings = app.settings.clone();
        let found = labels(&ctx, &mut next_start);
        assert!(
            found.iter().any(|l| l == LINE),
            "back at the next start: {found:?}"
        );
    }

    /// Don't show again is Settings → Behaviour's switch, turned off and saved.
    #[test]
    fn dont_show_again_turns_the_check_off_and_saves() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        let mut app = app_knowing(true);
        crate::test_ui::click_on(&ctx, screen(), DONT, row(&mut app));
        assert!(!app.settings.check_updates);
        assert!(!app.settings_store.load().check_updates, "and it was saved");
        let found = labels(&ctx, &mut app);
        assert!(!found.iter().any(|l| l == LINE), "{found:?}");
    }

    /// Both buttons read AFTER the line, Hide first, in either row — Develop's corner lays out right to left.
    #[test]
    fn the_buttons_follow_the_line_in_both_rows() {
        let ctx = crate::test_ui::ctx();
        if !drawn_here(&ctx) {
            return;
        }
        for develop in [false, true] {
            let mut app = app_knowing(true);
            let found = crate::test_ui::labelled_centres_on(&ctx, screen(), Vec::new(), |ui| {
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
            assert!(at(DONT).x > at("Hide").x, "develop: {develop}: {found:?}");
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
