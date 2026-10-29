//! The top bar: the view and mode switches, the Library bar's plan, and Export.

use super::centre_info::{centre_info, shared_gap};
use super::filter::{mark_filter, mark_filter_label, search_field};
use crate::segmented::{self, PillItem, PillStyle};
use crate::theme::Hover as _;
use crate::{develop, name_edit, theme, AppView, LibraryMode, RustroomApp};
use eframe::egui;
use rustroom_app::icons;
use rustroom_app::menu::Command;
use rustroom_app::photo_id::PhotoId;

impl RustroomApp {
    /// The bar's one row, at a **fixed** height in both views.
    ///
    /// ⚠⚠ `allocate_ui_with_layout` at `BAR_HEIGHT`, not `horizontal` with a minimum: the
    /// bar changed height between Library and Develop ("it feels like the height of the top
    /// bar also changes when switching", seen in use 2026-09-20), and a row allocated at an
    /// exact size cannot — `the_bar_is_the_same_height_in_both_views` measures it. Every
    /// control centres in the row.
    ///
    /// Returns the bar's file rename, if the centred name committed one, the command its
    /// Export button asked for, and the row's rect.
    /// The rename is applied by the caller after the panel closes: the commit writes the
    /// sidecar and rebuilds the Library with `&mut self`.
    pub(crate) fn top_bar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) -> (
        Option<(PhotoId, name_edit::Rename)>,
        Option<Command>,
        egui::Rect,
    ) {
        let mut rename = None;
        let mut command = None;
        let r = ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), BAR_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                // ⚠⚠ **The top row navigates and narrows, and nothing else** — `docs/DESIGN.md`
                // §6. Everything that informs or sets the view (the count, the sort, the size, the
                // format chips, Reindex, a running job) lives in the status row, because this row
                // overprinted itself at 1440 pt when it carried all of it.
                //
                // ⚠ Both views share one shape (2026-09-29, the author's call): navigation at the
                // left, the photo's name and facts in the centre, the view's own tools at the right
                // edge — the Library's search and Filter, Develop's Export.
                let view = self.view;
                let plan = (view == AppView::Library && self.library.is_some())
                    .then(|| plan_library_bar(self, ui));
                view_switch(ui, &mut self.view);
                if view == AppView::Develop && self.session.is_some() {
                    ui.separator();
                    let decoding = self.pending.is_some();
                    ui.add_enabled_ui(!decoding, |ui| develop::toolbar(self, ui, ctx));
                }
                if let Some(plan) = &plan {
                    ui.separator();
                    // ⚠ A visible control, not keys alone. `C` and `N` are the fast route and
                    // are what a photographer culling a shoot will use — but a feature reachable
                    // only by a key nothing on screen mentions is a feature nobody finds, and
                    // the Filter button's `Filter: ` prefix exists for the same reason.
                    let items = shown_items(&self.settings.library_views);
                    mode_switch(ui, &mut self.library_mode, &items, plan.mode_icons_only);
                }
                let row = ui.max_rect();
                let gap_left = ui.cursor().left();
                // ⚠ The right end, rightmost first: Develop's Export (its one accent-filled control,
                // the rule that gives the Library its accent-filled app mode — one per bar), or the
                // Library's Filter with the search field beside it — one idea, "narrow what I am
                // looking at", so they sit together.
                let gap_right = ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if view == AppView::Develop && self.session.is_some() {
                            if export_button(ui, &self.settings.shortcuts).clicked() {
                                command = Some(Command::Export);
                            }
                            ui.separator();
                        }
                        if let Some(plan) = &plan {
                            mark_filter(self, ui, plan.filter_compact);
                            search_field(self, ui, plan.search_width);
                            ui.separator();
                        }
                        let used = ui.min_rect().left();
                        if used.is_finite() {
                            used
                        } else {
                            row.right()
                        }
                    })
                    .inner;
                // ⚠⚠ **The name sits in the same place in both views** (the author's call,
                // 2026-09-29): centred on the bar, and clamped into the gap BOTH views leave, not
                // this one's alone — Develop's tools reach further right than the Library's, and a
                // name clamped only into its own view's gap jumped sideways on every switch.
                let gap = shared_gap(ui, view, row, gap_left, gap_right, self);
                rename = centre_info(self, ui, row, gap);
            },
        );
        (rename, command, r.response.rect)
    }
}

/// The top bar's height, `docs/DESIGN.md` §5.
const BAR_HEIGHT: f32 = crate::theme::tokens::TOP_BAR;

/// The app-mode pill's height: the control row.
const VIEW_PILL: f32 = crate::theme::tokens::CONTROL_ROW;

