//! The Library panel's row: a name, a count, the accent tint on the open one, and its actions
//! as glyph cells at the right edge.
//!
//! The Masks list's shape (`mask_components::row`) for the Library's lists — collections,
//! keywords, folders, smart collections — without the mask-specific glyph column, and with
//! any number of actions rather than a fixed eye and hold-out. ⚠ Its own painter rather than
//! a fourth `RowGlyph` variant: the two rows share a look, not a contract, and every action
//! here is a momentary button while the mask row's are toggles.

use eframe::egui;

/// Row height in points — the Masks list's.
pub const ROW_HEIGHT: f32 = 26.0;
/// How much of the accent the open row's tint carries — the rail's value.
const TINT: f32 = 0.18;

/// One action cell at the right edge of a row.
pub struct RowAction<'a> {
    /// The painted glyph — `×`, `+`, `−`, or an icon-font character.
    ///
    /// ⚠⚠ **VERIFY THE CODEPOINT AGAINST THE BUNDLED FONTS BEFORE USING IT HERE.** This is drawn with
    /// `FontId::proportional`, whose family is **Ubuntu-Light → NotoEmoji → emoji-icon-font → Phosphor**
    /// (`theme::install_fonts` adds the last at `FontPriority::Lowest`). A codepoint in **Hack** does not
    /// render here — Hack is the *Monospace* family only.
    ///
    /// ⚠ On 2026-09-24 the Immich rail shipped `↓` and `✕` as **grey squares**: `↓` (U+2193) is in Hack
    /// alone, and `✕` (U+2715) is in **no bundled font at all**. `icons.rs` carries the same warning and
    /// records four earlier features paying for it; that one was the fifth, and the reason this note is
    /// *here* is that the mistake is made while writing a `RowAction`, not while reading `icons.rs`.
    ///
    /// ⚠ Known good: `×` U+00D7, `+` U+002B, `−` U+2212 (Ubuntu-Light) and `⬇` U+2B07 (NotoEmoji, which is
    /// monochrome and so takes the painter's colour). Anything else: parse the cmaps first.
    pub glyph: char,
    /// The verb for a screen reader; the cell is named "{verb} {row name}".
    pub verb: &'a str,
    pub hover: &'a str,
    pub enabled: bool,
}

/// One row.
pub struct RowSpec<'a> {
    pub name: &'a str,
    /// Weak text after the name — a count, or nothing.
    pub count: Option<i64>,
    /// Weak text after the name when there is no count — a snapshot's time. Empty for none.
    pub meta: &'a str,
    /// The accent tint: the source the Library is showing.
    pub selected: bool,
    pub indent: f32,
    pub hover: &'a str,
    /// Right to left: the first action lands at the right edge.
    pub actions: &'a [RowAction<'a>],
}

pub struct RowOut {
    /// The row itself was clicked (never when an action was).
    pub clicked: bool,
    /// The row itself was double-clicked — the snapshot strip's rename gesture.
    pub double_clicked: bool,
    /// Which action was clicked, as an index into `actions`.
    pub action: Option<usize>,
    /// Where the row landed, for a caller painting into its gutter (the timeline's dots).
    pub rect: egui::Rect,
    /// The row's own response, for a context menu.
    pub response: egui::Response,
}

/// Draw one row.
///
/// ⚠ The actions are registered **after** the row so they win the hit-test tie — egui takes the
/// last widget registered at a point — and a click on one is not a click on the row.
pub fn row(ui: &mut egui::Ui, spec: RowSpec<'_>) -> RowOut {
    row_with(ui, spec, egui::Sense::click())
}

/// [`row`], also sensing a DRAG — the Keywords list's move gesture (M9.8). `RowOut::response`
/// carries it.
///
/// ⚠⚠ The row itself senses the drag. A drag-only layer registered over a click-only row was
/// tried first and SWALLOWED the row's clicks and double-clicks: egui gave the pointer to the top
/// widget. On one response, egui tells a click from a drag by whether the pointer passed the
/// drag threshold.
pub fn row_draggable(ui: &mut egui::Ui, spec: RowSpec<'_>) -> RowOut {
    row_with(ui, spec, egui::Sense::click_and_drag())
}

