//! The Library panel's row: a name, a count at the right edge, the accent tint on the open one,
//! and its actions as glyph cells left of the count.
//!
//! ⚠⚠ **The count ends at the same x on every row, and a header's count ends there too**
//! (`tokens::TRAILING_INSET`). The actions used to sit at the edge with the count left of them,
//! so a folder's count (one action) and an Immich album's (two) and a section header's (none)
//! each stood in a different column — "all the numbers should be to the most right, with the
//! buttons to the left of them" (the author, 2026-09-29). `every_count_in_the_rail_ends_at_one_x`.
//!
//! The Masks list's shape (`mask_components::row`) for the Library's lists — collections,
//! keywords, folders, smart collections — without the mask-specific glyph column, and with
//! any number of actions rather than a fixed eye and hold-out. ⚠ Its own painter rather than
//! a fourth `RowGlyph` variant: the two rows share a look, not a contract, and every action
//! here is a momentary button while the mask row's are toggles.

use eframe::egui;
use crate::theme::Hover as _;

/// Row height in points — the Masks list's, `docs/DESIGN.md` §5.
pub const ROW_HEIGHT: f32 = crate::theme::tokens::LIST_ROW;
/// How much of the accent the open row's tint carries — the rail's value.
const TINT: f32 = crate::theme::tokens::TINT;
/// The room between the leftmost-reaching count and the action cell beside it. The cells carry
/// their own padding around the glyph, so this only keeps a wide count off a cell's hit area.
const COUNT_GAP: f32 = 2.0;

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

/// A tree row's disclosure gutter, left of its name: the triangle's width.
pub const DISCLOSURE: f32 = 12.0;
/// How far one tree level indents — folders, dates, the Immich timeline.
pub const INDENT: f32 = 12.0;

/// The indent a tree row at `depth` passes as [`RowSpec::indent`], leaving the gutter for its
/// [`disclosure`] triangle.
pub fn tree_indent(depth: usize) -> f32 {
    depth as f32 * INDENT + DISCLOSURE
}

