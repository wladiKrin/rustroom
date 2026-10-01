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

/// The selected cell's border: the accent for the active one, a dimmed accent for the rest of
/// a multi-selection.
/// Being able to tell the source from the targets is what makes a sync predictable.
///
/// ⚠ Through the theme's accent rather than the fixed blue it used to be — the one hand-painted
/// colour the theme's own module comment listed as not yet routed through it.
pub fn selection_border(ui: &egui::Ui, rect: egui::Rect, active: bool, selected: bool) {
    if !selected {
        return;
    }
    let accent = ui.visuals().selection.bg_fill;
    let color = if active {
        accent
    } else {
        accent.gamma_multiply(0.45)
    };
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(crate::theme::tokens::RADIUS_THUMB),
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
/// `badge` labels the cell when present — the copy's label, else its number.
///
/// ⚠ Copies of one file show a **pixel-identical** thumbnail — Library thumbnails are the
/// camera's embedded JPEG, not a render of the edits. The badge and the label are therefore the
/// *only* things distinguishing them, which is the accepted limitation in the spec and the reason
/// edit-accurate thumbnails are backlogged.
/// Where the last badge `cell` drew for this `Ui` landed, if any.
///
/// ⚠ Keyed on the `Ui`'s id, so it is per cell rather than per frame — the grid draws hundreds.
pub fn badge_rect(ui: &egui::Ui) -> Option<egui::Rect> {
    ui.ctx().memory(|m| m.data.get_temp(badge_id(ui)))
}

fn badge_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("thumb_badge_rect")
}

/// Where the last `cell` drew for this `Ui` painted its picture — the placeholder's box when there
/// is no picture yet. The grid anchors the stack badge to it (`docs/DESIGN.md` §7.10), so a
/// portrait thumbnail's badge sits on the photograph rather than in the empty corner of its cell.
pub fn image_rect(ui: &egui::Ui) -> Option<egui::Rect> {
    ui.ctx().memory(|m| m.data.get_temp(image_id(ui)))
}

fn image_id(ui: &egui::Ui) -> egui::Id {
    ui.id().with("thumb_image_rect")
}

pub fn cell(
    ui: &mut egui::Ui,
    box_size: egui::Vec2,
    thumb: Option<&egui::TextureHandle>,
    badge: Option<&str>,
    marks: catalog::db::Marks,
) -> egui::Response {
    // ⚠ `false`: the filmstrip carries no downloaded indicator. Turning it on there later is one
    // argument, but it is a Develop-side strip where the photo is open and therefore downloaded.
    cell_with(
        ui,
        box_size,
        thumb,
        badge,
        marks,
        CellStyle {
            overlay_marks: true,
            downloaded: false,
            sit_on_bottom: false,
        },
        &[],
    )
}

/// How a cell is drawn beyond its picture.
#[derive(Clone, Copy, Debug, Default)]
pub struct CellStyle {
    /// Paint the marks over the picture's bottom edge (the filmstrip, which has no room under
    /// it) rather than leaving them to a [`marks_row`] the caller draws (the Library grid).
    pub overlay_marks: bool,
    /// Show the downloaded indicator.
    pub downloaded: bool,
    /// Sit the picture on the box's bottom edge, so the caption under the box is right under
    /// the picture whatever its shape; `false` centres it.
    pub sit_on_bottom: bool,
}

/// The thumbnail's corner radius in points.
const CORNER: u8 = crate::theme::tokens::RADIUS_THUMB;

