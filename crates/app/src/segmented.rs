//! The "one of N lit" controls: the segmented well, the content-sized pill, and the chip.
//!
//! ⚠ Moved out of `theme.rs` (923 lines) on 2026-09-20 when the pill joined; `theme`
//! re-exports the segmented helpers so their callers are untouched.
//!
//! Three shapes, one idea:
//! - [`segmented`] fills the width it is given and wraps its items into rows when they do not
//!   fit — the panels' view switches (HSL channel, grading range, mask mode).
//! - [`pill`] is sized to its content and carries an icon per item — the top bar's app mode
//!   and the Library's layout mode, where the row is shared with other controls and a
//!   full-width control would take it all.
//! - [`chip`] is one item on its own: a toggle with a dot — the format filter.

use crate::theme;
use eframe::egui;

/// A segmented control: one full-width well, `items.len()` equal columns, exactly one lit.
///
/// ⚠ For a **view switch** over a fixed small set — the HSL channel, the grading range — not
/// for a list of options with its own reasons to be `selectable_label`s (aspect presets, looks,
/// composition guides). Each column carries `WidgetInfo::selected` with its label, so
/// `test_ui::click(ctx, "Saturation", ..)` reaches it exactly as it reached the chips.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[(T, &str)],
) -> egui::Response {
    let with: Vec<(T, &str, &str)> = items.iter().map(|(t, l)| (*t, *l, "")).collect();
    segmented_with_hover(ui, current, &with)
}

/// [`segmented`] with a hover text per item (an empty one shows nothing).
pub fn segmented_with_hover<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[(T, &str, &str)],
) -> egui::Response {
    let font = theme::section_font(ui.style());
    segmented_sized(ui, current, items, SEGMENTED_HEIGHT, font)
}

/// The compact segmented control's height: the HSL and grading selectors.
pub const SEGMENTED_HEIGHT: f32 = 18.0;

/// A segmented control at a given height and font. The Masks panel's mode control is drawn
/// taller and in the body face (user's call, 2026-09-19: the compact one read too small for
/// the panel's primary control).
///
/// ⚠⚠ **It never lets a label overflow its cell.** The columns are the most that still hold
/// the widest label at this font; when the panel is too narrow for one row the items wrap onto
/// as many rows as that takes (four items at the panel's default 240 pt become a 2 × 2 block).
/// The first version drew four body-face labels into one row and they overlapped at the
/// default width — seen in use, 2026-09-19.
pub fn segmented_sized<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[(T, &str, &str)],
    height: f32,
    font: egui::FontId,
) -> egui::Response {
    /// Horizontal room around a label inside its cell.
    const PAD: f32 = 10.0;
    const GAP: f32 = 2.0;
    let n = items.len().max(1);
    let width = ui.available_width();
    let widest = ui.fonts_mut(|f| {
        items
            .iter()
            .map(|(_, l, _)| {
                f.layout_no_wrap((*l).to_owned(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0, f32::max)
    });
    let inner_w = (width - 4.0).max(1.0);
    let cols = (1..=n)
        .rev()
        .find(|&c| inner_w / c as f32 >= widest + PAD)
        .unwrap_or(1);
    let rows = n.div_ceil(cols);
    let total = rows as f32 * height + (rows as f32 - 1.0) * GAP;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, total), egui::Sense::hover());
    let v = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, v.extreme_bg_color);
    let inner = rect.shrink(2.0);
    let w = inner.width() / cols as f32;
    let row_h = (inner.height() - (rows as f32 - 1.0) * GAP) / rows as f32;
    let mut all = resp.clone();
    for (i, (item, label, hover)) in items.iter().enumerate() {
        let (col, row) = (i % cols, i / cols);
        let cell = egui::Rect::from_min_size(
            egui::pos2(
                inner.left() + w * col as f32,
                inner.top() + (row_h + GAP) * row as f32,
            ),
            egui::vec2(w, row_h),
        );
        let r = ui.interact(cell, resp.id.with(i), egui::Sense::click());
        let r = if hover.is_empty() {
            r
        } else {
            r.on_hover_text(*hover)
        };
        let enabled = ui.is_enabled();
        let selected = *current == *item;
        r.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, selected, *label)
        });
        if r.clicked() {
            *current = *item;
        }
        let lit = *current == *item;
        if lit {
            painter.rect_filled(cell, 3.0, v.widgets.inactive.bg_fill);
        } else if r.hovered() {
            painter.rect_filled(cell, 3.0, v.widgets.hovered.weak_bg_fill);
        }
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if lit {
                v.strong_text_color()
            } else {
                v.text_color()
            },
        );
        all = all.union(r);
    }
    all
}

