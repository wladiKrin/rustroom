//! The panel's slider row: label │ rail │ value, on one 16 pt line.
//!
//! ⚠⚠ **Why not `egui::Slider`.** Its rail is a fixed `spacing.slider_width` (100 pt), so a
//! third of every 240 pt row was the value box and the label. And its trailing fill starts at
//! the **left edge**, so a bipolar slider at 0 — Contrast, Tint, every HSL band — showed half a
//! bar of accent for "nothing applied", the opposite of the signal the accent was chosen to
//! carry. Here the fill runs from zero and the rail takes what the label and value leave.
//!
//! ⚠⚠ **The behaviour matches egui's on purpose.** Four headless harnesses locate a slider by
//! its AccessKit label, press at its centre and drag right; `Edited::slider` records
//! `changed()` and `drag_stopped()` separately for one history entry per settled drag. So the
//! rail carries `WidgetInfo::slider` with the label, the value is set from the pointer's
//! **position** (through `smart_aim`, so a drag lands on 0.69 rather than 0.6873), arrows step
//! it while focused, and the returned response is the union of rail and value exactly as
//! `egui::Slider::add_contents` builds it.
//!
//! ⚠ Not built, deliberately: double-click to reset, a precision modifier, vertical rails,
//! `step`, clamping modes. Only what the panel's call sites use.

use eframe::egui;
use rustroom_app::slider_map;
use std::ops::RangeInclusive;

/// The label column. The widest labels today ("Radius (px)", "Smoothness") fit; a wider one
/// truncates and shows in full on hover.
pub const LABEL_WIDTH: f32 = 78.0;
/// The value column's minimum width, right-aligned. A wider value widens its own cell — see
/// `value_w` in the widget.
pub const VALUE_WIDTH: f32 = 44.0;
const RAIL_HEIGHT: f32 = 3.0;
const HANDLE_RADIUS: f32 = 5.0;
const GAP: f32 = 8.0;
/// One arrow press moves the handle this fraction of the rail.
const KEY_STEP: f32 = 0.01;

type Formatter<'a> = Box<dyn Fn(f64) -> String + 'a>;
type Parser<'a> = Box<dyn Fn(&str) -> Option<f64> + 'a>;

pub struct Slider<'a> {
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    label: &'a str,
    log: bool,
    formatter: Option<Formatter<'a>>,
    parser: Option<Parser<'a>>,
}

impl<'a> Slider<'a> {
    pub fn new(value: &'a mut f32, range: RangeInclusive<f32>) -> Self {
        Self {
            value,
            range,
            label: "",
            log: false,
            formatter: None,
            parser: None,
        }
    }

    pub fn text(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }

    /// A log rail, for a range whose useful zone sits at the bottom (the retouch Size).
    /// Falls back to linear when an end is not strictly positive.
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.log = on;
        self
    }

    pub fn custom_formatter(mut self, f: impl Fn(f64) -> String + 'a) -> Self {
        self.formatter = Some(Box::new(f));
        self
    }

    pub fn custom_parser(mut self, f: impl Fn(&str) -> Option<f64> + 'a) -> Self {
        self.parser = Some(Box::new(f));
        self
    }
}

