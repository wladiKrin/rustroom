//! The one shell every dialog is drawn through: a title bar with a ×, the body in a scroll
//! area, a footer pinned under it with the accent verb — and the tab control the long dialogs
//! split their body with.
//!
//! ⚠ Its own file (2026-09-21) because `modals/mod.rs` was 572 lines of dispatch and small
//! dialogs before the shell grew a footer and tabs.
//!
//! ⚠ `egui::Modal`, not `egui::Window`: a Window opens at the top-left and is genuinely easy
//! to lose behind the canvas; a Modal is centred and dims the rest. The size is remembered
//! through `Settings::dialog_size` / `remember_dialog_size`, measured from `Resize`'s own
//! content `Ui`, whose extent is the same quantity `default_size` sets — the round trip
//! `a_dialogs_measured_size_round_trips_and_does_not_creep` pins, because
//! `Resize::State::desired_size` is `pub(crate)` and could not be read directly.

use crate::theme;
use eframe::egui;
use rustroom_app::settings::Settings;

/// The title bar's height.
const TITLE_BAR: f32 = 44.0;
/// The footer's height.
const FOOTER: f32 = 52.0;
/// The tab control's height — a step taller than the panels' compact selectors: it is the
/// dialog's primary navigation.
const TABS: f32 = crate::theme::tokens::CONTROL_ROW;

/// What the footer offers.
pub struct Footer<'a> {
    /// The accent-filled verb, and whether it is enabled. `None` for an informational dialog.
    pub primary: Option<(&'a str, bool)>,
    /// "Cancel", "Close" or "Done" — what the plain button says.
    pub dismiss: &'a str,
    /// Plain buttons left of dismiss, in this order. Empty for every dialog but one.
    pub extra: &'a [&'a str],
    /// Weak text at the footer's left. Empty for none.
    pub note: &'a str,
}

impl<'a> Footer<'a> {
    /// A confirm-or-cancel footer.
    pub fn confirm(primary: &'a str, enabled: bool, dismiss: &'a str) -> Self {
        Self {
            primary: Some((primary, enabled)),
            dismiss,
            extra: &[],
            note: "",
        }
    }
    /// An informational dialog's footer: one plain button.
    pub fn close(dismiss: &'a str) -> Self {
        Self {
            primary: None,
            dismiss,
            extra: &[],
            note: "",
        }
    }
    pub fn note(mut self, note: &'a str) -> Self {
        self.note = note;
        self
    }
    pub fn extra(mut self, extra: &'a [&'a str]) -> Self {
        self.extra = extra;
        self
    }
}

/// What the dialog reported this frame.
#[derive(Default, Debug)]
pub struct DialogOut {
    /// The backdrop, the × or the dismiss button.
    pub dismissed: bool,
    /// The primary was clicked (never when disabled).
    pub primary: bool,
    /// Which extra button was clicked, as an index into `Footer::extra`.
    pub extra: Option<usize>,
}

