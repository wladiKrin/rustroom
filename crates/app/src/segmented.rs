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
//! - [`switch`] is a chunky sliding toggle for a **consequential** on/off — enabling the Immich
//!   integration. A checkbox reads as one setting among many; this reads as a decision.

use crate::theme;
use crate::theme::Hover as _;
use eframe::egui;

/// A segmented control: one full-width well, `items.len()` equal columns, exactly one lit.
///
/// ⚠ For a **view switch** over a fixed small set — the HSL channel, the grading range — not
/// for a list of options with its own reasons to be `selectable_label`s (aspect presets, looks,
/// composition guides). Each column carries `WidgetInfo::selected` with its label, so
/// `test_ui::click(ctx, "Saturation", ..)` reaches it exactly as it reached the chips.
///
/// ⚠ Each item is `(value, label, hover sentence)` — the sentence is part of the signature (M10.6b),
/// so a segment cannot be added mute without writing `""` on purpose, which `hover_coverage` then
/// reports wherever it can draw the control.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[(T, &str, &str)],
) -> egui::Response {
    let font = theme::section_font(ui.style());
    segmented_sized(ui, current, items, SEGMENTED_HEIGHT, font)
}

/// The compact segmented control's height: the HSL and grading selectors, the curve channels.
///
/// ⚠ 24 with caption labels, where a primary switch (the mask mode, the app switch) is the
/// control row's 28 with body labels — `docs/DESIGN.md` §7.2. One shape at two sizes.
pub const SEGMENTED_HEIGHT: f32 = 24.0;

/// How much a segmented control's labels shrink to stay on one row.
const SHRINK: f32 = 0.85;

/// The size a label shrinks to: 15 % smaller, but **never below Caption** — `docs/DESIGN.md` §3's
/// floor.
///
/// ⚠⚠ Until 2026-09-29 the shrink was unconditional, and 15 % off Caption is 9.35 pt: the tone
/// curve's five channels rendered exactly there, "all very small" in the author's words. A
/// Caption-sized control now wraps instead of shrinking.
pub fn shrunk_size(size: f32) -> f32 {
    (size * SHRINK).max(crate::theme::tokens::CAPTION)
}

/// What sits before a segment's label.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mark {
    /// The label alone.
    None,
    /// An icon-font glyph from `icons.rs` — verified against the bundled cmap, as every glyph must
    /// be — drawn at `tokens::ICON` in the label's colour.
    Icon(char),
    /// A small dot in its own colour: the tone curve's channels.
    Dot(egui::Color32),
}

/// A dot mark's diameter.
const MARK_DOT: f32 = 7.0;
/// The space between a mark and its label.
const MARK_GAP: f32 = 6.0;

impl Mark {
    /// Its width plus the gap after it; nothing for [`Mark::None`].
    fn width(self) -> f32 {
        match self {
            Mark::None => 0.0,
            Mark::Icon(_) => crate::theme::tokens::ICON + MARK_GAP,
            Mark::Dot(_) => MARK_DOT + MARK_GAP,
        }
    }
}

/// One segment: its value, its label (also its accessible name), its hover, and what sits before
/// the label.
pub struct Segment<'a, T> {
    pub value: T,
    pub label: &'a str,
    pub hover: &'a str,
    pub mark: Mark,
}

/// Where a segmented control's items go: how many per row, and whether the labels shrank.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub cols: usize,
    pub shrink: bool,
}

