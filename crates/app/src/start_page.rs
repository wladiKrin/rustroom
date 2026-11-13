//! The start page (M11.4): what the Library shows with nothing open — the heading, the two ways in, the
//! Continue card, and beneath it the welcome until it is ticked away. `docs/DESIGN.md` §7.14; mockups at
//! <https://claude.ai/artifact/Hnj6DR4oeAh21SR1KGYjge>.
//!
//! ⚠ No drop hint, by the author's call: dropping a folder onto the window is intuitive.

use crate::theme::Hover;
use eframe::egui;
use rustroom_app::menu::Command;
use rustroom_app::start::{ContinueKind, ContinueRow};
use std::path::PathBuf;

/// The column's width at most; narrower windows take what they have.
const COLUMN: f32 = 620.0;

/// What the page needs from the app.
pub(crate) struct StartInputs<'a> {
    pub(crate) rows: &'a [ContinueRow],
    pub(crate) show_welcome: bool,
    pub(crate) keys: &'a [rustroom_app::shortcuts::Rebinding],
}

/// What a click on the page asks for, applied once the grid has let go of the app.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StartPick {
    Command(Command),
    AllPhotos,
    Folder(PathBuf),
    HideWelcome,
}

const WELCOME: &str = "Welcome to Rustroom";
const WELCOME_LEAD: &str = "Your files are never changed, edits are kept separately beside them.";
const NOTHING_OPEN: &str = "No photos open";
const NOTHING_OPEN_LEAD: &str = "Pick up where you were, or open something new";
const CONTINUE: &str = "Continue";
const WHATS_WHERE: &str = "What's where";
const DONT_SHOW: &str = "Don't show this again";

/// The two maps' keys: `(number, name, what it is for)`. ① is the app switch in both.
const LIBRARY_KEY: [(u8, &str, &str); 4] = [
    (1, "Library · Develop", "browse, or edit one photo"),
    (2, "Rail", "where photos come from"),
    (3, "Search and Filter", "narrow the grid"),
    (4, "Status row", "counts, sort and jobs"),
];
const DEVELOP_KEY: [(u8, &str, &str); 4] = [
    (5, "Panel", "the edits, in sections"),
    (6, "Tools", "crop, masks, retouch and more"),
    (7, "Filmstrip", "the other photos you have open"),
    (8, "Export", "save the finished picture"),
];

/// Every fixed string the page paints — `theme::every_menu_string_is_renderable` sweeps them.
#[cfg(test)]
pub(crate) fn strings() -> Vec<String> {
    let mut v: Vec<String> = [
        WELCOME,
        WELCOME_LEAD,
        NOTHING_OPEN,
        NOTHING_OPEN_LEAD,
        CONTINUE,
        WHATS_WHERE,
        DONT_SHOW,
        "Open a folder…",
        "Import from a card…",
        "Library",
        "Develop",
        "All Photos",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (n, name, what) in LIBRARY_KEY.iter().chain(&DEVELOP_KEY) {
        v.push(n.to_string());
        v.push(format!("{name}: {what}"));
    }
    v
}

/// Draw the page; `Some` when something on it was clicked.
pub(crate) fn show(ui: &mut egui::Ui, inp: StartInputs<'_>) -> Option<StartPick> {
    let mut pick = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width().min(COLUMN);
            let side = ((ui.available_width() - width) / 2.0).max(0.0);
            ui.horizontal_top(|ui| {
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 24.0;
                    ui.add_space(28.0);
                    heading(ui, inp.show_welcome);
                    if let Some(c) = buttons(ui, inp.keys) {
                        pick = Some(StartPick::Command(c));
                    }
                    if !inp.rows.is_empty() {
                        if let Some(p) = continue_card(ui, inp.rows) {
                            pick = Some(p);
                        }
                    }
                    if inp.show_welcome && welcome_card(ui) {
                        pick = Some(StartPick::HideWelcome);
                    }
                    ui.add_space(16.0);
                });
            });
        });
    pick
}

fn heading(ui: &mut egui::Ui, welcome: bool) {
    let (title, lead) = if welcome {
        (WELCOME, WELCOME_LEAD)
    } else {
        (NOTHING_OPEN, NOTHING_OPEN_LEAD)
    };
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.add(egui::Label::new(egui::RichText::new(title).heading()).selectable(false));
        let tok = crate::theme::tokens::of(ui);
        ui.add(egui::Label::new(egui::RichText::new(lead).color(tok.text2)).selectable(false));
    });
}

