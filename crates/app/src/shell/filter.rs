//! The Library filter: the marks popup, the capture section, its label, and the search field.

use crate::theme::Hover as _;
use crate::RustroomApp;
use eframe::egui;
use rustroom_app::icons;

/// Rating, flag and colour-label filtering, in one popup.
///
/// ⚠ **One control rather than three.** The row it sits in is a right-to-left cluster already
/// carrying the format bullets, the grid-size buttons, the sort combo, Reindex and the status
/// line; three more controls would squeeze the status out — the exact bug that cluster was
/// created to fix.
///
/// ⚠ The button reports the active state, so a filter is never invisible. A control that hides
/// rows while looking idle is the "invisible state" problem the crop aspect lock was fixed for.
///
/// ⚠ Composes with the format filter and the search text through `Library::refilter`, the single
/// derived `visible` list every consumer iterates — not a second filtering mechanism.
pub(super) fn mark_filter(app: &mut RustroomApp, ui: &mut egui::Ui, compact: bool) {
    if app.library.is_none() {
        return;
    }
    let before = app.filter.clone();
    let mut f = before.clone();

    // ⚠⚠ `CloseOnClickOutside`, not the default `CloseOnClick`. Every row here is a checkbox and
    // the whole point is ticking several; a popup that shut on the first tick made setting two
    // filters a four-click job of reopening it.
    let cfg = egui::containers::menu::MenuConfig::default()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    // ⚠ The funnel is prepended at the widget, not in `mark_filter_label`: the label stays a
    // pure, testable sentence and the glyph is its clothing.
    // ⚠ Compact: the funnel alone, a dot beside it while any filter is on — the label is the hover,
    // so a narrow bar still says a filter hides photos.
    let label = mark_filter_label(&f);
    let shown = if !compact {
        format!("{} {label}", icons::FILTER)
    } else if label == "Filter" {
        icons::FILTER.to_string()
    } else {
        format!("{} •", icons::FILTER)
    };
    let (button, _) = egui::containers::menu::MenuButton::new(shown)
        .config(cfg)
        .ui(ui, |ui| {
            // ⚠ Tall enough to need scrolling now that capture values join the marks — the list
            // of lenses grows with the library, unlike the fixed mark sections.
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    // ⚠⚠ **One ordering across all three sections: the unset value first, then the
                    // values.** Flag used to read "Any, picked, rejected, unflagged" while colour read
                    // "Any, unlabelled, colours" — two orderings for one idea. There is no "Any" row any
                    // more either: an empty set *is* "any", which is the same convention in all three and
                    // one fewer thing to explain.
                    ui.label("Rating");
                    for v in 0..=rustroom_app::library_filter::RatingSet::MAX {
                        let text = if v == 0 {
                            "Unrated".to_string()
                        } else {
                            format!("{v}★")
                        };
                        let mut on = f.stars.contains(v);
                        if ui.checkbox(&mut on, text).changed() {
                            f.stars.toggle(v);
                        }
                    }
                    ui.separator();
                    ui.label("Flag");
                    // ⚠ Driven by `FlagSet::ALL`, so the ordering lives beside the type rather than being
                    // re-invented here — the rule `EDIT_GROUP_NAMES` and `Tool::ALL` already follow.
                    for flag in rustroom_app::library_filter::FlagSet::ALL {
                        let text = match flag {
                            catalog::db::Flag::None => "Unflagged",
                            catalog::db::Flag::Pick => "⚑ Picked",
                            catalog::db::Flag::Reject => "× Rejected",
                        };
                        let mut on = f.flags.contains(flag);
                        if ui.checkbox(&mut on, text).changed() {
                            f.flags.toggle(flag);
                        }
                    }
                    ui.separator();
                    ui.label("Colour");
                    for (n, name) in [
                        (0u8, "Unlabelled"),
                        (1, "Red"),
                        (2, "Blue"),
                        (3, "Yellow"),
                        (4, "Green"),
                    ] {
                        let mut on = f.labels.contains(n);
                        if ui.checkbox(&mut on, name).changed() {
                            f.labels.toggle(n);
                        }
                    }
                    ui.separator();
                    // ⚠⚠ Shown only with **more than one** value. Measured: this library has exactly one
                    // camera, so a Camera section would be a single checkbox that can only hide
                    // everything — the same reasoning that hides the format chips below two formats.
                    let cameras = app
                        .catalog
                        .as_ref()
                        .and_then(|c| c.distinct_cameras().ok())
                        .unwrap_or_default();
                    capture_section(ui, "Camera", &cameras, &mut f.cameras);
                    let lenses = app
                        .catalog
                        .as_ref()
                        .and_then(|c| c.distinct_lenses().ok())
                        .unwrap_or_default();
                    capture_section(ui, "Lens", &lenses, &mut f.lenses);
                    ui.separator();
                    if ui.button("Clear").clicked() {
                        f.clear_marks_and_text();
                    }
                });
        });
    button.hover(if compact {
        label.as_str()
    } else {
        "Shows only the photos with the ratings, flags, labels, cameras or lenses you choose"
    });

    // ⚠ Guarded on an actual change: this body runs every frame, and refiltering a folder of
    // hundreds per frame is the `apply_view_demosaic` mistake in another costume.
    if f != before {
        app.filter = f;
        // ⚠ The filter lives on the app now, so refiltering is a separate step rather than a
        // field write — that split is what lets it survive a source switch.
        let filter = app.filter.clone();
        if let Some(lib) = app.library.as_mut() {
            lib.refilter(&filter);
        }
    }
}