/// [`cell`] with a say over the marks: `overlay_marks` paints them over the picture's bottom
/// edge (the filmstrip, which has no room under it); `false` leaves them to the caller, who
/// draws a [`marks_row`] under the picture (the Library grid).
pub fn cell_with(
    ui: &mut egui::Ui,
    box_size: egui::Vec2,
    thumb: Option<&egui::TextureHandle>,
    badge: Option<&str>,
    marks: catalog::db::Marks,
    style: CellStyle,
    // ⚠ M10.8c after use: a collapsed stack's pile, one layer per photo below the top, nearest first —
    // its thumbnail when peeking and loaded, else a plain card. Empty for anything else.
    pile: &[Option<&egui::TextureHandle>],
) -> egui::Response {
    let CellStyle {
        overlay_marks,
        downloaded,
        sit_on_bottom,
    } = style;
    let (rect, resp) = ui.allocate_exact_size(box_size, egui::Sense::hover());
    // ⚠ The pile steps up and to the right, so the top photo gives up that much of the box at its top
    // and its right — the cell's own size never changes (the reflow rule above).
    let step = rustroom_app::cell_layout::pile_step(pile.len(), box_size.x.min(box_size.y));
    let reach = step * pile.len() as f32;
    let inner = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + reach),
        egui::pos2(rect.right() - reach, rect.bottom()),
    );
    let at = match thumb {
        Some(tex) => {
            // Fit inside the cell without upscaling — the same result
            // `Image::max_size(..).maintain_aspect_ratio(true)` produced, minus the effect
            // on layout.
            let size = tex.size_vec2();
            let scale = (inner.width() / size.x)
                .min(inner.height() / size.y)
                .min(1.0);
            let painted = size * scale;
            if sit_on_bottom {
                egui::Rect::from_min_size(
                    egui::pos2(
                        inner.center().x - painted.x * 0.5,
                        inner.bottom() - painted.y,
                    ),
                    painted,
                )
            } else {
                egui::Rect::from_center_size(inner.center(), painted)
            }
        }
        None => inner,
    };
    paint_pile(ui, at, step, pile);
    if !pile.is_empty() {
        // ⚠ A rim around the top photo, so it stands off the layer right behind it.
        ui.painter().rect_filled(
            at.expand(1.5),
            egui::CornerRadius::same(CORNER),
            pile_rim(ui),
        );
    }
    match thumb {
        Some(tex) => {
            egui::Image::new(tex)
                .corner_radius(egui::CornerRadius::same(CORNER))
                .paint_at(ui, at);
        }
        None => {
            ui.painter().rect_filled(
                at,
                egui::CornerRadius::same(CORNER),
                crate::theme::tokens::of(ui).raised,
            );
        }
    }
    ui.ctx()
        .memory_mut(|m| m.data.insert_temp(image_id(ui), at));
    let mut badge_end: Option<f32> = None;
    if let Some(text) = badge {
        // ⚠ Sized to its text, not fixed: a badge used to hold only a copy number, and a label
        // like "Client crop" would have overflowed a 17 pt box. Capped at half the cell so a long
        // label cannot cover the photograph, and elided rather than clipped so the truncation is
        // visible as truncation.
        let font = egui::FontId::proportional(crate::theme::tokens::CAPTION);
        let max_w = (box_size.x * 0.5).max(18.0);
        let galley = ui.painter().layout(
            text.to_string(),
            font.clone(),
            egui::Color32::WHITE,
            max_w - 6.0,
        );
        let size = egui::vec2((galley.size().x + 10.0).max(18.0), 16.0);
        let badge_rect = egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 3.0), size);
        // ⚠ Published through egui's memory rather than the return type: `cell` already returns
        // the cell's own `Response`, and the two call sites want the badge only to open a label
        // draft. Threading a second rect through both would churn every caller for one use.
        ui.ctx()
            .memory_mut(|m| m.data.insert_temp(badge_id(ui), badge_rect));
        // ⚠ White on 55 % black, `docs/DESIGN.md` §7.10: readable over any photograph, and not
        // the accent — the accent means *selected*, and a badge on every stack said so falsely.
        ui.painter().rect_filled(
            badge_rect,
            egui::CornerRadius::same(4),
            egui::Color32::from_black_alpha(140),
        );
        ui.painter().galley(
            badge_rect.center() - galley.size() / 2.0,
            galley,
            egui::Color32::WHITE,
        );
        badge_end = Some(badge_rect.right());
    }
    if downloaded {
        paint_downloaded(&ui.painter_at(rect), rect, badge_end);
    }
    // ⚠ Painted into the **allocated** rect rather than allocating more: the cell's geometry must
    // not depend on whether a photo happens to be marked, or marked photos would resize their
    // cells — the reflow bug the filmstrip already paid for.
    if overlay_marks {
        paint_marks(&ui.painter_at(rect), rect, marks, box_size.y * 0.09);
    }

    resp
}