fn buttons(ui: &mut egui::Ui, keys: &[rustroom_app::shortcuts::Rebinding]) -> Option<Command> {
    let hint = |c| rustroom_app::shortcuts::hint(c, keys);
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let big = |text: &str, fill: Option<egui::Color32>, colour: Option<egui::Color32>| {
            let mut t = egui::RichText::new(text).size(crate::theme::tokens::TITLE);
            if let Some(c) = colour {
                t = t.color(c);
            }
            let mut b = egui::Button::new(t).min_size(egui::vec2(220.0, 36.0));
            if let Some(f) = fill {
                b = b.fill(f);
            }
            b
        };
        let accent = ui.visuals().selection.bg_fill;
        let on = crate::theme::tokens::on_accent(ui.visuals().dark_mode);
        if ui
            .add(big("Open a folder…", Some(accent), Some(on)))
            .hover(format!(
                "Opens a folder of photos{}",
                hint(Command::OpenFolder)
            ))
            .clicked()
        {
            picked = Some(Command::OpenFolder);
        }
        if ui
            .add(big("Import from a card…", None, None))
            .hover(format!(
                "Copies new photos from a memory card into dated folders{}",
                hint(Command::Import)
            ))
            .clicked()
        {
            picked = Some(Command::Import);
        }
    });
    picked
}

fn card<R>(ui: &mut egui::Ui, margin: egui::Margin, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let tok = crate::theme::tokens::of(ui);
    egui::Frame::new()
        .fill(tok.card)
        .corner_radius(crate::theme::tokens::RADIUS_CARD)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui)
        })
        .inner
}

fn continue_card(ui: &mut egui::Ui, rows: &[ContinueRow]) -> Option<StartPick> {
    let mut pick = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        crate::theme::sub_label(ui, CONTINUE);
        card(ui, egui::Margin::symmetric(10, 8), |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in rows {
                let name = match &row.parent {
                    Some(p) => format!("{}  {p}", row.name),
                    None => row.name.clone(),
                };
                let hover = match row.kind {
                    ContinueKind::AllPhotos => "Opens all your photos".to_string(),
                    ContinueKind::Folder(_) => format!("Opens {}", row.name),
                };
                let out = crate::list_row::row(
                    ui,
                    crate::list_row::RowSpec {
                        name: &name,
                        count: row.count,
                        meta: "",
                        selected: false,
                        indent: 0.0,
                        hover: &hover,
                        actions: &[],
                    },
                );
                if out.clicked {
                    pick = Some(match &row.kind {
                        ContinueKind::AllPhotos => StartPick::AllPhotos,
                        ContinueKind::Folder(p) => StartPick::Folder(p.clone()),
                    });
                }
            }
        });
    });
    pick
}

/// The welcome card; `true` when its box was ticked.
fn welcome_card(ui: &mut egui::Ui) -> bool {
    let mut hide = false;
    card(ui, egui::Margin::same(16), |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        ui.add(
            egui::Label::new(
                egui::RichText::new(WHATS_WHERE).font(crate::theme::title_font(ui.style())),
            )
            .selectable(false),
        );
        let gap = 20.0;
        let half = ((ui.available_width() - gap) / 2.0).max(0.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (label, map, key) in [
                ("Library", Map::Library, &LIBRARY_KEY),
                ("Develop", Map::Develop, &DEVELOP_KEY),
            ] {
                ui.vertical(|ui| {
                    ui.set_width(half);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    crate::theme::sub_label(ui, label);
                    crate::start_page_maps::paint(ui, map, half);
                    for &(n, name, what) in key {
                        key_line(ui, n, name, what);
                    }
                });
            }
        });
        let mut ticked = false;
        if ui
            .checkbox(&mut ticked, DONT_SHOW)
            .hover("Hides this explanation until Settings → Behaviour shows it again")
            .clicked()
        {
            hide = true;
        }
    });
    hide
}

/// One line of a map's key: the number's disc, the part's name, what it is for.
fn key_line(ui: &mut egui::Ui, n: u8, name: &str, what: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        crate::start_page_maps::disc(ui, n);
        let tok = crate::theme::tokens::of(ui);
        let mut job = egui::text::LayoutJob::default();
        let body = egui::TextStyle::Body.resolve(ui.style());
        job.append(name, 0.0, egui::TextFormat::simple(body.clone(), tok.text));
        job.append(
            &format!(": {what}"),
            0.0,
            egui::TextFormat::simple(body, tok.text2),
        );
        job.wrap.max_width = ui.available_width();
        ui.add(egui::Label::new(job).selectable(false));
    });
}

/// Which window a map draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Map {
    Library,
    Develop,
}

#[cfg(test)]
#[path = "start_page_tests.rs"]
mod tests;