impl egui::Widget for Slider<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let Self {
            value,
            range,
            label,
            log,
            formatter,
            parser,
        } = self;
        let (min, max) = (*range.start(), *range.end());
        let height = ui.spacing().interact_size.y;
        let old = *value;
        // ⚠ **The value cell is sized from the text it will show, never a fixed width.** At a
        // fixed 44 pt a value wider than the cell — "12500 K" — would draw a `DragValue` past
        // it, the row would exceed the panel, and egui would widen the panel to fit, again every
        // frame: the feedback loop the 2026-09-19 growth hunt found elsewhere (a nested
        // sub-label row, see `theme::sub_label_with`) and this row was hardened against while
        // suspected. The rail takes what is left, so a wide value costs rail, not panel.
        // Measured with the `DragValue`'s own text style and button padding; `VALUE_WIDTH` is
        // the floor so the columns stay aligned across ordinary rows.
        let value_w = {
            let text = match &formatter {
                Some(f) => f(*value as f64),
                None => egui::emath::format_with_decimals_in_range(*value as f64, 0..=2),
            };
            let font = ui.style().drag_value_text_style.resolve(ui.style());
            let text_w = ui
                .fonts_mut(|f| f.layout_no_wrap(text, font, egui::Color32::WHITE))
                .size()
                .x;
            let padded = text_w + 2.0 * ui.spacing().button_padding.x;
            VALUE_WIDTH.max(padded.ceil())
        };
        let bipolar = slider_map::straddles_zero(min, max);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;

            // ── label ──
            let (label_rect, label_resp) =
                ui.allocate_exact_size(egui::vec2(LABEL_WIDTH, height), egui::Sense::hover());
            let font = egui::TextStyle::Body.resolve(ui.style());
            let colour = ui.visuals().text_color();
            let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, colour);
            job.wrap.max_width = LABEL_WIDTH;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let elided = galley.elided;
            ui.painter().galley(
                egui::pos2(
                    label_rect.left(),
                    label_rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                colour,
            );
            if elided {
                label_resp.on_hover_text(label);
            }

            // ── rail ──
            let rail_w = (ui.available_width() - value_w - GAP).max(24.0);
            let (rect, mut resp) =
                ui.allocate_exact_size(egui::vec2(rail_w, height), egui::Sense::click_and_drag());
            let x_range =
                egui::Rangef::new(rect.left() + HANDLE_RADIUS, rect.right() - HANDLE_RADIUS);
            if let Some(p) = resp.interact_pointer_pos() {
                // Position-based, through smart aim, exactly as egui's own slider does it.
                let aim = ui.input(|i| i.aim_radius());
                let at = |x: f32| {
                    slider_map::value_of(egui::remap_clamp(x, x_range, 0.0..=1.0), min, max, log)
                        as f64
                };
                let (a, b) = (at(p.x - aim), at(p.x + aim));
                *value = egui::emath::smart_aim::best_in_range_f64(a.min(b), a.max(b)) as f32;
            }
            if resp.clicked() {
                resp.request_focus();
            }
            if resp.has_focus() {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        resp.id,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            ..Default::default()
                        },
                    );
                });
                let steps = ui.input(|i| {
                    i.num_presses(egui::Key::ArrowRight) as i32
                        - i.num_presses(egui::Key::ArrowLeft) as i32
                });
                if steps != 0 {
                    let t = slider_map::t_of(*value, min, max, log) + steps as f32 * KEY_STEP;
                    *value = slider_map::value_of(t, min, max, log);
                }
            }
            if *value != old {
                resp.mark_changed();
            }
            let now = *value as f64;
            let enabled = ui.is_enabled();
            resp.widget_info(|| egui::WidgetInfo::slider(enabled, now, label));

            if ui.is_rect_visible(rect) {
                let v = ui.visuals().clone();
                let painter = ui.painter_at(rect);
                let rail = egui::Rect::from_x_y_ranges(
                    x_range,
                    egui::Rangef::new(
                        rect.center().y - RAIL_HEIGHT / 2.0,
                        rect.center().y + RAIL_HEIGHT / 2.0,
                    ),
                );
                painter.rect_filled(rail, 1.5, v.widgets.inactive.bg_fill);
                let t = slider_map::t_of(*value, min, max, log);
                let (lo, hi) = slider_map::fill_span(t, min, max);
                let x = |t: f32| egui::lerp(x_range, t);
                painter.rect_filled(
                    egui::Rect::from_x_y_ranges(x(lo)..=x(hi), rail.y_range()),
                    1.5,
                    v.selection.bg_fill,
                );
                if let Some(z) = slider_map::zero_t(min, max) {
                    painter.line_segment(
                        [
                            egui::pos2(x(z), rail.top() - 2.0),
                            egui::pos2(x(z), rail.bottom() + 2.0),
                        ],
                        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
                    );
                }
                let c = egui::pos2(x(t), rail.center().y);
                painter.circle_filled(c, HANDLE_RADIUS, v.strong_text_color());
                if resp.hovered() || resp.dragged() || resp.has_focus() {
                    painter.circle_stroke(
                        c,
                        HANDLE_RADIUS,
                        egui::Stroke::new(1.5, v.selection.bg_fill),
                    );
                }
            }

            // ── value ──
            // ⚠ A `DragValue` drawn frameless so it reads as text: typing, drag-on-the-number
            // and the formatter/parser come with it, and a zero on a bipolar row is dimmed so
            // untouched rows recede.
            let dim = bipolar && *value == 0.0;
            let value_resp = ui
                .allocate_ui_with_layout(
                    egui::vec2(value_w, height),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let vis = ui.visuals_mut();
                        vis.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                        vis.widgets.hovered.weak_bg_fill = egui::Color32::TRANSPARENT;
                        vis.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                        vis.widgets.hovered.bg_stroke = egui::Stroke::NONE;
                        if dim {
                            let weak = vis.weak_text_color();
                            vis.widgets.inactive.fg_stroke.color = weak;
                        }
                        let mut dv = egui::DragValue::new(value)
                            .range(range.clone())
                            // ⚠⚠ **`range()` clamps EXISTING values by default and writes the clamp
                            // back through the `&mut`, reporting it as `changed()`.** So a caller
                            // that handed this row a value outside the range had it silently
                            // rewritten on the first frame — and the caller, seeing `changed()`,
                            // committed it as if the user had done it.
                            //
                            // Reported in use 2026-09-23: opening certain photos set Temp to
                            // 2000 K while the preset still read "As Shot", because the white
                            // balance panel shows the *effective* temperature while `kelvin` is
                            // `None` and a warm as-shot solve fell below the slider's warm end.
                            //
                            // ⚠ `false` clamps only what the **user** types or drags, which is the
                            // only thing a range should govern. `slider_map::t_of` already clamps
                            // the handle's position, so an out-of-range value draws at the end of
                            // the rail and reads truthfully in the number.
                            .clamp_existing_to_range(false)
                            .speed(((max - min) / 200.0).max(1e-3));
                        dv = match formatter {
                            Some(f) => dv.custom_formatter(move |v, _| f(v)),
                            None => dv.min_decimals(0).max_decimals(2),
                        };
                        if let Some(p) = parser {
                            dv = dv.custom_parser(p);
                        }
                        ui.add(dv)
                    },
                )
                .inner;
            resp.union(value_resp)
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    /// Press at `from`, move to `to`, release — five passes, `drag_through`'s count, because a
    /// harness that moves on the same pass as the press produces a gesture that grabs nothing
    /// while looking exactly like one that works.
    fn drag(
        ctx: &egui::Context,
        from: egui::Pos2,
        to: egui::Pos2,
        mut body: impl FnMut(&mut egui::Ui),
    ) {
        for pass in 0..5 {
            let events = match pass {
                1 => vec![egui::Event::PointerMoved(from)],
                2 => vec![egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
                3 => vec![egui::Event::PointerMoved(to)],
                4 => vec![egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                _ => Vec::new(),
            };
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    events,
                    ..Default::default()
                },
                &mut body,
            );
        }
    }

    #[test]
    fn the_row_is_found_by_its_label_and_a_drag_moves_and_settles_it() {
        let ctx = test_ui::ctx();
        let mut v = 0.0f32;
        let mut settles = 0;
        let mut changes = 0;
        let mut body = |ui: &mut egui::Ui| {
            let r = ui.add(Slider::new(&mut v, -100.0..=100.0).text("Contrast"));
            changes += r.changed() as u32;
            settles += r.drag_stopped() as u32;
        };
        let at = test_ui::find(&ctx, "Contrast", &mut body).expect("labelled by its text");
        drag(&ctx, at, at + egui::vec2(60.0, 0.0), &mut body);
        assert!(v > 0.0, "a drag to the right moved nothing: {v}");
        assert!(changes >= 1, "no frame reported a change");
        assert_eq!(settles, 1, "one release, one settle");
    }

    #[test]
    fn a_press_without_motion_at_the_centre_of_a_bipolar_rail_leaves_zero() {
        // ⚠ The fill-from-zero geometry puts the handle at the rail's centre for 0 on a
        // symmetric range; the rail's accesskit centre is that point, so a press there must
        // resolve to 0 — a slider that reset to some other value on a stray click would be
        // reported as "the panel changes my photo".
        let ctx = test_ui::ctx();
        let mut v = 0.0f32;
        let mut body = |ui: &mut egui::Ui| {
            let _ = ui.add(Slider::new(&mut v, -100.0..=100.0).text("Tint"));
        };
        let at = test_ui::find(&ctx, "Tint", &mut body).expect("drawn");
        drag(&ctx, at, at, &mut body);
        assert_eq!(v, 0.0);
    }

    #[test]
    fn a_log_rail_reports_its_value_through_accessibility() {
        // The retouch Size row: log scale plus a "px" formatter, driven through the builder.
        let ctx = test_ui::ctx();
        let mut v = 10.0f32;
        let values = test_ui::numeric_values(&ctx, |ui| {
            let _ = ui.add(
                Slider::new(&mut v, 1.0..=100.0)
                    .text("Size")
                    .logarithmic(true)
                    .custom_formatter(|v| format!("{v:.0} px")),
            );
        });
        assert!(
            values.contains(&10.0),
            "the value never reached accessibility: {values:?}"
        );
    }
}