/// One item of a [`pill`]: its value, its glyph, its label and its hover text.
pub struct PillItem<'a, T> {
    pub value: T,
    pub icon: char,
    pub label: &'a str,
    pub hover: &'a str,
}

/// How a pill is drawn.
pub struct PillStyle {
    /// The well's height in points.
    pub height: f32,
    /// The label face.
    pub font: egui::FontId,
    /// Fill the lit cell with the accent rather than the button fill. ⚠ For the app mode only:
    /// it is the one accent-filled control in the bar, and a second would cost the first its
    /// meaning.
    pub accent: bool,
}

/// A segmented control sized to its content: every cell as wide as the widest icon + label.
///
/// ⚠ Never the available width. It sits in a horizontal row with other controls, and
/// [`segmented`]'s "fill what you are given" would take the whole bar. Each cell carries
/// `WidgetInfo::selected` with its label alone — the glyph is a picture, not a word, so
/// `test_ui::click(ctx, "Library", ..)` reaches it and a screen reader says the name.
pub fn pill<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[PillItem<'_, T>],
    style: PillStyle,
) -> egui::Response {
    const PAD: f32 = 12.0;
    const GAP: f32 = 6.0;
    let icon_font = egui::FontId::proportional(style.font.size + 3.0);
    let (label_w, icon_w) = ui.fonts_mut(|f| {
        let lw = items
            .iter()
            .map(|it| {
                f.layout_no_wrap(
                    it.label.to_owned(),
                    style.font.clone(),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
            .fold(0.0, f32::max);
        let iw = items
            .iter()
            .map(|it| {
                f.layout_no_wrap(it.icon.to_string(), icon_font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0, f32::max);
        (lw, iw)
    });
    let cell_w = (icon_w + GAP + label_w + PAD * 2.0).ceil();
    let n = items.len().max(1) as f32;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(cell_w * n + 4.0, style.height),
        egui::Sense::hover(),
    );
    let v = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 5.0, v.extreme_bg_color);
    let inner = rect.shrink(2.0);
    let mut all = resp.clone();
    for (i, it) in items.iter().enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(inner.left() + cell_w * i as f32, inner.top()),
            egui::vec2(cell_w, inner.height()),
        );
        let r = ui.interact(cell, resp.id.with(i), egui::Sense::click());
        let r = if it.hover.is_empty() {
            r
        } else {
            r.on_hover_text(it.hover)
        };
        let enabled = ui.is_enabled();
        let selected = *current == it.value;
        r.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                enabled,
                selected,
                it.label,
            )
        });
        if r.clicked() {
            *current = it.value;
        }
        let lit = *current == it.value;
        let colour = if lit && style.accent {
            painter.rect_filled(cell, 4.0, v.selection.bg_fill);
            // ⚠ White on the accent in both themes: every accent pair was contrast-checked
            // against white text (`every_accent_has_usable_contrast`), not against the
            // theme's strong text, which is near-black on light.
            egui::Color32::WHITE
        } else if lit {
            painter.rect_filled(cell, 4.0, v.widgets.inactive.bg_fill);
            v.strong_text_color()
        } else if r.hovered() {
            painter.rect_filled(cell, 4.0, v.widgets.hovered.weak_bg_fill);
            v.strong_text_color()
        } else {
            v.text_color()
        };
        let x = cell.left() + PAD;
        let icon_rect = painter.text(
            egui::pos2(x, cell.center().y),
            egui::Align2::LEFT_CENTER,
            it.icon,
            icon_font.clone(),
            colour,
        );
        painter.text(
            egui::pos2(icon_rect.right() + GAP, cell.center().y),
            egui::Align2::LEFT_CENTER,
            it.label,
            style.font.clone(),
            colour,
        );
        all = all.union(r);
    }
    all
}