/// The Library layout pill's height — the same row; the accent, not the size, tells them apart.
const MODE_PILL: f32 = crate::theme::tokens::CONTROL_ROW;

/// The Library / Develop switch: an icon + label pill, its lit half filled with the accent.
///
/// ⚠⚠ **The one accent-filled control in the bar**, so it answers "where am I" at a glance —
/// which two selectable labels at the same weight as every other control did not (the canvas
/// "Rustroom Top Bar & Library", 2026-09-20). The accessible names stay "Library" / "Develop".
fn view_switch(ui: &mut egui::Ui, view: &mut AppView) {
    segmented::pill(ui, view, &VIEW_ITEMS, view_style());
}

pub(super) const VIEW_ITEMS: [PillItem<'static, AppView>; 2] = [
    PillItem {
        value: AppView::Library,
        icon: icons::LIBRARY,
        label: "Library",
        hover: "Shows the Library, to browse and cull photos (G)",
    },
    PillItem {
        value: AppView::Develop,
        icon: icons::DEVELOP,
        label: "Develop",
        hover: "Opens the selected photo for editing (D)",
    },
];

pub(super) fn view_style() -> PillStyle {
    PillStyle {
        height: VIEW_PILL,
        font: egui::FontId::proportional(crate::theme::tokens::BODY),
        accent: true,
        icons_only: false,
    }
}

/// The Grid / Compare / Survey switch: a second pill with an icon per layout.
///
/// ⚠ A *second* pill rather than three more cells on the first one: Library and Develop are
/// modules and these are layouts within one of them, and flattening the two would say that
/// leaving Compare for Develop is the same kind of move as leaving Compare for the grid. It is
/// shown only in Library, where the distinction is visible — and it takes the button fill, not
/// the accent, for the reason on [`view_switch`].
///
/// ⚠ The chord is in the hover text rather than the label: a segment reading "Grid (G)" is a
/// third of it punctuation, and the Keyboard-shortcuts dialog is generated from the menu model
/// so it already lists all three.
///
/// ⚠ **Compare's and Survey's keys are in the Keyboard shortcuts dialog** (2026-09-29): they were
/// a line of 11-pt text across the top of each view, then a paragraph in these hovers; a hover says
/// what a control does in one sentence (`docs/DESIGN.md` §8).
///
/// ⚠ `icons_only`: the narrow form the bar plan picks — the names stay in the hovers and the
/// accessible labels.
fn mode_switch(
    ui: &mut egui::Ui,
    mode: &mut LibraryMode,
    items: &[PillItem<'_, LibraryMode>],
    icons_only: bool,
) {
    segmented::pill(ui, mode, items, mode_style(icons_only));
}

/// Whether the top row shows `mode` (M10.18c — the author's setting; Grid always).
pub(crate) fn mode_shown(v: &rustroom_app::settings::LibraryViews, mode: LibraryMode) -> bool {
    match mode {
        LibraryMode::Grid => true,
        LibraryMode::Compare => v.compare,
        LibraryMode::Survey => v.survey,
        LibraryMode::Map => v.map,
        LibraryMode::People => v.people,
    }
}

/// The views the top row shows, in order.
fn shown_items(v: &rustroom_app::settings::LibraryViews) -> Vec<PillItem<'static, LibraryMode>> {
    MODE_ITEMS
        .into_iter()
        .filter(|i| mode_shown(v, i.value))
        .collect()
}

const MODE_ITEMS: [PillItem<'static, LibraryMode>; 5] = [
    PillItem {
        value: LibraryMode::Grid,
        icon: icons::GRID,
        label: "Grid",
        hover: "Shows the photos as a grid of thumbnails (G)",
    },
    PillItem {
        value: LibraryMode::Compare,
        icon: icons::COMPARE,
        label: "Compare",
        hover: "Compares two photos side by side (C)",
    },
    PillItem {
        value: LibraryMode::Survey,
        icon: icons::SURVEY,
        label: "Survey",
        hover: "Shows the whole selection at once, to narrow it down (N)",
    },
    PillItem {
        value: LibraryMode::Map,
        icon: icons::MAP,
        label: "Map",
        hover: "Shows on a map where the photos in view were taken",
    },
    PillItem {
        value: LibraryMode::People,
        icon: icons::PEOPLE,
        label: "People",
        hover: "Shows who is in the photos, and the faces still to name",
    },
];

fn mode_style(icons_only: bool) -> PillStyle {
    PillStyle {
        height: MODE_PILL,
        font: egui::FontId::proportional(crate::theme::tokens::BODY),
        accent: false,
        icons_only,
    }
}