/// One multi-select section of open-ended string values.
///
/// ⚠ Drawn only when there is **more than one** value: a single checkbox can only hide everything,
/// which is not a filter. The same guard the format chips use.
fn capture_section(
    ui: &mut egui::Ui,
    title: &str,
    values: &[String],
    chosen: &mut std::collections::BTreeSet<String>,
) {
    if values.len() < 2 {
        return;
    }
    ui.label(title);
    for v in values {
        let mut on = chosen.contains(v);
        if ui.checkbox(&mut on, v).changed() {
            if on {
                chosen.insert(v.clone());
            } else {
                chosen.remove(v);
            }
        }
    }
    ui.separator();
}

/// What the Filter button says.
///
/// ⚠ Pure, so "an active filter is visible" is testable without a window.
pub(super) fn mark_filter_label(f: &rustroom_app::library_filter::LibraryFilter) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !f.stars.is_empty() {
        let list: Vec<String> = f
            .stars
            .ratings()
            .into_iter()
            .map(|r| if r == 0 { "0".into() } else { r.to_string() })
            .collect();
        parts.push(format!("{}★", list.join(",")));
    }
    for flag in f.flags.flags() {
        parts.push(
            match flag {
                catalog::db::Flag::None => "unflagged",
                catalog::db::Flag::Pick => "⚑",
                catalog::db::Flag::Reject => "×",
            }
            .to_string(),
        );
    }
    for c in &f.cameras {
        parts.push(c.clone());
    }
    for l in &f.lenses {
        parts.push(l.clone());
    }
    if !f.labels.is_empty() {
        let names = ["unlabelled", "red", "blue", "yellow", "green"];
        let list: Vec<&str> = f
            .labels
            .labels()
            .into_iter()
            .map(|l| names.get(l as usize).copied().unwrap_or("?"))
            .collect();
        parts.push(list.join(","));
    }
    // ⚠⚠ **Always prefixed.** It read as `3★+` alone when active, so the button stopped naming
    // itself and looked like a different control depending on what was set — reported in use.
    // The prefix is what keeps it recognisably the same button.
    if parts.is_empty() {
        "Filter".to_string()
    } else {
        format!("Filter: {}", parts.join(" "))
    }
}

/// Narrow the Library by name.
///
/// ⚠ Matches `entries::shown_name`, so what you search is what the caption shows — including a
/// copy's own name.
///
/// ⚠ **Needs no new keyboard work, and that is checked rather than assumed.** The Library's bare
/// culling keys (`0`–`9`, `p`, `x`, `u`), its arrow navigation and `pressed_command` are all
/// already gated on `memory().focused()`, which is exactly the gate a text field needs. Escape is
/// added so the keyboard is recoverable without the mouse.
pub(super) fn search_field(app: &mut RustroomApp, ui: &mut egui::Ui, width: f32) {
    if app.library.is_none() {
        return;
    }
    let before = app.filter.text.clone();
    let mut text = before.clone();
    // ⚠ The magnifying glass is in the hint, so it disappears with it once you type — it
    // names the field, it is not a button.
    let r = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(format!("{} Search photos", icons::ZOOM_ACTUAL))
            .desired_width(width),
    );
    // ⚠ Read the key press into a local *before* touching `r`. `Response` methods take the context
    // lock internally, and holding one across `ui.input(..)` panics inside epaint's mutex — the
    // re-entrant lock `name_edit::field` already paid for.
    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escaped && r.has_focus() {
        r.surrender_focus();
    }
    // ⚠ Guarded on a change: the body runs every frame.
    if text != before {
        app.filter.text = text;
        let filter = app.filter.clone();
        if let Some(lib) = app.library.as_mut() {
            lib.refilter(&filter);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_button_reports_an_active_filter() {
        // ⚠ A control that hides rows while looking idle is invisible state — the problem the crop
        // aspect lock was fixed for. This label is the only thing that says a filter is on.
        use rustroom_app::library_filter::LibraryFilter;
        assert_eq!(mark_filter_label(&LibraryFilter::default()), "Filter");
        let mut three = rustroom_app::library_filter::RatingSet::default();
        three.toggle(3);
        let mut unflagged = rustroom_app::library_filter::FlagSet::default();
        unflagged.toggle(catalog::db::Flag::None);
        let mut unlabelled = rustroom_app::library_filter::LabelSet::default();
        unlabelled.toggle(0);
        for f in [
            LibraryFilter {
                stars: three,
                ..Default::default()
            },
            LibraryFilter {
                flags: unflagged,
                ..Default::default()
            },
            LibraryFilter {
                labels: unlabelled,
                ..Default::default()
            },
        ] {
            let label = mark_filter_label(&f);
            assert_ne!(label, "Filter", "an active filter looked idle: {f:?}");
            // ⚠⚠ Always prefixed. It read as `3★+` alone before, so the button stopped naming
            // itself and looked like a different control depending on what was set.
            assert!(
                label.starts_with("Filter: "),
                "the button stopped calling itself Filter: {label:?}"
            );
        }
    }

    #[test]
    fn every_filter_glyph_is_renderable() {
        // ⚠⚠ M4.2a found `✓` and `✕` are in NO bundled font and would have shipped as tofu boxes,
        // and the rail's four candidates were all absent too. Probe, never assume.
        //
        // ⚠ A frame must run before the fonts exist, and `install_fonts` is what makes this ask
        // about the stack production uses rather than egui's defaults.
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for c in ['★', '⚑', '×'] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                "{c} (U+{:04X}) is not in any bundled font and would render as a tofu box",
                c as u32
            );
        }
    }
}