/// The layout for `n` items in `inner_w` points: one row at full size if it fits, else one row
/// at the shrunken size if *that* fits, else the fewest rows that fit at full size, **balanced** —
/// four items that would fit three to a row are drawn two and two, never three and one.
pub fn plan(inner_w: f32, n: usize, widest: f32, widest_small: f32) -> Plan {
    const PAD: f32 = 10.0;
    const PAD_SMALL: f32 = 6.0;
    let n = n.max(1);
    if inner_w / n as f32 >= widest + PAD {
        return Plan {
            cols: n,
            shrink: false,
        };
    }
    if inner_w / n as f32 >= widest_small + PAD_SMALL {
        return Plan {
            cols: n,
            shrink: true,
        };
    }
    let fit = (1..=n)
        .rev()
        .find(|&c| inner_w / c as f32 >= widest + PAD)
        .unwrap_or(1);
    // The fewest rows, then as few columns as fill them: a 3 + 1 block reads as a broken row.
    let rows = n.div_ceil(fit);
    Plan {
        cols: n.div_ceil(rows),
        shrink: false,
    }
}

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
    let segments: Vec<Segment<'_, T>> = items
        .iter()
        .map(|&(value, label, hover)| Segment {
            value,
            label,
            hover,
            mark: Mark::None,
        })
        .collect();
    segmented_marked(ui, current, &segments, height, font)
}

/// [`segmented_sized`] with a [`Mark`] before each label — the mask modes' icons, the curve
/// channels' dots. The mark is part of the width a cell must hold, and never shrinks.
pub fn segmented_marked<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[Segment<'_, T>],
    height: f32,
    font: egui::FontId,
) -> egui::Response {
    /// Horizontal room around a label inside its cell.
    const GAP: f32 = 2.0;
    let n = items.len().max(1);
    let width = ui.available_width();
    let widest_at = |ui: &mut egui::Ui, font: &egui::FontId| {
        ui.fonts_mut(|f| {
            items
                .iter()
                .map(|it| {
                    f.layout_no_wrap(it.label.to_owned(), font.clone(), egui::Color32::WHITE)
                        .size()
                        .x
                        + it.mark.width()
                })
                .fold(0.0, f32::max)
        })
    };
    let widest = widest_at(ui, &font);
    let inner_w = (width - 4.0).max(1.0);
    // ⚠ Shrink before wrapping: at the panel's floor width the five-tab curve selector, the
    // four Retouch modes, the four mask modes and Tint by tone each spilled onto a second row,
    // which both first-use reviews read as a broken control (A-20, P-02). A slightly smaller
    // label that keeps one row beats a second row every time; only when even that does not
    // fit do the items wrap. ⚠ Never below Caption — `shrunk_size`.
    let small = egui::FontId::new(shrunk_size(font.size), font.family.clone());
    let widest_small = widest_at(ui, &small);
    let Plan { cols, shrink } = plan(inner_w, n, widest, widest_small);
    let font = if shrink { small } else { font };
    let rows = n.div_ceil(cols);
    let total = rows as f32 * height + (rows as f32 - 1.0) * GAP;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, total), egui::Sense::hover());
    let t = theme::tokens::of(ui);
    let painter = ui.painter_at(rect);
    // ⚠ A pill well on the canvas colour, the lit cell raised out of it — `docs/DESIGN.md` §7.2.
    // Wrapped rows keep the radius of one row, so a 2 × 2 block reads as the same control.
    painter.rect_filled(rect, height / 2.0, t.canvas);
    let inner = rect.shrink(2.0);
    let w = inner.width() / cols as f32;
    let row_h = (inner.height() - (rows as f32 - 1.0) * GAP) / rows as f32;
    let cell_radius = row_h / 2.0;
    let mut all = resp.clone();
    for (i, it) in items.iter().enumerate() {
        let (item, label, hover) = (&it.value, &it.label, &it.hover);
        let (col, row) = (i % cols, i / cols);
        let cell = egui::Rect::from_min_size(
            egui::pos2(
                inner.left() + w * col as f32,
                inner.top() + (row_h + GAP) * row as f32,
            ),
            egui::vec2(w, row_h),
        );
        let r = ui.interact(cell, resp.id.with(i), egui::Sense::click());
        let r = if hover.is_empty() { r } else { r.hover(*hover) };
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
            lit_cell(&painter, cell, cell_radius, t.raised);
        } else if r.hovered() {
            painter.rect_filled(cell, cell_radius, t.hover);
        }
        let colour = if lit { t.text } else { t.text2 };
        // The mark and the label centred together as one group.
        let galley = painter.layout_no_wrap(label.to_string(), font.clone(), colour);
        let group = it.mark.width() + galley.size().x;
        let left = cell.center().x - group / 2.0;
        match it.mark {
            Mark::None => {}
            Mark::Icon(glyph) => {
                painter.text(
                    egui::pos2(left + theme::tokens::ICON / 2.0, cell.center().y),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    egui::FontId::proportional(theme::tokens::ICON),
                    colour,
                );
            }
            Mark::Dot(c) => {
                painter.circle_filled(
                    egui::pos2(left + MARK_DOT / 2.0, cell.center().y),
                    MARK_DOT / 2.0,
                    c,
                );
            }
        }
        painter.galley(
            egui::pos2(
                left + it.mark.width(),
                cell.center().y - galley.size().y / 2.0,
            ),
            galley,
            colour,
        );
        all = all.union(r);
    }
    all
}