/// The rim around every photo of a pile — dark on the light theme, light on the dark one, where a dark
/// rim vanished into the panel (the author, after use).
fn pile_rim(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_white_alpha(110)
    } else {
        egui::Color32::from_black_alpha(110)
    }
}

/// The layers behind a collapsed stack's top photo (M10.8c after use), farthest first so the nearest
/// sits on top: each the top's rect stepped `k × step` up and to the right, the photo below peeking out
/// when its thumbnail is given — else a plain card — and darker the deeper it lies.
fn paint_pile(ui: &egui::Ui, top: egui::Rect, step: f32, pile: &[Option<&egui::TextureHandle>]) {
    let t = crate::theme::tokens::of(ui);
    for (k, layer) in pile.iter().enumerate().rev() {
        let depth = (k + 1) as f32;
        let r = top.translate(egui::vec2(depth * step, -depth * step));
        match layer {
            Some(tex) => {
                egui::Image::new(*tex)
                    .corner_radius(egui::CornerRadius::same(CORNER))
                    .paint_at(ui, r);
            }
            None => {
                ui.painter()
                    .rect_filled(r, egui::CornerRadius::same(CORNER), t.raised);
            }
        }
        // ⚠ A scrim over a photograph, as the badges use — deeper is darker, so the pile reads in depth.
        ui.painter().rect_filled(
            r,
            egui::CornerRadius::same(CORNER),
            // ⚠ Lighter after use ("more pronounced"): the photos below show through.
            egui::Color32::from_black_alpha((20.0 + 18.0 * depth).min(150.0) as u8),
        );
        ui.painter().rect_stroke(
            r,
            egui::CornerRadius::same(CORNER),
            // ⚠ The same rim as the top photo's, on every layer (the author: "for all photos in the
            // pile"); the theme's `line` vanished on the dark panel.
            egui::Stroke::new(1.5, pile_rim(ui)),
            egui::StrokeKind::Inside,
        );
    }
}

/// A circled downward arrow, marking a photo whose original is on disk.
///
/// ⚠⚠ **Hand-painted, and that is forced rather than lazy.** `icons.rs` requires every codepoint to be
/// verified against the bundled Phosphor 2.1.2 TTF, because *"four separate features in this project
/// have picked a glyph that turned out to be absent"*. That font's `post` table is **version 3.0 — no
/// glyph names** — so a codepoint can be verified as *present* but **not** as being the right picture,
/// and shipping the wrong icon looks fine while being wrong. A circle and three strokes need no
/// verification and scale cleanly. The histogram's clipping marks are painted for the same reason.
///
/// ⚠ `after` is the badge's right edge when a cell also carries a copy or label badge: the indicator
/// then sits just past it, so neither moves the other's established position and they never overlap.
/// A virtual copy of a downloaded Immich photo is exactly that case.
///
/// ⚠ A translucent black disc with white ink rather than an accent colour, so it stays legible over a
/// blown sky and over a dark frame alike.
fn paint_downloaded(p: &egui::Painter, rect: egui::Rect, after: Option<f32>) {
    // ⚠ Proportional to the cell with a floor and a ceiling, rather than a fixed radius: the grid's
    // cell size is a slider, and a mark that does not grow with it reads as a speck at large sizes.
    // ⚠ Enlarged on the same 2026-09-23 call as the marks row.
    let r = (rect.height() * 0.075).clamp(8.0, 16.0);
    const PAD: f32 = 3.0;
    let left = match after {
        Some(x) => x + PAD,
        None => rect.left() + PAD,
    };
    let c = egui::pos2(left + r, rect.top() + PAD + r);
    p.circle_filled(c, r, egui::Color32::from_black_alpha(150));
    // ⚠ Every offset is a fraction of `r`, so the arrow keeps its proportions at any radius rather
    // than turning into a dot inside a big disc.
    let stroke = egui::Stroke::new((r * 0.23).max(1.4), egui::Color32::WHITE);
    let tip = c + egui::vec2(0.0, r * 0.45);
    p.line_segment([c + egui::vec2(0.0, -r * 0.5), tip], stroke);
    p.line_segment([c + egui::vec2(-r * 0.45, 0.0), tip], stroke);
    p.line_segment([c + egui::vec2(r * 0.45, 0.0), tip], stroke);
}