/// Draw a dialog. Returns what its chrome reported; the body's own widgets report through
/// whatever the caller captured in `content`.
pub fn dialog(
    ctx: &egui::Context,
    settings: &mut Settings,
    id: &str,
    title: &str,
    size: theme::DialogSize,
    footer: Footer<'_>,
    content: impl FnOnce(&mut egui::Ui),
) -> DialogOut {
    // ⚠ Read **before** the content draws: a focused text field consumes Escape when it runs,
    // so a check after the body saw nothing while the Export dialog's naming field had focus
    // (P-04). One press closes the dialog from anywhere inside it.
    let escaped = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let min = size.min();
    let seed = settings.dialog_size(id, min, size.default_size());
    let vp = ctx.viewport_rect().size();
    let max = (vp.x * 0.8, vp.y * 0.8);
    let mut measured = seed;
    let mut out = DialogOut::default();

    let backdrop = egui::Modal::new(egui::Id::new(id))
        .frame(theme::dialog_frame(ctx))
        .show(ctx, |ui| {
            egui::Resize::default()
                .id_salt(id)
                .default_size(seed)
                .min_size(min)
                .max_size(max)
                .resizable(true)
                .show(ui, |ui| {
                    measured = (ui.available_width(), ui.available_height());
                    if title_bar(ui, egui::Id::new(id), title) {
                        out.dismissed = true;
                    }
                    // The body takes what the footer leaves, so the footer never scrolls.
                    // ⚠ Less the spacing egui puts between the body and the footer: without
                    // it the content overshot the region by 4 pt and the dialog crept 4 pt
                    // taller on every reopen — `a_dialogs_measured_size_round_trips_and_does_not_creep`.
                    let body_h =
                        (ui.available_height() - FOOTER - ui.spacing().item_spacing.y).max(0.0);
                    // ⚠ `min_scrolled_height(0)`: egui's default of 64 overrode a smaller
                    // `max_height` on a Compact dialog, the content overshot the region and
                    // the dialog grew 4 pt on its first reopen — the probe read 63 where 58
                    // was asked.
                    egui::ScrollArea::vertical()
                        .id_salt((id, "body"))
                        .auto_shrink([false, false])
                        .min_scrolled_height(0.0)
                        .max_height(body_h)
                        .show(ui, |ui| {
                            content(ui);
                        });
                    footer_row(ui, egui::Id::new(id), &footer, &mut out);
                });
        })
        .backdrop_response
        .clicked();
    out.dismissed |= backdrop || escaped;

    settings.remember_dialog_size(id, measured);
    out
}

/// The title in the strong body face at the left, a × at the right, a hairline under.
/// Returns whether the × was clicked.
fn title_bar(ui: &mut egui::Ui, id: egui::Id, title: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TITLE_BAR),
        egui::Sense::hover(),
    );
    let v = ui.visuals().clone();
    let cell = egui::Rect::from_min_max(
        egui::pos2(rect.right() - TITLE_BAR + 6.0, rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
    );
    let close = ui.interact(cell, id.with("close"), egui::Sense::click());
    let enabled = ui.is_enabled();
    close.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "Close"));
    let painter = ui.painter_at(rect);
    painter.text(
        egui::pos2(rect.left(), rect.center().y - 3.0),
        egui::Align2::LEFT_CENTER,
        title,
        crate::theme::title_font(ui.style()),
        v.strong_text_color(),
    );
    painter.text(
        egui::pos2(cell.center().x, rect.center().y - 3.0),
        egui::Align2::CENTER_CENTER,
        "×",
        egui::FontId::proportional(crate::theme::tokens::ICON),
        if close.hovered() {
            v.strong_text_color()
        } else {
            v.weak_text_color()
        },
    );
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 1.0),
            egui::pos2(rect.right(), rect.bottom() - 1.0),
        ],
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
    );
    close.clicked()
}

/// The footer: a hairline, the note at the left, the buttons right-aligned — the primary
/// rightmost, dismiss left of it, the extras left of that in their given order.
fn footer_row(ui: &mut egui::Ui, id: egui::Id, footer: &Footer<'_>, out: &mut DialogOut) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), FOOTER),
        egui::Sense::hover(),
    );
    let v = ui.visuals().clone();
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), rect.top()),
            egui::pos2(rect.right(), rect.top()),
        ],
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
    );
    let row = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 8.0), rect.max);
    let used_left = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::right_to_left(egui::Align::Center))
                .id_salt(id.with("footer")),
            |ui| {
                if let Some((label, enabled)) = footer.primary {
                    if theme::primary_button(ui, label, enabled).clicked() {
                        out.primary = true;
                    }
                }
                if ui.button(footer.dismiss).clicked() {
                    out.dismissed = true;
                }
                // Right to left, so the last extra is added first and the on-screen order is
                // the given one.
                for (i, label) in footer.extra.iter().enumerate().rev() {
                    if ui.button(*label).clicked() {
                        out.extra = Some(i);
                    }
                }
                ui.min_rect().left()
            },
        )
        .inner;
    if !footer.note.is_empty() {
        let right = if used_left.is_finite() {
            used_left - 8.0
        } else {
            row.right()
        };
        let width = (right - row.left()).max(0.0);
        if width > 20.0 {
            let mut job = egui::text::LayoutJob::simple_singleline(
                footer.note.to_string(),
                egui::FontId::proportional(crate::theme::tokens::CAPTION),
                v.weak_text_color(),
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(width);
            let galley = ui.painter().layout_job(job);
            ui.painter().galley(
                egui::pos2(row.left(), row.center().y - galley.size().y / 2.0),
                galley,
                v.weak_text_color(),
            );
        }
    }
}