/// The lit segment: the fill and a one-point shadow under it, so it reads as raised out of the
/// well rather than painted on it.
fn lit_cell(painter: &egui::Painter, cell: egui::Rect, radius: f32, fill: egui::Color32) {
    painter.rect_filled(
        cell.translate(egui::vec2(0.0, 1.0)),
        radius,
        egui::Color32::from_black_alpha(70),
    );
    painter.rect_filled(cell, radius, fill);
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
    /// Draw the glyphs alone — the narrow form a bar plan picks. The labels stay the accessible
    /// names, and the hovers still say them.
    pub icons_only: bool,
}

/// The inner padding of a pill cell, either side of its content.
const PILL_PAD: f32 = 10.0;
/// The space between a pill cell's glyph and its label.
const PILL_GAP: f32 = 6.0;

/// The width of one pill cell: the widest glyph, and the widest label unless `icons_only`.
fn pill_cell_width<T>(ui: &egui::Ui, items: &[PillItem<'_, T>], style: &PillStyle) -> f32 {
    let icon_font = egui::FontId::proportional(crate::theme::tokens::ICON);
    let widest = |f: &dyn Fn(&PillItem<'_, T>) -> (String, egui::FontId)| {
        items
            .iter()
            .map(|it| {
                let (t, font) = f(it);
                ui.painter()
                    .layout_no_wrap(t, font, egui::Color32::PLACEHOLDER)
                    .size()
                    .x
            })
            .fold(0.0, f32::max)
    };
    let icon_w = widest(&|it| (it.icon.to_string(), icon_font.clone()));
    if style.icons_only {
        return (icon_w + PILL_PAD * 2.0).ceil();
    }
    let label_w = widest(&|it| (it.label.to_owned(), style.font.clone()));
    (icon_w + PILL_GAP + label_w + PILL_PAD * 2.0).ceil()
}

/// How wide [`pill`] draws `items` in `style` — what a bar plan measures before drawing.
pub fn pill_width<T>(ui: &egui::Ui, items: &[PillItem<'_, T>], style: &PillStyle) -> f32 {
    pill_cell_width(ui, items, style) * items.len().max(1) as f32 + 4.0
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
    let icon_font = egui::FontId::proportional(crate::theme::tokens::ICON);
    let cell_w = pill_cell_width(ui, items, &style);
    let n = items.len().max(1) as f32;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(cell_w * n + 4.0, style.height),
        egui::Sense::hover(),
    );
    let v = ui.visuals().clone();
    let t = theme::tokens::of(ui);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, style.height / 2.0, t.canvas);
    let inner = rect.shrink(2.0);
    let radius = inner.height() / 2.0;
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
            r.hover(it.hover)
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
            painter.rect_filled(cell, radius, v.selection.bg_fill);
            // ⚠ `on_accent`, never white: white on the dark theme's steel blue is 2.4:1.
            theme::on_accent(v.dark_mode)
        } else if lit {
            lit_cell(&painter, cell, radius, t.raised);
            t.text
        } else if r.hovered() {
            painter.rect_filled(cell, radius, t.hover);
            t.text
        } else {
            t.text2
        };
        if style.icons_only {
            painter.text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                it.icon,
                icon_font.clone(),
                colour,
            );
        } else {
            let icon_rect = painter.text(
                egui::pos2(cell.left() + PILL_PAD, cell.center().y),
                egui::Align2::LEFT_CENTER,
                it.icon,
                icon_font.clone(),
                colour,
            );
            painter.text(
                egui::pos2(icon_rect.right() + PILL_GAP, cell.center().y),
                egui::Align2::LEFT_CENTER,
                it.label,
                style.font.clone(),
                colour,
            );
        }
        all = all.union(r);
    }
    all
}