/// A chip: a dot and a label in a rounded well, toggling `on`. The dot is the accent while on
/// and dim while off, so the state is visible without reading.
///
/// ⚠ Reports as a checkbox: it *is* one, drawn as a chip — the format filter's semantics did
/// not change with its clothes.
pub fn chip(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    const H: f32 = 20.0;
    const DOT: f32 = 3.5;
    let font = egui::FontId::proportional(11.0);
    let w = ui.fonts_mut(|f| {
        f.layout_no_wrap(label.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    });
    let (rect, mut r) =
        ui.allocate_exact_size(egui::vec2(w + DOT * 2.0 + 18.0, H), egui::Sense::click());
    let enabled = ui.is_enabled();
    let was = *on;
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, was, label));
    // ⚠ `mark_changed`, or a caller reading `changed()` — as the format filter does, exactly
    // as it read the checkbox it replaced — sees nothing and the chips are "not clickable"
    // (seen in use 2026-09-20). A hand-drawn widget has to say so itself.
    // ⚠ Gated on the ui being enabled: a hand-drawn widget gets no greying and no gate from
    // `add_enabled_ui` for free, and the format filter draws the only format in a folder as a
    // chip that informs and cannot hide everything.
    if enabled && r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let v = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        H / 2.0,
        if r.hovered() {
            v.widgets.hovered.weak_bg_fill
        } else {
            v.widgets.inactive.weak_bg_fill
        },
    );
    let (dot, text) = if *on {
        (v.selection.bg_fill, v.widgets.inactive.fg_stroke.color)
    } else {
        (v.weak_text_color().gamma_multiply(0.5), v.weak_text_color())
    };
    painter.circle_filled(
        egui::pos2(rect.left() + 7.0 + DOT, rect.center().y),
        DOT,
        dot,
    );
    painter.text(
        egui::pos2(rect.left() + 7.0 + DOT * 2.0 + 5.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        text,
    );
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Mode {
        A,
        B,
    }

    fn items() -> [PillItem<'static, Mode>; 2] {
        [
            PillItem {
                value: Mode::A,
                icon: 'a',
                label: "Library",
                hover: "",
            },
            PillItem {
                value: Mode::B,
                icon: 'b',
                label: "Develop",
                hover: "",
            },
        ]
    }

    fn style() -> PillStyle {
        PillStyle {
            height: 32.0,
            font: egui::FontId::proportional(13.0),
            accent: true,
        }
    }

    #[test]
    fn a_pill_is_reached_by_its_labels_and_a_click_switches() {
        // ⚠ The glyph must not be part of the accessible name: the harness and a screen
        // reader want "Develop", not a private-use character and then "Develop".
        let ctx = test_ui::ctx();
        let mut mode = Mode::A;
        let labels: Vec<String> = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            pill(ui, &mut mode, &items(), style());
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect();
        assert!(labels.contains(&"Library".to_string()), "{labels:?}");
        assert!(labels.contains(&"Develop".to_string()), "{labels:?}");
        test_ui::click(&ctx, "Develop", |ui| {
            pill(ui, &mut mode, &items(), style());
        });
        assert_eq!(mode, Mode::B);
    }

    #[test]
    fn a_pill_is_sized_to_its_content_not_the_row() {
        // ⚠ The whole reason it is not `segmented`: in the top bar it shares the row.
        let ctx = test_ui::ctx();
        let mut w = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                ..Default::default()
            },
            |ui| {
                let mut mode = Mode::A;
                w = pill(ui, &mut mode, &items(), style()).rect.width();
            },
        );
        assert!(w > 60.0 && w < 260.0, "pill width {w}");
    }

    #[test]
    fn a_chip_reports_as_a_checkbox_and_toggles() {
        let ctx = test_ui::ctx();
        let mut on = true;
        let mut changed = false;
        test_ui::click(&ctx, "RAF 12", |ui| {
            changed |= chip(ui, &mut on, "RAF 12").changed();
        });
        assert!(!on, "a click did not toggle the chip");
        // ⚠ The format filter reads `changed()`, not the flag — a chip that flips the flag
        // silently is one nothing downstream notices.
        assert!(changed, "the chip did not report the change");
    }
}