/// A disclosure triangle in `row`'s gutter, just left of a name drawn at `indent`; returns
/// whether it was clicked. Call it **after** the row, so it wins the click there — the row's own
/// actions work the same way.
///
/// ⚠⚠ **PAINTED, not a glyph.** The icon font is Phosphor with a version-3 `post` table — no
/// glyph names — so a caret codepoint could only be taken from its stylesheet on trust, and this
/// project has shipped glyphs that rendered as empty boxes (`icons.rs` records it). A painted
/// triangle cannot be missing from a font. Moved here from `folder_rows` when the Dates and Immich
/// timeline trees became list rows (2026-09-29).
pub fn disclosure(
    ui: &mut egui::Ui,
    row: egui::Rect,
    indent: f32,
    open: bool,
    id: egui::Id,
) -> bool {
    let centre = egui::pos2(row.left() + indent - DISCLOSURE / 2.0, row.center().y);
    let hit = ui.interact(
        egui::Rect::from_center_size(centre, egui::vec2(DISCLOSURE, row.height())),
        id,
        egui::Sense::click(),
    );
    let colour = ui.visuals().text_color();
    let s = 3.5;
    let pts = if open {
        vec![
            centre + egui::vec2(-s, -s * 0.6),
            centre + egui::vec2(s, -s * 0.6),
            centre + egui::vec2(0.0, s * 0.8),
        ]
    } else {
        vec![
            centre + egui::vec2(-s * 0.6, -s),
            centre + egui::vec2(s * 0.8, 0.0),
            centre + egui::vec2(-s * 0.6, s),
        ]
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(pts, colour, egui::Stroke::NONE));
    hit.clicked()
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
        resp.hover(spec.hover)
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
    // ⚠ The count is decoration on a closed row and information on the open one — §7.6.
    let tk = crate::theme::tokens::of(ui);
    let count_colour = if spec.selected { tk.text2 } else { tk.text3 };
    let caption = egui::FontId::proportional(crate::theme::tokens::CAPTION);
    let right_text = spec
        .count
        .map(|n| n.to_string())
        .or_else(|| (!spec.meta.is_empty()).then(|| spec.meta.to_string()));
    let count_galley = right_text.map(|s| {
        ui.painter()
            .layout_no_wrap(s, caption.clone(), count_colour)
    });
    // The count column: right-aligned at the inset, at least four digits wide so the action cells
    // left of it stand at one x on every row below 10,000. A meta text (a snapshot's time) takes
    // its own width.
    let column = count_galley.as_ref().map(|g| {
        if spec.count.is_some() {
            let four = ui
                .painter()
                .layout_no_wrap("0000".to_owned(), caption.clone(), count_colour)
                .size()
                .x;
            g.size().x.max(four)
        } else {
            g.size().x
        }
    });
    let mut action = None;
    // The actions fill in leftwards from the count column, or from the edge on a row without one.
    let mut right = match column {
        Some(w) => rect.right() - crate::theme::tokens::TRAILING_INSET - w - COUNT_GAP,
        None => rect.right(),
    };
    let mut cells: Vec<egui::Rect> = Vec::with_capacity(spec.actions.len());
    let mut colours: Vec<egui::Color32> = Vec::with_capacity(spec.actions.len());
    // The cell the pointer is on, if any — it alone gets the button face.
    let mut pointed: Option<usize> = None;
    // ⚠⚠ **Always drawn, greyed, and brought up by the row's hover** — the author's call
    // (2026-09-29), reversing P-13, which had drawn them only under the pointer because "a × on
    // every folder row at all times read as a standing threat". `text3` at rest (the counts' grey),
    // `text` with the pointer anywhere on the row, and the `raised` face under the pointed cell.
    let pointer_here = ui.rect_contains_pointer(rect);
    for (i, a) in spec.actions.iter().enumerate() {
        // ⚠ Narrower than the row is tall (`ROW_ACTION`), and as tall: "make the layout of all
        // buttons tighter" (2026-09-29).
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - crate::theme::tokens::ROW_ACTION, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let name = format!("{} {}", a.verb, spec.name);
        // ⚠⚠ A plain `interact`, never a child scope. The first version wrapped this in
        // `scope_builder(..max_rect(cell))` to get `add_enabled_ui`'s greying — and a scope
        // that places nothing has an empty `min_rect`, which the parent then advanced its
        // cursor after, so every row after the first landed on top of it ("the boxes and
        // names on the left are overlapping", seen in use 2026-09-20). The enabled flag goes
        // straight into the widget info instead.
        let on = enabled && a.enabled;
        // ⚠ A disabled action senses the pointer but never a click, so a click on it reaches the
        // row. The keyword row's Tag and Untag are disabled until photos are picked, and once the
        // count moved to the edge (2026-09-29) they stood under the middle of a short row, where a
        // click meant to open the keyword was silently eaten.
        let sense = if on {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let r = ui.interact(cell, resp.id.with(i), sense);
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, on, &name));
        let r = if a.hover.is_empty() {
            r
        } else {
            r.hover(a.hover)
        };
        if a.enabled && r.clicked() {
            action = Some(i);
        }
        if on && r.hovered() {
            pointed = Some(i);
        }
        // ⚠ A disabled action neither pops nor takes the face: it is not the one a click hits.
        colours.push(if !on {
            // ⚠ Kept below the resting enabled glyph, so the two stay apart at rest too.
            tk.text3
                .gamma_multiply(crate::theme::tokens::ACTION_REST * 0.6)
        } else if pointer_here {
            tk.text
        } else {
            tk.text3.gamma_multiply(crate::theme::tokens::ACTION_REST)
        });
        // ⚠ Advance, or the next cell lands on this one's hit area and wins the tie.
        right = cell.left();
        cells.push(cell);
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
            painter.rect_filled(rect, crate::theme::tokens::RADIUS, fill);
        }
        let text = if spec.selected {
            v.strong_text_color()
        } else {
            v.widgets.inactive.fg_stroke.color
        };
        let font = egui::TextStyle::Body.resolve(ui.style());
        // The name is clipped before the actions and the count, never over them — the actions'
        // room is kept even while they are hidden, so a name does not shorten under the pointer.
        let name_right = right - 6.0;
        // ⚠ Elided with "…", not clipped mid-glyph: "immich (downloaded" and "concert/Krinitsi"
        // were what a clip left of two folder names (P-13). The full name is the row's hover.
        let name_left = rect.left() + spec.indent + 8.0;
        let mut job = egui::text::LayoutJob::simple_singleline(spec.name.to_owned(), font, text);
        job.wrap.max_width = (name_right - name_left).max(0.0);
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
                egui::pos2(
                    rect.right() - crate::theme::tokens::TRAILING_INSET - g.size().x,
                    rect.center().y - g.size().y / 2.0,
                ),
                g,
                count_colour,
            );
        }
        for (i, ((a, colour), cell)) in spec.actions.iter().zip(colours).zip(&cells).enumerate() {
            if pointed == Some(i) {
                painter.rect_filled(*cell, crate::theme::tokens::RADIUS, tk.raised);
            }
            painter.text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                a.glyph,
                egui::FontId::proportional(crate::theme::tokens::ICON),
                colour,
            );
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

    /// The action glyphs painted in one frame, with their colours, and the theme's tokens —
    /// `pointer` places the pointer (on the row's name, away from its cells) or leaves it off-screen.
    fn painted_glyphs(
        ctx: &egui::Context,
        pointer: Option<egui::Pos2>,
    ) -> (Vec<(String, egui::Color32)>, crate::theme::tokens::Tokens) {
        let mut tk = None;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                events: pointer
                    .map(|p| vec![egui::Event::PointerMoved(p)])
                    .unwrap_or_default(),
                ..Default::default()
            },
            |ui| {
                tk = Some(*crate::theme::tokens::of(ui));
                draw(ui, &mut (false, None));
            },
        );
        fn walk(shape: &egui::Shape, found: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, found)),
                egui::Shape::Text(t) => {
                    let text = t.galley.text();
                    if text == "×" || text == "+" {
                        let colour = t
                            .galley
                            .job
                            .sections
                            .first()
                            .map_or(t.fallback_color, |s| s.format.color);
                        found.push((text.to_owned(), colour));
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for c in &out.shapes {
            walk(&c.shape, &mut found);
        }
        (found, tk.expect("drawn"))
    }

    /// ⚠ The author's call (2026-09-29), reversing P-13: the buttons are always there, greyed.
    #[test]
    fn the_buttons_are_drawn_with_no_pointer_on_the_row() {
        let (glyphs, tk) = painted_glyphs(&test_ui::ctx(), None);
        assert_eq!(glyphs.len(), 2, "both glyphs, at rest: {glyphs:?}");
        let rest = tk.text3.gamma_multiply(crate::theme::tokens::ACTION_REST);
        assert!(glyphs.iter().all(|(_, c)| *c == rest), "{glyphs:?}");
    }

    /// The pop: `text3` at rest, `text` with the pointer anywhere on the row.
    #[test]
    fn the_buttons_pop_when_the_pointer_is_on_the_row() {
        let ctx = test_ui::ctx();
        let (rest, tk) = painted_glyphs(&ctx, None);
        // ⚠ Proven different before anything is asserted about which is which (lesson (c)).
        let faint = tk.text3.gamma_multiply(crate::theme::tokens::ACTION_REST);
        assert_ne!(faint, tk.text, "the two states must be told apart");
        let name = test_ui::labelled_centres(&ctx, Vec::new(), |ui| draw(ui, &mut (false, None)))
            .into_iter()
            .find(|(l, _)| l == "Portraits")
            .map(|(_, p)| p)
            .expect("the row is named");
        // The row's own centre is on its name, well left of the cells at the right edge.
        let (hot, _) = painted_glyphs(&ctx, Some(name));
        assert!(rest.iter().all(|(_, c)| *c == faint), "rest: {rest:?}");
        assert_eq!(hot.len(), 2, "{hot:?}");
        assert!(hot.iter().all(|(_, c)| *c == tk.text), "hovered: {hot:?}");
    }

    /// How many cell faces (`raised` fills) one frame paints, with the pointer at `pointer`.
    fn faces(ctx: &egui::Context, pointer: egui::Pos2) -> usize {
        let mut raised = None;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                events: vec![egui::Event::PointerMoved(pointer)],
                ..Default::default()
            },
            |ui| {
                raised = Some(crate::theme::tokens::of(ui).raised);
                draw(ui, &mut (false, None));
            },
        );
        let raised = raised.expect("drawn");
        fn walk(shape: &egui::Shape, raised: egui::Color32, n: &mut usize) {
            match shape {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, raised, n)),
                egui::Shape::Rect(r) if r.fill == raised => *n += 1,
                _ => {}
            }
        }
        let mut n = 0;
        for c in &out.shapes {
            walk(&c.shape, raised, &mut n);
        }
        n
    }

    /// The pointed button alone gets a face — both halves, so a face painted everywhere fails too.
    #[test]
    fn the_pointed_button_gets_a_face() {
        let ctx = test_ui::ctx();
        let all = test_ui::labelled_centres(&ctx, Vec::new(), |ui| draw(ui, &mut (false, None)));
        let at = |label: &str| {
            all.iter()
                .find(|(l, _)| l == label)
                .map(|(_, p)| *p)
                .unwrap_or_else(|| panic!("no {label:?}: {all:?}"))
        };
        assert_eq!(faces(&ctx, at("Portraits")), 0, "on the name, no face");
        assert_eq!(
            faces(&ctx, at("Delete Portraits")),
            1,
            "on a button, its face"
        );
    }

    /// Tighter: neighbouring cells stand `ROW_ACTION` apart, not a row height.
    #[test]
    fn the_buttons_stand_a_row_action_apart() {
        let ctx = test_ui::ctx();
        let all = test_ui::labelled_centres(&ctx, Vec::new(), |ui| draw(ui, &mut (false, None)));
        let x = |label: &str| {
            all.iter()
                .find(|(l, _)| l == label)
                .map(|(_, p)| p.x)
                .unwrap_or_else(|| panic!("no {label:?}: {all:?}"))
        };
        let gap = x("Delete Portraits") - x("Add to Portraits");
        assert!(
            (gap - crate::theme::tokens::ROW_ACTION).abs() < 0.5,
            "cells {gap} apart, want {}",
            crate::theme::tokens::ROW_ACTION
        );
    }

    /// ⚠ A disabled action must not eat a click meant for its row — the keyword row's Tag and Untag
    /// are disabled until photos are picked, and stood under the middle of a short row once the
    /// count moved to the edge (2026-09-29). ⚠ Both halves: the same cell enabled is still reached.
    #[test]
    fn a_click_on_a_disabled_action_reaches_the_row() {
        let draw = |ui: &mut egui::Ui, enabled: bool, out: &mut (bool, Option<usize>)| {
            let acts = [RowAction {
                glyph: '+',
                verb: "Tag",
                hover: "",
                enabled,
            }];
            let o = row(
                ui,
                RowSpec {
                    name: "Europe",
                    count: Some(3),
                    meta: "",
                    selected: false,
                    indent: 0.0,
                    hover: "",
                    actions: &acts,
                },
            );
            out.0 |= o.clicked;
            if o.action.is_some() {
                out.1 = o.action;
            }
        };
        for (enabled, row_opens, action) in [(false, true, None), (true, false, Some(0))] {
            let ctx = test_ui::ctx();
            let mut out = (false, None);
            test_ui::click(&ctx, "Tag Europe", |ui| draw(ui, enabled, &mut out));
            assert_eq!(out.0, row_opens, "enabled: {enabled} — did the row open");
            assert_eq!(out.1, action, "enabled: {enabled} — which action fired");
        }
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