/// The dot's radius in a [`chip`].
const DOT: f32 = 3.5;

/// How wide [`chip`] draws `label` — what the status row's plan measures before drawing.
pub fn chip_width(ui: &egui::Ui, label: &str, font: &egui::FontId) -> f32 {
    let w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
        .size()
        .x;
    w + DOT * 2.0 + 18.0
}

/// A chip: a dot and a label in a rounded well, toggling `on`. The dot is the accent while on
/// and dim while off, so the state is visible without reading.
///
/// ⚠ Reports as a checkbox: it *is* one, drawn as a chip — the format filter's semantics did
/// not change with its clothes.
pub fn chip(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    const H: f32 = 20.0;
    let font = egui::FontId::proportional(crate::theme::tokens::CAPTION);
    let width = chip_width(ui, label, &font);
    let (rect, mut r) = ui.allocate_exact_size(egui::vec2(width, H), egui::Sense::click());
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

/// One swatch of [`swatches`]: its value, its colour, its name and its hover.
pub struct Swatch<'a, T> {
    pub value: T,
    pub colour: egui::Color32,
    pub label: &'a str,
    pub hover: &'a str,
}

/// A row of colour swatches, one lit — the accent choice in Settings (`docs/DESIGN.md` §4.2).
///
/// ⚠ A 22-pt circle each, the chosen one ringed in its own colour with a gap of the panel colour,
/// so the ring reads on every accent. Each reports `WidgetInfo::selected` with its **name**, so
/// `test_ui::click(ctx, "Orange", ..)` reaches it and a screen reader says the colour, not "button".
/// ⚠ `mark_changed`, the lesson `chip` paid for: a hand-drawn widget has to say it changed.
pub fn swatches<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[Swatch<'_, T>],
) -> egui::Response {
    const D: f32 = 22.0;
    const GAP: f32 = 8.0;
    let n = items.len().max(1) as f32;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(n * D + (n - 1.0) * GAP + 6.0, D + 6.0),
        egui::Sense::hover(),
    );
    let panel = ui.visuals().panel_fill;
    let enabled = ui.is_enabled();
    let mut all = resp.clone();
    for (i, it) in items.iter().enumerate() {
        let c = egui::pos2(
            rect.left() + 3.0 + D / 2.0 + i as f32 * (D + GAP),
            rect.center().y,
        );
        let cell = egui::Rect::from_center_size(c, egui::vec2(D + 6.0, D + 6.0));
        let mut r = ui.interact(cell, resp.id.with(i), egui::Sense::click());
        let selected = *current == it.value;
        r.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, enabled, selected, it.label)
        });
        if enabled && r.clicked() && !selected {
            *current = it.value;
            r.mark_changed();
        }
        let r = if it.hover.is_empty() {
            r.hover(it.label)
        } else {
            r.hover(it.hover)
        };
        let p = ui.painter();
        if *current == it.value {
            p.circle_filled(c, D / 2.0 + 3.0, it.colour);
            p.circle_filled(c, D / 2.0 + 1.0, panel);
        }
        p.circle_filled(c, D / 2.0 - 1.0, it.colour);
        all = all.union(r);
    }
    all
}