/// The marks row's height for a glyph of `size` points.
pub fn marks_row_height(size: f32) -> f32 {
    size + 4.0
}

/// A row under a grid thumbnail: the stars at the left, the flag and the colour label at the
/// right. Allocated at a **constant** height whether or not the photo is marked, so a mark can
/// never resize a cell — the reflow rule [`cell`] follows.
///
/// ⚠ Under the picture rather than over it (2026-09-20): a star over a photograph's corner is
/// read as part of the photograph until you look twice, and a rating you have to look twice
/// for is one you miss while culling.
pub fn marks_row(ui: &mut egui::Ui, width: f32, marks: catalog::db::Marks, size: f32) {
    // ⚠ 10–20, up from 7–13 (user's call, 2026-09-23): the stars, the reject cross and the colour
    // chip were legible but small, and they are what the grid is *read* for while culling. The floor
    // rises with the ceiling so a small cell does not shrink them back to unreadable.
    let size = size.clamp(10.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, marks_row_height(size)),
        egui::Sense::hover(),
    );
    if marks.is_empty() || !ui.is_rect_visible(rect) {
        return;
    }
    let p = ui.painter_at(rect);
    let y = rect.center().y;
    let mut x = rect.left() + 2.0;
    for _ in 0..marks.rating {
        p.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            "★",
            egui::FontId::proportional(size),
            egui::Color32::from_rgb(240, 200, 90),
        );
        x += size * 0.8;
    }
    let mut right = rect.right() - 2.0;
    if marks.label > 0 {
        let c = label_colour(marks.label);
        p.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(right - size, y - size / 2.0),
                egui::vec2(size, size),
            ),
            egui::CornerRadius::same(2),
            c,
        );
        right -= size + 5.0;
    }
    if let Some((glyph, colour, scale)) = flag_glyph(marks.flag) {
        p.text(
            egui::pos2(right, y),
            egui::Align2::RIGHT_CENTER,
            glyph,
            egui::FontId::proportional(size * scale),
            colour,
        );
    }
}

/// The flag's glyph and colour.
///
/// ⚠⚠ `⚑` and `×`, chosen by **probing**, not by taste. `✓` and `✕` are absent from every
/// bundled font and would have shipped as tofu boxes — caught by `every_mark_glyph_is_renderable`,
/// the same finding the rail's four glyphs produced. ⚠ The flag is also the better symbol:
/// Lightroom's "pick" *is* a flag.
/// The flag's glyph, its colour, and **how much to scale it** for the size the row asked for.
///
/// ⚠⚠ The scale compensates for the glyph's own proportions, not for a preference about flags. `×` is
/// U+00D7, the multiplication sign, which occupies far less of its em box than `★` or `⚑` — so at an
/// identical font size it reads visibly smaller, which is what "make the reject cross even larger"
/// was about (user, 2026-09-23). ⚠ Scaling the glyph rather than swapping in a heavier one such as
/// `✕`: a different codepoint risks a tofu box, and `×` is known to render in the bundled fonts.
fn flag_glyph(flag: catalog::db::Flag) -> Option<(&'static str, egui::Color32, f32)> {
    match flag {
        catalog::db::Flag::Pick => Some(("⚑", egui::Color32::from_rgb(120, 220, 120), 1.0)),
        catalog::db::Flag::Reject => Some(("×", egui::Color32::from_rgb(230, 110, 110), 1.7)),
        catalog::db::Flag::None => None,
    }
}