fn row_with(ui: &mut egui::Ui, spec: RowSpec<'_>, sense: egui::Sense) -> RowOut {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), sense);
    let resp = if spec.hover.is_empty() {
        resp
    } else {
        resp.on_hover_text(spec.hover)
    };
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            enabled,
            spec.selected,
            spec.name,
        )
    });
    let v = ui.visuals().clone();
    let mut action = None;
    let mut right = rect.right();
    let mut colours: Vec<egui::Color32> = Vec::with_capacity(spec.actions.len());
    // ⚠ The actions exist on every row — a screen reader or a test finds them by name — but
    // they are **drawn only under the pointer**: a "×" on every folder row at all times read as
    // a standing threat (P-13). A click is always under the pointer, so nothing is lost.
    let pointer_here = ui.rect_contains_pointer(rect);
    for (i, a) in spec.actions.iter().enumerate() {
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - ROW_HEIGHT, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let name = format!("{} {}", a.verb, spec.name);
        // ⚠⚠ A plain `interact`, never a child scope. The first version wrapped this in
        // `scope_builder(..max_rect(cell))` to get `add_enabled_ui`'s greying — and a scope
        // that places nothing has an empty `min_rect`, which the parent then advanced its
        // cursor after, so every row after the first landed on top of it ("the boxes and
        // names on the left are overlapping", seen in use 2026-09-20). The enabled flag goes
        // straight into the widget info instead.
        let r = ui.interact(cell, resp.id.with(i), egui::Sense::click());
        let on = enabled && a.enabled;
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, on, &name));
        let r = if a.hover.is_empty() {
            r
        } else {
            r.on_hover_text(a.hover)
        };
        if a.enabled && r.clicked() {
            action = Some(i);
        }
        colours.push(if !a.enabled {
            v.weak_text_color().gamma_multiply(0.45)
        } else if r.hovered() {
            v.strong_text_color()
        } else {
            v.weak_text_color()
        });
        // ⚠ Advance, or the next cell lands on this one's hit area and wins the tie.
        right = cell.left();
    }
    let clicked = resp.clicked() && action.is_none();
    let double_clicked = resp.double_clicked() && action.is_none();

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let fill = if spec.selected {
            v.selection.bg_fill.gamma_multiply(TINT)
        } else if resp.hovered() {
            v.widgets.hovered.weak_bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        if fill != egui::Color32::TRANSPARENT {
            painter.rect_filled(rect, 4.0, fill);
        }
        let text = if spec.selected {
            v.strong_text_color()
        } else {
            v.widgets.inactive.fg_stroke.color
        };
        let font = egui::TextStyle::Body.resolve(ui.style());
        // The name is clipped before the count and the actions, never over them.
        let name_right = right - 6.0;
        let right_text = spec
            .count
            .map(|n| n.to_string())
            .or_else(|| (!spec.meta.is_empty()).then(|| spec.meta.to_string()));
        let count_galley = right_text.map(|s| {
            painter.layout_no_wrap(s, egui::FontId::proportional(11.0), v.weak_text_color())
        });
        let count_w = count_galley
            .as_ref()
            .map(|g| g.size().x + 8.0)
            .unwrap_or(0.0);
        // ⚠ Elided with "…", not clipped mid-glyph: "immich (downloaded" and "concert/Krinitsi"
        // were what a clip left of two folder names (P-13). The full name is the row's hover.
        let name_left = rect.left() + spec.indent + 8.0;
        let mut job = egui::text::LayoutJob::simple_singleline(spec.name.to_owned(), font, text);
        job.wrap.max_width = (name_right - count_w - name_left).max(0.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(
            egui::pos2(name_left, rect.center().y - galley.size().y / 2.0),
            galley,
            text,
        );
        if let Some(g) = count_galley {
            painter.galley(
                egui::pos2(name_right - g.size().x, rect.center().y - g.size().y / 2.0),
                g,
                v.weak_text_color(),
            );
        }
        let mut cell_x = rect.right() - ROW_HEIGHT / 2.0;
        for (a, colour) in spec.actions.iter().zip(colours) {
            if !pointer_here {
                break;
            }
            painter.text(
                egui::pos2(cell_x, rect.center().y),
                egui::Align2::CENTER_CENTER,
                a.glyph,
                egui::FontId::proportional(if a.glyph == '×' { 17.0 } else { 15.0 }),
                colour,
            );
            cell_x -= ROW_HEIGHT;
        }
    }
    RowOut {
        clicked,
        double_clicked,
        action,
        rect,
        response: resp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn actions() -> [RowAction<'static>; 2] {
        [
            RowAction {
                glyph: '×',
                verb: "Delete",
                hover: "",
                enabled: true,
            },
            RowAction {
                glyph: '+',
                verb: "Add to",
                hover: "",
                enabled: true,
            },
        ]
    }

    fn draw(ui: &mut egui::Ui, out: &mut (bool, Option<usize>)) {
        let acts = actions();
        let o = row(
            ui,
            RowSpec {
                name: "Portraits",
                count: Some(21),
                meta: "",
                selected: false,
                indent: 0.0,
                hover: "",
                actions: &acts,
            },
        );
        if o.clicked {
            out.0 = true;
        }
        if o.action.is_some() {
            out.1 = o.action;
        }
    }

    #[test]
    fn the_row_and_its_actions_are_named_and_distinct() {
        let ctx = test_ui::ctx();
        let mut out = (false, None);
        let labels: Vec<String> =
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| draw(ui, &mut out))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        for want in ["Portraits", "Delete Portraits", "Add to Portraits"] {
            assert!(
                labels.iter().any(|l| l == want),
                "{want:?} missing in {labels:?}"
            );
        }
        test_ui::click(&ctx, "Portraits", |ui| draw(ui, &mut out));
        assert!(out.0, "a click on the row did not open it");
        assert_eq!(out.1, None, "a click on the row hit an action");
        let mut out = (false, None);
        test_ui::click(&ctx, "Delete Portraits", |ui| draw(ui, &mut out));
        assert_eq!(out.1, Some(0), "the delete cell was not reached");
        assert!(!out.0, "a click on an action opened the row");
        let mut out = (false, None);
        test_ui::click(&ctx, "Add to Portraits", |ui| draw(ui, &mut out));
        assert_eq!(out.1, Some(1), "the second cell was not reached");
    }

    #[test]
    fn rows_are_laid_out_at_a_constant_pitch() {
        // ⚠ The regression: three rows with action cells must stack, not overlap. The action
        // cells used to be drawn through a child scope whose empty rect moved the cursor.
        let ctx = test_ui::ctx();
        let mut tops = Vec::new();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                ..Default::default()
            },
            |ui| {
                let acts = actions();
                for name in ["One", "Two", "Three"] {
                    let top = ui.cursor().top();
                    row(
                        ui,
                        RowSpec {
                            name,
                            count: Some(1),
                            meta: "",
                            selected: false,
                            indent: 0.0,
                            hover: "",
                            actions: &acts,
                        },
                    );
                    tops.push(top);
                }
            },
        );
        let pitch: Vec<f32> = tops.windows(2).map(|p| p[1] - p[0]).collect();
        assert!(
            pitch.iter().all(|&d| d >= ROW_HEIGHT && d.is_finite()),
            "rows overlap or wander: tops {tops:?}"
        );
        assert_eq!(pitch[0], pitch[1], "the pitch is not constant: {tops:?}");
    }

    #[test]
    fn a_disabled_action_does_nothing() {
        let ctx = test_ui::ctx();
        let mut hit = None;
        let acts = [RowAction {
            glyph: '+',
            verb: "Add to",
            hover: "",
            enabled: false,
        }];
        test_ui::click(&ctx, "Add to Portraits", |ui| {
            let o = row(
                ui,
                RowSpec {
                    name: "Portraits",
                    count: None,
                    meta: "",
                    selected: false,
                    indent: 0.0,
                    hover: "",
                    actions: &acts,
                },
            );
            if o.action.is_some() {
                hit = o.action;
            }
        });
        assert_eq!(hit, None);
    }
}