/// What the Library's top row draws at this width — `bar_plan`'s answer for its four items.
struct LibraryBarPlan {
    mode_icons_only: bool,
    search_width: f32,
    filter_compact: bool,
}

/// The search field at its full and its narrow width.
const SEARCH_WIDE: f32 = 240.0;

const SEARCH_NARROW: f32 = 120.0;

/// Measure the Library row's items and plan them into the bar's width.
///
/// ⚠ Measured with the same fonts and paddings the widgets draw with, plus the separators; the
/// app switch is fixed, and the rest step down in the order `docs/DESIGN.md` §6 gives — the
/// Filter's label first, then the mode switch's, then the search narrows.
fn plan_library_bar(app: &RustroomApp, ui: &egui::Ui) -> LibraryBarPlan {
    use rustroom_app::bar_plan::{plan, Form, Item};
    let body = egui::TextStyle::Body.resolve(ui.style());
    let pad = ui.spacing().button_padding.x * 2.0;
    let gap = ui.spacing().item_spacing.x;
    let separators = 2.0 * (gap * 2.0 + 1.0);
    let text_w = |t: &str| {
        ui.painter()
            .layout_no_wrap(t.to_owned(), body.clone(), egui::Color32::PLACEHOLDER)
            .size()
            .x
    };
    let filter_full = text_w(&format!(
        "{} {}",
        icons::FILTER,
        mark_filter_label(&app.filter)
    )) + pad;
    let filter_compact = text_w(&format!("{} •", icons::FILTER)) + pad;
    // ⚠ The row it will actually draw — the views the author chose to show.
    let modes = shown_items(&app.settings.library_views);
    let items = [
        Item::fixed(99, segmented::pill_width(ui, &VIEW_ITEMS, &view_style())),
        Item {
            priority: 60,
            full: segmented::pill_width(ui, &modes, &mode_style(false)),
            compact: Some(segmented::pill_width(ui, &modes, &mode_style(true))),
            hideable: false,
        },
        Item {
            priority: 70,
            full: SEARCH_WIDE,
            compact: Some(SEARCH_NARROW),
            hideable: false,
        },
        Item {
            priority: 50,
            full: filter_full,
            compact: Some(filter_compact),
            hideable: false,
        },
    ];
    let forms = plan((ui.available_width() - separators).max(0.0), gap, &items);
    LibraryBarPlan {
        mode_icons_only: forms[1] == Form::Compact,
        search_width: if forms[2] == Form::Compact {
            SEARCH_NARROW
        } else {
            SEARCH_WIDE
        },
        filter_compact: forms[3] == Form::Compact,
    }
}