/// The tab control at the top of a long dialog's body. Returns the current tab.
///
/// ⚠ The index lives in egui's temporary memory under `id` — per dialog, per process: opening
/// the dialog again in the same session lands on the last tab, a restart on the first. Not in
/// `Settings`: a remembered tab is a convenience, and the settings file is for decisions.
pub fn tabs(ui: &mut egui::Ui, id: egui::Id, names: &[&str]) -> usize {
    let plain: Vec<(&str, &str)> = names.iter().map(|n| (*n, "")).collect();
    tabs_with_hover(ui, id, &plain)
}

/// [`tabs`] with a hover text per tab — the Export dialog says whether a tab's section is
/// doing anything ("Metadata is on") without opening it.
pub fn tabs_with_hover(ui: &mut egui::Ui, id: egui::Id, names: &[(&str, &str)]) -> usize {
    let n = names.len().max(1);
    let mut current = ui
        .data_mut(|d| d.get_temp::<usize>(id))
        .unwrap_or(0)
        .min(n - 1);
    let items: Vec<(usize, &str, &str)> = names
        .iter()
        .enumerate()
        .map(|(i, (s, h))| (i, *s, *h))
        .collect();
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.add_space(4.0);
    theme::segmented_sized(ui, &mut current, &items, TABS, font);
    ui.add_space(6.0);
    ui.data_mut(|d| d.insert_temp(id, current));
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;
    use crate::theme::DialogSize;

    /// Run `dialog` once on a viewport of `size`, returning what it recorded.
    fn record_once(
        ctx: &egui::Context,
        settings: &mut Settings,
        size: egui::Vec2,
    ) -> Option<[u32; 2]> {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| {
                dialog(
                    ui.ctx(),
                    settings,
                    "round_trip",
                    "Round trip",
                    DialogSize::Compact,
                    Footer::close("Close"),
                    |ui| {
                        ui.label("body");
                    },
                );
            },
        );
        settings.dialog_sizes.get("round_trip").copied()
    }

    /// ⚠⚠ **The round trip the design could not confirm from egui's source**, because
    /// `Resize::State::desired_size` is `pub(crate)`. `dialog` measures the content `Ui`'s
    /// available size and feeds it back as `default_size` next time; if those are not the same
    /// quantity, a dialog creeps larger or smaller every time it is opened.
    ///
    /// Three cycles, because a one-frame check cannot see a creep. ⚠ On a **large** viewport,
    /// so nothing is clamped and the recorded size can be compared to the class default.
    #[test]
    fn a_dialogs_measured_size_round_trips_and_does_not_creep() {
        let ctx = test_ui::ctx();
        let mut settings = Settings::default();
        let big = egui::vec2(1600.0, 1200.0);
        let seen: Vec<Option<[u32; 2]>> = (0..3)
            .map(|_| record_once(&ctx, &mut settings, big))
            .collect();
        let (w, h) = DialogSize::Compact.default_size();
        let want = [w as u32, h as u32];
        assert!(
            seen.iter().all(|s| *s == Some(want)),
            "the size must round-trip at the class default, got {seen:?}, wanted {want:?}"
        );
    }

    /// ⚠ The other half: a viewport too small for the class **clamps**, and stays clamped rather
    /// than shrinking again on each reopen.
    #[test]
    fn a_small_viewport_clamps_the_dialog_once_and_then_holds() {
        let ctx = test_ui::ctx();
        let mut settings = Settings::default();
        let small = egui::vec2(400.0, 300.0);
        let seen: Vec<Option<[u32; 2]>> = (0..3)
            .map(|_| record_once(&ctx, &mut settings, small))
            .collect();
        // 80% of 400 is 320, below Compact's 340 default and above its 300 minimum. The height
        // is Compact's default, 160 since direction B's 44-pt head and 52-pt foot (2026-09-29).
        assert_eq!(seen[0], Some([320, 160]), "clamped to the viewport");
        assert!(
            seen.iter().all(|s| *s == seen[0]),
            "and then stable, got {seen:?}"
        );
    }

    /// Draw a dialog with `footer` and report its accessible labels and what it returned.
    fn chrome(footer: Footer<'static>, click: Option<&str>) -> (Vec<String>, DialogOut) {
        let ctx = test_ui::ctx();
        let mut settings = Settings::default();
        let mut last = DialogOut::default();
        let Footer {
            primary,
            dismiss,
            extra,
            note,
        } = footer;
        let mut draw = |ui: &mut egui::Ui| {
            let out = dialog(
                ui.ctx(),
                &mut settings,
                "chrome",
                "Chrome",
                DialogSize::Standard,
                Footer {
                    primary,
                    dismiss,
                    extra,
                    note,
                },
                |ui| {
                    ui.label("body");
                },
            );
            if out.dismissed || out.primary || out.extra.is_some() {
                last = out;
            }
        };
        let labels: Vec<String> = test_ui::labelled_centres(&ctx, Vec::new(), &mut draw)
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        if let Some(c) = click {
            test_ui::click(&ctx, c, &mut draw);
        }
        (labels, last)
    }

    #[test]
    fn the_footers_buttons_and_the_close_are_named_and_reported() {
        let footer = Footer::confirm("Forget", true, "Cancel").extra(&["Skip", "Overwrite"]);
        let (labels, _) = chrome(
            Footer {
                primary: footer.primary,
                dismiss: footer.dismiss,
                extra: footer.extra,
                note: footer.note,
            },
            None,
        );
        for want in ["Forget", "Cancel", "Skip", "Overwrite", "Close"] {
            assert!(
                labels.iter().any(|l| l == want),
                "{want:?} missing in {labels:?}"
            );
        }
        let (_, out) = chrome(Footer::confirm("Forget", true, "Cancel"), Some("Forget"));
        assert!(out.primary && !out.dismissed);
        let (_, out) = chrome(Footer::confirm("Forget", true, "Cancel"), Some("Cancel"));
        assert!(out.dismissed && !out.primary);
        let (_, out) = chrome(Footer::confirm("Forget", true, "Cancel"), Some("Close"));
        assert!(out.dismissed, "the × must dismiss");
        let (_, out) = chrome(
            Footer::confirm("Keep both", true, "Cancel export").extra(&["Skip", "Overwrite"]),
            Some("Overwrite"),
        );
        assert_eq!(out.extra, Some(1));
    }

    #[test]
    fn a_disabled_primary_is_shown_and_does_nothing() {
        // ⚠ Shown, greyed, never hidden: the Save button with an empty name must still be seen.
        let (labels, out) = chrome(Footer::confirm("Save", false, "Cancel"), Some("Save"));
        assert!(labels.iter().any(|l| l == "Save"));
        assert!(!out.primary, "a disabled primary reported a click");
    }

    #[test]
    fn the_tabs_switch_the_content_and_remember_within_a_context() {
        let ctx = test_ui::ctx();
        let id = egui::Id::new("tabs_test");
        let mut draw = |ui: &mut egui::Ui| match tabs(ui, id, &["One", "Two"]) {
            0 => {
                ui.checkbox(&mut true, "first content");
            }
            _ => {
                ui.checkbox(&mut true, "second content");
            }
        };
        let labels = |ctx: &egui::Context, draw: &mut dyn FnMut(&mut egui::Ui)| -> Vec<String> {
            test_ui::labelled_centres(ctx, Vec::new(), |ui| draw(ui))
                .into_iter()
                .map(|(l, _)| l)
                .collect()
        };
        let first = labels(&ctx, &mut draw);
        assert!(first.iter().any(|l| l == "first content"), "{first:?}");
        assert!(!first.iter().any(|l| l == "second content"));
        test_ui::click(&ctx, "Two", &mut draw);
        let second = labels(&ctx, &mut draw);
        assert!(second.iter().any(|l| l == "second content"), "{second:?}");
        assert!(!second.iter().any(|l| l == "first content"));
        // A fresh context starts on the first tab: the memory is per process, not persisted.
        let fresh = test_ui::ctx();
        let again = labels(&fresh, &mut draw);
        assert!(again.iter().any(|l| l == "first content"));
    }
}