/// A chunky sliding switch: a track with a knob that moves left to right, filled with the **accent**
/// when on.
///
/// ⚠ For a **consequential** on/off, not for an ordinary preference. A tick box reads as one setting
/// among many, and enabling or disabling a whole integration is not that (user's call, 2026-09-24).
///
/// ⚠⚠ **`mark_changed()` is required**, and this is the lesson `chip` paid for: a hand-drawn widget
/// that does not say it changed leaves every `changed()` reader seeing nothing, and the control reads
/// as "not clickable".
///
/// ⚠⚠ **The `enabled` gate is required too.** A hand-drawn widget gets no greying and no click gate
/// from `add_enabled_ui` for free.
///
/// ⚠ `WidgetInfo::selected` with the label, so the headless harness reaches it by label exactly as it
/// reaches a checkbox — `test_ui::click(ctx, "Enable Immich", …)` works.
pub fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    const H: f32 = 22.0;
    const W: f32 = 40.0;
    const PAD: f32 = 2.0;
    /// ⚠ Larger than `section_font`'s 11 pt on purpose (user's call, 2026-09-24): this switch marks a
    /// consequential decision, and a label at the same size as every section header read as one more
    /// small control. An explicit size rather than a resolved `TextStyle`, so there is no style to
    /// miss in a bare harness `Context` — the panic `section_font`'s own doc warns about.
    const LABEL_PT: f32 = crate::theme::tokens::TITLE;
    let gap = ui.spacing().item_spacing.x;
    let font = egui::FontId::proportional(LABEL_PT);
    let text_w = ui.fonts_mut(|f| {
        f.layout_no_wrap(label.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    });
    let (rect, mut r) =
        ui.allocate_exact_size(egui::vec2(W + gap + text_w, H), egui::Sense::click());
    let enabled = ui.is_enabled();
    let was = *on;
    r.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, was, label));
    if enabled && r.clicked() {
        *on = !*on;
        r.mark_changed();
    }

    let v = ui.visuals().clone();
    let track = egui::Rect::from_min_size(rect.min, egui::vec2(W, H));
    let painter = ui.painter_at(rect);

    // ⚠ Animated by egui's own `animate_bool`, keyed on the response id, so the knob slides rather
    // than jumping — which is the whole point of a switch over a tick.
    let t = ui.ctx().animate_bool(r.id, *on);
    let fill = if *on {
        // ⚠ `selection.bg_fill` **is** the accent: the theme sets it from `Accent`, and every other
        // hand-drawn control here reads it the same way rather than reaching for a private helper.
        v.selection.bg_fill
    } else {
        v.widgets.inactive.bg_fill
    };
    let fill = if enabled {
        fill
    } else {
        // ⚠ Hand-drawn, so the greying is ours to do.
        fill.gamma_multiply(0.4)
    };
    painter.rect_filled(track, H / 2.0, fill);
    if r.hovered() && enabled {
        painter.rect_stroke(
            track,
            H / 2.0,
            v.widgets.hovered.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }

    let kr = H / 2.0 - PAD;
    let cx = track.left() + PAD + kr + t * (W - 2.0 * (PAD + kr));
    painter.circle_filled(
        egui::pos2(cx, track.center().y),
        kr,
        if enabled {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_gray(0x88)
        },
    );

    let text_color = if enabled {
        v.widgets.active.fg_stroke.color
    } else {
        v.widgets.noninteractive.fg_stroke.color
    };
    painter.text(
        egui::pos2(track.right() + gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        text_color,
    );
    r
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    /// Wide enough: one row at full size. Narrow: one row at the shrunken size. Narrower still:
    /// it wraps at full size — never a wrapped *and* shrunken control.
    #[test]
    fn labels_shrink_before_they_wrap() {
        assert_eq!(
            plan(400.0, 5, 60.0, 50.0),
            Plan {
                cols: 5,
                shrink: false
            }
        );
        assert_eq!(
            plan(300.0, 5, 60.0, 50.0),
            Plan {
                cols: 5,
                shrink: true
            }
        );
        let wrapped = plan(200.0, 5, 60.0, 50.0);
        assert!(!wrapped.shrink);
        assert!(wrapped.cols < 5 && wrapped.cols >= 1, "{wrapped:?}");
    }

    /// Four that would fit three to a row are drawn two and two — a 3 + 1 block reads as a row
    /// with a straggler. Five that fit three go three and two.
    #[test]
    fn a_wrapped_control_is_balanced() {
        // 300 fits three of 90 (+10) but not four.
        assert_eq!(plan(300.0, 4, 90.0, 90.0).cols, 2);
        assert_eq!(plan(300.0, 5, 90.0, 90.0).cols, 3);
        // ⚠ The positive control: a row that fits stays one row.
        assert_eq!(plan(500.0, 4, 90.0, 90.0).cols, 4);
    }

    /// The shrink stops at Caption — `docs/DESIGN.md` §3's floor. ⚠ Both halves: a Body label
    /// still shrinks, a Caption one does not.
    #[test]
    fn a_label_never_shrinks_below_caption() {
        let caption = crate::theme::tokens::CAPTION;
        let body = crate::theme::tokens::BODY;
        assert!(shrunk_size(body) < body, "a body label must still shrink");
        assert!(shrunk_size(body) >= caption);
        assert_eq!(
            shrunk_size(caption),
            caption,
            "a caption label shrank below the floor"
        );
    }

    #[test]
    fn a_single_item_never_wraps() {
        assert_eq!(
            plan(10.0, 1, 60.0, 50.0),
            Plan {
                cols: 1,
                shrink: false
            }
        );
    }
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
            icons_only: false,
        }
    }

    #[test]
    fn the_accent_swatches_select_by_name() {
        // ⚠ Reached by the colour's name, the way a person and a screen reader refer to it — and the
        // click must actually change the value, which a hand-drawn widget does only if told to.
        #[derive(Clone, Copy, PartialEq, Debug)]
        enum A {
            Blue,
            Orange,
        }
        let items = [
            Swatch {
                value: A::Blue,
                colour: egui::Color32::LIGHT_BLUE,
                label: "Steel blue",
                hover: "",
            },
            Swatch {
                value: A::Orange,
                colour: egui::Color32::ORANGE,
                label: "Orange",
                hover: "",
            },
        ];
        let ctx = crate::test_ui::ctx();
        let mut cur = A::Blue;
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            swatches(ui, &mut cur, &items);
        });
        assert!(found.iter().any(|(l, _)| l == "Orange"), "{found:?}");
        crate::test_ui::click(&ctx, "Orange", |ui| {
            swatches(ui, &mut cur, &items);
        });
        assert_eq!(cur, A::Orange);
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

    /// ⚠⚠ **The two things a hand-drawn toggle gets wrong**, both of which `chip` shipped once: a
    /// click that does not flip it, and a flip that never reports `changed()` — which makes every
    /// caller read it as "not clickable".
    #[test]
    fn a_switch_flips_and_reports_the_change() {
        let ctx = crate::test_ui::ctx();
        let mut on = false;
        let mut changed = 0;
        crate::test_ui::click(&ctx, "Enable Immich", |ui| {
            if switch(ui, &mut on, "Enable Immich").changed() {
                changed += 1;
            }
        });
        assert!(on, "the click must flip it");
        assert_eq!(changed, 1, "it must report the change exactly once");
    }

    /// ⚠ And a disabled switch neither flips nor reports — the gate a hand-drawn widget has to do
    /// for itself, since `add_enabled_ui` cannot do it.
    #[test]
    fn a_disabled_switch_does_not_flip() {
        let ctx = crate::test_ui::ctx();
        let mut on = false;
        let mut changed = 0;
        crate::test_ui::click(&ctx, "Enable Immich", |ui| {
            ui.add_enabled_ui(false, |ui| {
                if switch(ui, &mut on, "Enable Immich").changed() {
                    changed += 1;
                }
            });
        });
        assert!(!on, "a disabled switch must not flip");
        assert_eq!(changed, 0);
    }
}
