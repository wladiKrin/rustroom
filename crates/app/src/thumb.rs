//! The thumbnail cell, shared by the Library grid and the Develop filmstrip.
//!
//! ⚠ **`cell` allocates first and paints into the allocated rect.** That is the whole point of
//! it: `Ui::allocate_ui` allocates the space its content *actually used*, so an earlier version
//! that drew an `Image` directly gave a portrait thumbnail a 58.6 px cell where its placeholder
//! had taken 132 — every thumbnail that arrived shifted every cell to its right, and the
//! filmstrip's centred photo drifted as its predecessors loaded.
//!
//! ⚠ **It senses `hover`, not `click`, and that is load-bearing.** In the Library grid the
//! click target is the whole cell — thumbnail *and* filename — so two overlapping click-sensing
//! widgets both sit at distance 0 from the pointer, and egui breaks that tie by registration
//! order: the thumbnail swallows the click and only the grey margin opens the photo. `cell`
//! reserves space and paints; interaction is the caller's to add, which the filmstrip does with
//! `.interact(egui::Sense::click())`.

use eframe::egui;

pub fn selection_border(ui: &egui::Ui, rect: egui::Rect, active: bool, selected: bool) {
    if !selected {
        return;
    }
    let color = if active {
        egui::Color32::from_rgb(120, 160, 240)
    } else {
        egui::Color32::from_rgb(90, 100, 120)
    };
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(3),
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Inside,
    );
}