/// The colour of label `n` (1-based).
///
/// ⚠ Red, blue, yellow, green — **not** the obvious red/orange/yellow/green. Orange and yellow
/// are barely distinguishable at badge size, which defeats the whole point of a colour label;
/// blue is unmistakable against all three.
///
/// ⚠ Exactly four, matching the four keys that can reach them. A fifth colour existed briefly
/// with no key bound to it — unreachable, and invisible as a bug.
fn label_colour(n: u8) -> egui::Color32 {
    let colours = [
        egui::Color32::from_rgb(220, 90, 90),
        egui::Color32::from_rgb(110, 150, 230),
        egui::Color32::from_rgb(220, 210, 90),
        egui::Color32::from_rgb(110, 200, 110),
    ];
    colours[(n as usize - 1).min(colours.len() - 1)]
}

/// Draw a photo's stars, flag and colour label into `rect`.
///
/// ⚠ Extracted from [`cell`] so Compare and Survey draw the *same* badges rather than growing
/// a second set — the rule `cell_menu`, `name_edit` and `apply_cell_action` already follow.
/// The glyphs and the four label colours are decisions with reasons, recorded below, and a
/// second copy is where they quietly become five colours and a tick nobody can render.
///
/// `nominal` is the badge size before clamping — the caller's cell height times a fraction —
/// so a tile several hundred points tall does not get a badge to match.
pub(crate) fn paint_marks(
    p: &egui::Painter,
    rect: egui::Rect,
    marks: catalog::db::Marks,
    nominal: f32,
) {
    if !marks.is_empty() {
        let pad = 3.0;
        let size = nominal.clamp(7.0, 13.0);
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
        // ⚠ The same per-glyph scale as `marks_row`, so the filmstrip's overlay and the grid's row
        // cannot disagree about how big a reject cross is.
        if let Some((glyph, colour, scale)) = flag_glyph(marks.flag) {
            p.text(
                egui::pos2(rect.right() - pad, y),
                egui::Align2::RIGHT_TOP,
                glyph,
                egui::FontId::proportional(size * scale),
                colour,
            );
        }
        if marks.label > 0 {
            let c = label_colour(marks.label);
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
                        cell(
                            ui,
                            box_size,
                            tex.as_ref(),
                            None,
                            catalog::db::Marks::default(),
                        )
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

    /// The Library grid's shape: a `CELL`-square cell holding a thumbnail box, a filename and
    /// the marks row.
    fn grid_cell_size(thumb_size: Option<[usize; 2]>) -> egui::Vec2 {
        grid_cell_size_marked(thumb_size, catalog::db::Marks::default())
    }

    fn grid_cell_size_marked(
        thumb_size: Option<[usize; 2]>,
        marks: catalog::db::Marks,
    ) -> egui::Vec2 {
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
                        cell_with(
                            ui,
                            box_size,
                            tex.as_ref(),
                            None,
                            marks,
                            CellStyle::default(),
                            &[],
                        );
                        ui.label(egui::RichText::new("DSCF6509.RAF").small());
                        marks_row(ui, box_size.x, marks, 9.0);
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

    #[test]
    fn a_marked_grid_cell_is_the_size_of_an_unmarked_one() {
        // ⚠ The marks row is allocated whether or not the photo is marked — rating a photo must
        // not move its neighbours, the reflow rule the whole module exists for.
        let marked = catalog::db::Marks {
            rating: 5,
            flag: catalog::db::Flag::Pick,
            label: 2,
        };
        assert_eq!(
            grid_cell_size_marked(Some(LANDSCAPE), marked),
            grid_cell_size(Some(LANDSCAPE))
        );
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
                            cell(
                                ui,
                                box_size,
                                Some(&tex),
                                None,
                                catalog::db::Marks::default(),
                            );
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
                        let resp = cell(
                            ui,
                            box_size,
                            Some(&tex),
                            None,
                            catalog::db::Marks::default(),
                        )
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

    /// M10.8c after use: a collapsed stack of four draws three layers behind its top photo, each
    /// stepped up and to the right, and the top photo gives up that room inside the same box.
    #[test]
    fn a_pile_draws_one_layer_per_photo_below() {
        let ctx = egui::Context::default();
        let box_size = egui::vec2(160.0, 106.0);
        let cards: [Option<&egui::TextureHandle>; 3] = [None, None, None];
        let shapes_and_top = |pile: &[Option<&egui::TextureHandle>]| {
            let mut top = egui::Rect::NOTHING;
            let out = ctx.run_ui(Default::default(), |ui| {
                cell_with(
                    ui,
                    box_size,
                    None,
                    None,
                    Default::default(),
                    CellStyle::default(),
                    pile,
                );
                top = image_rect(ui).unwrap();
            });
            (out.shapes, top)
        };
        let (plain, alone) = shapes_and_top(&[]);
        let (piled, top) = shapes_and_top(&cards);
        let step = rustroom_app::cell_layout::pile_step(3, box_size.y);
        assert!(step > 0.0, "the sample");
        assert!(
            (alone.width() - top.width() - 3.0 * step).abs() < 0.01,
            "room for the pile"
        );
        let rects: Vec<egui::Rect> = piled
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) => Some(r.rect),
                _ => None,
            })
            .collect();
        for k in 1..=3 {
            let want = top.translate(egui::vec2(k as f32 * step, -(k as f32) * step));
            assert!(
                rects.iter().any(|r| (r.min - want.min).length() < 0.01),
                "layer {k} at {want:?}: {rects:?}"
            );
        }
        assert!(piled.len() > plain.len(), "the pile added shapes");
    }

    /// The rim around a pile's top photo is light on the dark theme and dark on the light one.
    #[test]
    fn the_rim_contrasts_with_the_theme() {
        // The top photo's rim fill and every layer's outline, on one theme.
        let rims = |dark: bool| -> Vec<egui::Color32> {
            let ctx = egui::Context::default();
            ctx.set_visuals(if dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            let mut top = egui::Rect::NOTHING;
            let out = ctx.run_ui(Default::default(), |ui| {
                cell_with(
                    ui,
                    egui::vec2(160.0, 106.0),
                    None,
                    None,
                    Default::default(),
                    CellStyle::default(),
                    &[None, None],
                );
                top = image_rect(ui).unwrap();
            });
            let mut found = Vec::new();
            for c in &out.shapes {
                if let egui::Shape::Rect(r) = &c.shape {
                    if (r.rect.min - top.expand(1.5).min).length() < 0.01 {
                        found.push(r.fill);
                    }
                    if r.stroke.width >= 1.5 {
                        found.push(r.stroke.color);
                    }
                }
            }
            found
        };
        let (dark, light) = (rims(true), rims(false));
        assert_eq!(
            dark.len(),
            3,
            "the sample: the top's rim and two layers' — {dark:?}"
        );
        assert!(
            dark.iter().all(|c| c.r() > 0),
            "light on the dark theme: {dark:?}"
        );
        assert!(
            light.iter().all(|c| c.r() == 0),
            "dark on the light theme: {light:?}"
        );
    }
}