/// The Develop toolbar's Export button: the bar's one accent-filled control.
fn export_button(ui: &mut egui::Ui, keys: &[rustroom_app::shortcuts::Rebinding]) -> egui::Response {
    ui.scope(|ui| {
        ui.style_mut().spacing.button_padding.y = (MODE_PILL - 14.0) / 2.0;
        theme::primary_button(ui, &format!("{} Export…", icons::EXPORT), true)
    })
    .inner
    .hover(format!(
        "Exports the selected photos{}",
        rustroom_app::shortcuts::hint(Command::Export, keys)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bar with a Library of three, the second selected, and that same photo open.
    fn app_with_photo() -> (egui::Context, RustroomApp) {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let mut lib = crate::test_app::library_of(3);
        lib.selection.click(1);
        let path = lib.entries[1].id.path.clone();
        if let Some(s) = app.session.as_mut() {
            s.path = path;
        }
        app.library = Some(lib);
        (ctx, app)
    }

    /// Draw the bar in `view` and return the row's height and every accessible label in it.
    fn bar(ctx: &egui::Context, app: &mut RustroomApp, view: AppView) -> (f32, Vec<String>) {
        app.view = view;
        let mut height = 0.0;
        // ⚠ A wide screen: at the panels' 400 pt the bar has no gap for its centred line.
        let wide = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 200.0));
        let labels: Vec<String> =
            crate::test_ui::labelled_centres_on(ctx, wide, Vec::new(), |ui| {
                let ctx = ui.ctx().clone();
                let (_, _, rect) = app.top_bar(ui, &ctx);
                height = rect.height();
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        (height, labels)
    }

    #[test]
    fn the_bar_is_the_same_height_in_both_views() {
        // ⚠ Seen in use: the bar changed height on a view switch. The row is allocated at an
        // exact height now, and this measures it with the real controls of both views.
        let (ctx, mut app) = app_with_photo();
        let (lib_h, _) = bar(&ctx, &mut app, AppView::Library);
        let (dev_h, _) = bar(&ctx, &mut app, AppView::Develop);
        assert_eq!(lib_h, BAR_HEIGHT, "the Library bar grew past its height");
        assert_eq!(dev_h, BAR_HEIGHT, "the Develop bar grew past its height");
    }

    #[test]
    fn the_centred_name_is_the_same_in_both_views() {
        // ⚠ Seen in use: the line said different things about one photo depending on the
        // view. The name is the caption widget's label; it must be there, and be the same.
        // ⚠ And only there: the status row must not name the photo a second time.
        let (ctx, mut app) = app_with_photo();
        let name = app.library.as_ref().unwrap().entries[1].caption();
        let (_, lib) = bar(&ctx, &mut app, AppView::Library);
        let (_, dev) = bar(&ctx, &mut app, AppView::Develop);
        assert!(lib.contains(&name), "Library bar lacks the name: {lib:?}");
        assert!(dev.contains(&name), "Develop bar lacks the name: {dev:?}");
        app.view = AppView::Library;
        let wide = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 200.0));
        let status: Vec<String> =
            crate::test_ui::labelled_centres_on(&ctx, wide, Vec::new(), |ui| {
                let ctx = ui.ctx().clone();
                crate::status_bar::library_row(&mut app, ui, &ctx);
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        assert!(
            !status.contains(&name),
            "the status row names the photo too: {status:?}"
        );
    }

    /// ⚠⚠ **Reported in use 2026-09-29: the name should be "in the center of the top row, same as
    /// in Develop, so the position doesn't change when you switch."** Drawn with the real theme at
    /// the 13-inch width, **Library first** — Develop never drawn, so its edges come from the
    /// measured seed — then Develop, then the Library again; the name's centre must not move.
    /// ⚠ The first draw is the case the seed exists for: without it the Library clamps into its own
    /// wider gap and the name jumps on the first switch of a session.
    #[test]
    fn the_centred_name_sits_at_the_same_x_in_both_views() {
        let (ctx, mut app) = app_with_photo();
        crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
        let name = app.library.as_ref().unwrap().entries[1].caption();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 200.0));
        let mut x_in = |view: AppView| -> f32 {
            app.view = view;
            let found = crate::test_ui::labelled_centres_on(&ctx, screen, Vec::new(), |ui| {
                let ctx = ui.ctx().clone();
                let _ = app.top_bar(ui, &ctx);
            });
            found
                .iter()
                .find(|(l, _)| *l == name)
                .unwrap_or_else(|| panic!("{} lacks the name: {found:?}", view == AppView::Develop))
                .1
                .x
        };
        let lib_first = x_in(AppView::Library);
        let dev = x_in(AppView::Develop);
        let lib_again = x_in(AppView::Library);
        assert!(
            (lib_first - dev).abs() < 1.0 && (lib_again - dev).abs() < 1.0,
            "the name moved: Library first {lib_first}, Develop {dev}, Library again {lib_again}"
        );
    }

    /// ⚠⚠ Seen in use 2026-09-29: the name sat 27 pt further left in the Library than in Develop,
    /// because the Library's facts lacked the pixel size and the whole block was centred. Same view,
    /// same gaps, facts of two lengths: the name's centre must not move. ⚠ The sample proves the
    /// facts really differed, or equal positions would say nothing.
    #[test]
    fn the_name_does_not_move_with_its_facts() {
        let (ctx, mut app) = app_with_photo();
        crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
        let name = app.library.as_ref().unwrap().entries[1].caption();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 200.0));
        let draw = |app: &mut RustroomApp, view: AppView| {
            app.view = view;
            crate::test_ui::labelled_centres_on(&ctx, screen, Vec::new(), |ui| {
                let ctx = ui.ctx().clone();
                let _ = app.top_bar(ui, &ctx);
            })
        };
        // Develop once, so both views' edges are recorded and the gap is the same below.
        let _ = draw(&mut app, AppView::Develop);
        let with = draw(&mut app, AppView::Library);
        app.session = None;
        let without = draw(&mut app, AppView::Library);
        let at = |found: &[(String, egui::Pos2)]| {
            found
                .iter()
                .find(|(l, _)| *l == name)
                .map(|(_, p)| p.x)
                .unwrap_or_else(|| panic!("the name is missing: {found:?}"))
        };
        let facts = |found: &[(String, egui::Pos2)]| {
            found
                .iter()
                .find(|(l, _)| l.contains(" · ") || l.contains("Camera"))
                .map(|(l, _)| l.clone())
        };
        assert_ne!(
            facts(&with),
            facts(&without),
            "sample: the facts did not differ"
        );
        assert!(
            (at(&with) - at(&without)).abs() < 0.5,
            "the name moved with its facts: {} vs {}",
            at(&with),
            at(&without)
        );
    }
}