/// One thumbnail cell — filmstrip *and* Library grid — a fixed-size box holding either the
/// thumbnail, centred and aspect-preserved, or a grey placeholder while it loads.
///
/// ⚠ **The fixed size is the entire point, and it is load-bearing.** `Ui::allocate_ui`
/// allocates the space its content *actually used*, not the space requested — so drawing a
/// loaded thumbnail with `Image::max_size` allocated a **portrait** cell only 58.6 px wide
/// against its placeholder's 132 px. Every thumbnail that arrived then shifted every cell to
/// its right, which made the strip twitch as it loaded and slid the centred photo out from
/// under the scroll target (that target is an absolute offset egui computes **once** and
/// never recomputes). Allocating first and painting into the allocated rect makes a cell's
/// geometry independent of whether its thumbnail has arrived.
/// ⚠ Allocates with `Sense::hover()`, **not** `Sense::click()` — this reserves space and
/// paints, and interaction is the caller's to add. In the Library grid the click target is
/// the whole cell (thumbnail *and* filename), so a click-sensing thumbnail sitting inside it
/// would swallow the click: both rects are distance 0 from the pointer and egui breaks that
/// tie by registration order. The grid then responded only on the grey margin around the
/// image. `clicking_a_library_grid_thumbnail_opens_it` pins it.
/// `copy` badges the cell when non-zero.
///
/// ⚠ Copies of one file show a **pixel-identical** thumbnail — Library thumbnails are the
/// camera's embedded JPEG, not a render of the edits. The badge and the label are therefore the
/// *only* things distinguishing them, which is the accepted limitation in the spec and the reason
/// edit-accurate thumbnails are backlogged.
pub fn cell(
    ui: &mut egui::Ui,
    box_size: egui::Vec2,
    thumb: Option<&egui::TextureHandle>,
    copy: u32,
    marks: catalog::db::Marks,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(box_size, egui::Sense::hover());
    match thumb {
        Some(tex) => {
            // Fit inside the cell without upscaling, then centre — the same result
            // `Image::max_size(..).maintain_aspect_ratio(true)` produced, minus the effect
            // on layout.
            let size = tex.size_vec2();
            let scale = (box_size.x / size.x).min(box_size.y / size.y).min(1.0);
            egui::Image::new(tex).paint_at(
                ui,
                egui::Rect::from_center_size(rect.center(), size * scale),
            );
        }
        None => {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(3),
                egui::Color32::from_gray(60),
            );
        }
    }
    if copy != 0 {
        let badge =
            egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 3.0), egui::vec2(17.0, 13.0));
        ui.painter().rect_filled(
            badge,
            egui::CornerRadius::same(2),
            ui.visuals().selection.bg_fill,
        );
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            copy.to_string(),
            egui::FontId::proportional(9.0),
            ui.visuals().extreme_bg_color,
        );
    }
    // ⚠ Painted into the **allocated** rect rather than allocating more: the cell's geometry must
    // not depend on whether a photo happens to be marked, or marked photos would resize their
    // cells — the reflow bug the filmstrip already paid for.
    if !marks.is_empty() {
        let p = ui.painter_at(rect);
        let pad = 3.0;
        let size = (box_size.y * 0.09).clamp(7.0, 13.0);
        let mut x = rect.left() + pad;
        let y = rect.bottom() - pad - size;
        for _ in 0..marks.rating {
            p.text(
                egui::pos2(x, y),
                egui::Align2::LEFT_TOP,
                "★",
                egui::FontId::proportional(size),
                egui::Color32::from_rgb(240, 200, 90),
            );
            x += size * 0.8;
        }
        let flag_glyph = match marks.flag {
            // ⚠⚠ `⚑` and `×`, chosen by **probing**, not by taste. `✓` and `✕` are absent from
            // every bundled font and would have shipped as tofu boxes — caught by the guard
            // below, which is the same finding the rail's four glyphs produced.
            //
            // ⚠ The flag is also the better symbol: Lightroom's "pick" *is* a flag.
            catalog::db::Flag::Pick => Some(("⚑", egui::Color32::from_rgb(120, 220, 120))),
            catalog::db::Flag::Reject => Some(("×", egui::Color32::from_rgb(230, 110, 110))),
            catalog::db::Flag::None => None,
        };
        if let Some((glyph, colour)) = flag_glyph {
            p.text(
                egui::pos2(rect.right() - pad, y),
                egui::Align2::RIGHT_TOP,
                glyph,
                egui::FontId::proportional(size),
                colour,
            );
        }
        if marks.label > 0 {
            // ⚠ Red, blue, yellow, green — **not** the obvious red/orange/yellow/green.
            // Orange and yellow are barely distinguishable at badge size, which defeats the whole
            // point of a colour label; blue is unmistakable against all three.
            //
            // ⚠ Exactly four, matching the four keys that can reach them. A fifth colour existed
            // briefly with no key bound to it — unreachable, and invisible as a bug.
            let colours = [
                egui::Color32::from_rgb(220, 90, 90),
                egui::Color32::from_rgb(110, 150, 230),
                egui::Color32::from_rgb(220, 210, 90),
                egui::Color32::from_rgb(110, 200, 110),
            ];
            let c = colours[(marks.label as usize - 1).min(colours.len() - 1)];
            p.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left() + pad, rect.top() + pad),
                    egui::vec2(size, size),
                ),
                egui::CornerRadius::ZERO,
                c,
            );
        }
    }

    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A portrait thumbnail, as `extract_thumbnail` returns it once libraw's flip is applied.
    const PORTRAIT: [usize; 2] = [341, 512];
    const LANDSCAPE: [usize; 2] = [512, 341];

    /// Lay out three identical cells and return their allocated widths.
    ///
    /// `old` reproduces the pre-fix construction (`allocate_ui` + `Image::max_size`) so the
    /// regression is demonstrated here rather than only described in a comment.
    fn cell_widths(thumb_size: Option<[usize; 2]>, old: bool) -> Vec<f32> {
        let ctx = egui::Context::default();
        let box_size = egui::vec2(132.0, 88.0);
        let tex = thumb_size.map(|s| {
            ctx.load_texture(
                "t",
                egui::ColorImage::new(s, vec![egui::Color32::WHITE; s[0] * s[1]]),
                Default::default(),
            )
        });
        let mut widths = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.horizontal(|ui| {
                for _ in 0..3 {
                    let w = if old {
                        ui.allocate_ui(box_size, |ui| match &tex {
                            Some(t) => {
                                ui.add(
                                    egui::Image::new(t)
                                        .max_size(box_size)
                                        .maintain_aspect_ratio(true),
                                );
                            }
                            None => {
                                let (rect, _) =
                                    ui.allocate_exact_size(box_size, egui::Sense::hover());
                                ui.painter().rect_filled(
                                    rect,
                                    egui::CornerRadius::same(3),
                                    egui::Color32::from_gray(60),
                                );
                            }
                        })
                        .response
                        .rect
                        .width()
                    } else {
                        cell(ui, box_size, tex.as_ref(), 0, catalog::db::Marks::default())
                            .rect
                            .width()
                    };
                    widths.push(w);
                }
            });
        });
        widths
    }

    #[test]
    fn the_old_cell_shrank_when_a_portrait_thumbnail_arrived() {
        // ⚠ The bug, reproduced. This is why the strip twitched while loading and why the
        // centred photo drifted left: every arriving portrait narrowed its cell by ~73 px,
        // shifting every cell to its right, while egui's scroll target — computed once, as
        // an absolute offset — stayed where it was.
        let empty = cell_widths(None, true)[0];
        let portrait = cell_widths(Some(PORTRAIT), true)[0];
        assert_eq!(empty, 132.0);
        assert!(
            portrait < empty - 70.0,
            "expected the old portrait cell to collapse, got {portrait} vs {empty}"
        );
    }

    #[test]
    fn a_thumb_cell_keeps_its_width_however_its_thumbnail_loads() {
        // The fix: geometry no longer depends on load state, so nothing reflows and the
        // scroll target stays valid.
        let empty = cell_widths(None, false);
        let portrait = cell_widths(Some(PORTRAIT), false);
        let landscape = cell_widths(Some(LANDSCAPE), false);
        assert_eq!(empty, vec![132.0; 3]);
        assert_eq!(portrait, empty, "a portrait thumbnail changed the layout");
        assert_eq!(landscape, empty, "a landscape thumbnail changed the layout");
    }

    /// The Library grid's shape: a `CELL`-square cell holding a thumbnail box and a filename.
    fn grid_cell_size(thumb_size: Option<[usize; 2]>) -> egui::Vec2 {
        const CELL: f32 = 140.0;
        let ctx = egui::Context::default();
        let box_size = egui::vec2(CELL - 12.0, (CELL - 12.0) * 0.66);
        let tex = thumb_size.map(|s| {
            ctx.load_texture(
                "t",
                egui::ColorImage::new(s, vec![egui::Color32::WHITE; s[0] * s[1]]),
                Default::default(),
            )
        });
        let mut size = egui::Vec2::ZERO;
        let _ = ctx.run_ui(Default::default(), |ui| {
            size = ui
                .allocate_ui(egui::vec2(CELL, CELL), |ui| {
                    ui.set_min_size(egui::vec2(CELL, CELL));
                    ui.vertical_centered(|ui| {
                        cell(ui, box_size, tex.as_ref(), 0, catalog::db::Marks::default());
                        ui.label(egui::RichText::new("DSCF6509.RAF").small());
                    });
                })
                .response
                .rect
                .size();
        });
        size
    }

    #[test]
    fn a_library_grid_cell_keeps_its_size_however_its_thumbnail_loads() {
        // The same defect as the filmstrip's, in the grid's shape. `egui::Grid` aligns
        // columns to the widest cell, which masked it — but a cell that changes size on load
        // is still a cell that moves its neighbours.
        let empty = grid_cell_size(None);
        assert_eq!(grid_cell_size(Some(PORTRAIT)), empty);
        assert_eq!(grid_cell_size(Some(LANDSCAPE)), empty);
        assert_eq!(empty.x, 140.0, "the cell should fill its column");
    }

    /// Click at `pos` and report whether the Library grid's *cell* response saw it.
    ///
    /// Three passes: one to lay out with the pointer in place, one to press, one to release
    /// — egui resolves a click against the widgets registered on the previous pass.
    fn grid_cell_click_lands(offset: egui::Vec2) -> bool {
        const CELL: f32 = 140.0;
        let ctx = egui::Context::default();
        let box_size = egui::vec2(CELL - 12.0, (CELL - 12.0) * 0.66);
        let tex = ctx.load_texture(
            "t",
            egui::ColorImage::new(LANDSCAPE, vec![egui::Color32::WHITE; 512 * 341]),
            Default::default(),
        );
        let mut clicked = false;
        let mut origin = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            let pos = origin + offset;
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui
                    .allocate_ui(egui::vec2(CELL, CELL), |ui| {
                        ui.set_min_size(egui::vec2(CELL, CELL));
                        ui.vertical_centered(|ui| {
                            cell(ui, box_size, Some(&tex), 0, catalog::db::Marks::default());
                            ui.label(egui::RichText::new("DSCF6509.RAF").small());
                        });
                    })
                    .response
                    .interact(egui::Sense::click());
                origin = resp.rect.min;
                if pass == 2 && resp.clicked() {
                    clicked = true;
                }
            });
        }
        clicked
    }

    #[test]
    fn clicking_a_library_grid_thumbnail_opens_it() {
        // ⚠ Regression: the whole cell is the click target in the grid, but `thumb_cell`
        // allocated its box with `Sense::click()`. Two overlapping click widgets both sit at
        // distance 0 from the pointer, and egui breaks that tie by registration order — so
        // the thumbnail swallowed the click and only the grey margin around it responded.
        assert!(
            grid_cell_click_lands(egui::vec2(70.0, 40.0)),
            "a click on the thumbnail itself did not reach the cell"
        );
        // The margin below the image (over the filename) must keep working too.
        assert!(
            grid_cell_click_lands(egui::vec2(70.0, 125.0)),
            "a click on the cell margin did not reach the cell"
        );
    }

    /// Click at `offset` from the first filmstrip cell and report whether that cell saw it.
    fn filmstrip_click_lands(offset: egui::Vec2) -> bool {
        let ctx = egui::Context::default();
        let box_size = egui::vec2(132.0, 88.0);
        let tex = ctx.load_texture(
            "t",
            egui::ColorImage::new(LANDSCAPE, vec![egui::Color32::WHITE; 512 * 341]),
            Default::default(),
        );
        let mut clicked = false;
        let mut origin = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 200.0),
                )),
                ..Default::default()
            };
            let pos = origin + offset;
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..3 {
                        let resp = cell(ui, box_size, Some(&tex), 0, catalog::db::Marks::default())
                            .interact(egui::Sense::click());
                        if i == 0 {
                            origin = resp.rect.min;
                            if pass == 2 && resp.clicked() {
                                clicked = true;
                            }
                        }
                    }
                });
            });
        }
        clicked
    }

    #[test]
    fn clicking_a_filmstrip_thumbnail_opens_it() {
        // The symmetric guard to the grid's. `thumb_cell` allocates with `Sense::hover()`,
        // so the filmstrip — where the cell *is* the click target — has to opt in. Dropping
        // that `.interact()` would make the strip silently unclickable.
        assert!(
            filmstrip_click_lands(egui::vec2(66.0, 44.0)),
            "a click on a filmstrip thumbnail did not register"
        );
    }

    #[test]
    fn cells_are_laid_out_at_a_constant_pitch() {
        // The property that actually matters to the scroll target: cell *n*'s position must
        // not depend on how many of its predecessors have loaded. Equal widths imply it,
        // but assert the spacing too, since that is what accumulates over 800 cells.
        let w = cell_widths(Some(PORTRAIT), false);
        assert!(w.windows(2).all(|p| p[0] == p[1]), "{w:?}");
    }

    #[test]
    fn every_mark_glyph_is_renderable() {
        // ⚠⚠ egui's bundled fonts hold about twenty usable symbols, and a missing one ships as a
        // tofu box that no other test can see. The rail's four glyphs were **all** absent when
        // that was last checked; this is the same guard.
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        // ⚠ `has_glyph` needs a frame to have run: a fresh context has not built its atlas, so
        // asking one answers about nothing.
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for g in ['★', '⚑', '×'] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, g)),
                "{g} (U+{:04X}) is not in any bundled font and would render as a tofu box",
                g as u32
            );
        }
    }
}
