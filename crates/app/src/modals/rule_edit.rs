//! Editing a smart collection's rule.
//!
//! ⚠⚠ **Sixteen criterion kinds need only SIX widgets**, which is what makes this finite: an
//! operator-plus-number covers Rating, ISO, Aperture, Shutter, Focal length and File size on its
//! own.
//!
//! ⚠ Every dropdown is built from the catalog — the same lists the Filter popup and the panel use
//! — so a value that cannot exist cannot be chosen.

use crate::modals::{dialog, Footer, Modal};
use crate::theme;
use crate::RustroomApp;
use eframe::egui;
use rustroom_app::rule::{Cmp, Criterion};
use rustroom_app::rule_form::{Match, Row};

/// Every kind a new row can be, in the order the dropdown offers them.
///
/// ⚠ A prototype value per kind rather than a parallel name list: the label comes from
/// `Criterion::kind`, so the dropdown and the row header cannot drift.
fn prototypes() -> Vec<Criterion> {
    // ⚠ The dropdown draws these under `Criterion::GROUPS` headings, so this list's own
    // order only decides the order *within* a group.
    vec![
        Criterion::Rating {
            op: Cmp::AtLeast,
            value: 3,
        },
        Criterion::Flag(catalog::db::Flag::Pick),
        Criterion::Label(1),
        Criterion::Text(String::new()),
        Criterion::Collection(0),
        Criterion::Keyword(0),
        Criterion::Folder(std::path::PathBuf::new()),
        Criterion::Format("raf".into()),
        Criterion::Period {
            year: 2026,
            month: None,
            day: None,
        },
        Criterion::Camera(String::new()),
        Criterion::Lens(String::new()),
        Criterion::Iso {
            op: Cmp::AtLeast,
            value: 800,
        },
        Criterion::Aperture {
            op: Cmp::AtMost,
            value: 2.8,
        },
        Criterion::Shutter {
            op: Cmp::AtMost,
            value: 0.02,
        },
        Criterion::FocalLength {
            op: Cmp::AtLeast,
            value: 50.0,
        },
        Criterion::FileSize {
            op: Cmp::AtLeast,
            value: 10 * 1024 * 1024,
        },
    ]
}

fn cmp_label(c: Cmp) -> &'static str {
    match c {
        Cmp::AtLeast => "≥",
        Cmp::Exactly => "=",
        Cmp::AtMost => "≤",
    }
}

/// The operator dropdown shared by every numeric criterion.
fn cmp_picker(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, op: &mut Cmp) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(cmp_label(*op))
        .width(44.0)
        .show_ui(ui, |ui| {
            for c in [Cmp::AtLeast, Cmp::Exactly, Cmp::AtMost] {
                ui.selectable_value(op, c, cmp_label(c));
            }
        });
}

/// A dropdown over a list of strings the catalog knows.
fn string_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    opts: &[String],
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if value.is_empty() {
            "—"
        } else {
            value.as_str()
        })
        .show_ui(ui, |ui| {
            for o in opts {
                ui.selectable_value(value, o.clone(), o);
            }
        });
}

/// A dropdown over named ids — collections, keywords.
fn id_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut i64,
    opts: &[(i64, String)],
) {
    let shown = opts
        .iter()
        .find(|(i, _)| i == value)
        .map(|(_, n)| n.as_str())
        .unwrap_or("—");
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (i, n) in opts {
                ui.selectable_value(value, *i, n);
            }
        });
}

/// Draw one value of one row, and report whether it changed.
fn value_widget(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug + Clone,
    c: &mut Criterion,
    lists: &Lists,
) {
    match c {
        // ⚠ One arm for six criteria — the reason this dialog is finite.
        Criterion::Rating { op, value } => {
            cmp_picker(ui, (id.clone(), "op"), op);
            ui.add(egui::DragValue::new(value).range(0..=5));
            ui.label("★");
        }
        Criterion::Iso { op, value } => {
            cmp_picker(ui, (id.clone(), "op"), op);
            ui.add(egui::DragValue::new(value).range(50..=409_600));
        }
        Criterion::Aperture { op, value } => {
            cmp_picker(ui, (id.clone(), "op"), op);
            ui.add(egui::DragValue::new(value).speed(0.1).range(0.7..=64.0));
            ui.label("f");
        }
        Criterion::Shutter { op, value } => {
            cmp_picker(ui, (id.clone(), "op"), op);
            ui.add(
                egui::DragValue::new(value)
                    .speed(0.001)
                    .range(0.0001..=3600.0),
            );
            ui.label("s");
        }
        Criterion::FocalLength { op, value } => {
            cmp_picker(ui, (id.clone(), "op"), op);
            ui.add(egui::DragValue::new(value).range(1.0..=2000.0));
            ui.label("mm");
        }
        Criterion::FileSize { op, value } => {
            cmp_picker(ui, (id.clone(), "op"), op);
            let mut mb = *value as f64 / (1024.0 * 1024.0);
            if ui
                .add(
                    egui::DragValue::new(&mut mb)
                        .speed(1.0)
                        .range(0.0..=10_000.0),
                )
                .changed()
            {
                *value = (mb * 1024.0 * 1024.0) as i64;
            }
            ui.label("MB");
        }
        Criterion::Flag(f) => {
            egui::ComboBox::from_id_salt((id.clone(), "flag"))
                .selected_text(match f {
                    catalog::db::Flag::Pick => "⚑ Picked",
                    catalog::db::Flag::Reject => "× Rejected",
                    catalog::db::Flag::None => "Unflagged",
                })
                .show_ui(ui, |ui| {
                    for v in rustroom_app::library_filter::FlagSet::ALL {
                        let text = match v {
                            catalog::db::Flag::Pick => "⚑ Picked",
                            catalog::db::Flag::Reject => "× Rejected",
                            catalog::db::Flag::None => "Unflagged",
                        };
                        ui.selectable_value(f, v, text);
                    }
                });
        }
        Criterion::Label(n) => {
            const NAMES: [&str; 5] = ["Unlabelled", "Red", "Blue", "Yellow", "Green"];
            egui::ComboBox::from_id_salt((id.clone(), "label"))
                .selected_text(*NAMES.get(*n as usize).unwrap_or(&"—"))
                .show_ui(ui, |ui| {
                    for (i, name) in NAMES.iter().enumerate() {
                        ui.selectable_value(n, i as u8, *name);
                    }
                });
        }
        Criterion::Text(t) => {
            ui.add(egui::TextEdit::singleline(t).desired_width(160.0));
        }
        Criterion::Camera(v) => string_picker(ui, (id.clone(), "cam"), v, &lists.cameras),
        Criterion::Lens(v) => string_picker(ui, (id.clone(), "lens"), v, &lists.lenses),
        Criterion::Format(v) => string_picker(ui, (id.clone(), "fmt"), v, &lists.formats),
        Criterion::Collection(v) => id_picker(ui, (id.clone(), "coll"), v, &lists.collections),
        Criterion::Keyword(v) => id_picker(ui, (id.clone(), "kw"), v, &lists.keywords),
        Criterion::Folder(p) => {
            let mut cur = p.to_string_lossy().to_string();
            let before = cur.clone();
            string_picker(ui, (id.clone(), "folder"), &mut cur, &lists.folders);
            if cur != before {
                *p = std::path::PathBuf::from(cur);
            }
        }
        Criterion::Period { year, month, day } => {
            ui.add(egui::DragValue::new(year).range(1900..=2200));
            let mut m = month.unwrap_or(0);
            if ui
                .add(egui::DragValue::new(&mut m).range(0..=12).prefix("m"))
                .changed()
            {
                *month = (m > 0).then_some(m);
            }
            let mut d = day.unwrap_or(0);
            if ui
                .add(egui::DragValue::new(&mut d).range(0..=31).prefix("d"))
                .changed()
            {
                *day = (d > 0).then_some(d);
            }
            // ⚠ 0 means "any", so a day without a month is meaningless — the criterion ignores
            // `day` unless `month` is set, and saying so beats a silently inert field.
            if day.is_some() && month.is_none() {
                ui.label(egui::RichText::new("needs a month").weak());
            }
        }
    }
}

/// Everything the dropdowns offer, read once when the dialog opens.
#[derive(Default, Clone, PartialEq)]
pub struct Lists {
    pub cameras: Vec<String>,
    pub lenses: Vec<String>,
    pub formats: Vec<String>,
    pub folders: Vec<String>,
    pub collections: Vec<(i64, String)>,
    pub keywords: Vec<(i64, String)>,
}

impl Lists {
    /// ⚠ Read **once**, when the dialog opens — a dialog body runs sixty times a second, and this
    /// is six catalog queries. The `apply_view_demosaic` rule, now cited nine times in CLAUDE.md.
    pub fn read(app: &RustroomApp) -> Self {
        let Some(cat) = &app.catalog else {
            return Self::default();
        };
        Self {
            cameras: cat.distinct_cameras().unwrap_or_default(),
            lenses: cat.distinct_lenses().unwrap_or_default(),
            formats: vec!["raf".into(), "jpg".into(), "png".into(), "tif".into()],
            folders: cat
                .folders()
                .unwrap_or_default()
                .into_iter()
                .map(|f| f.path.to_string_lossy().to_string())
                .collect(),
            collections: cat
                .collections()
                .unwrap_or_default()
                .into_iter()
                .map(|c| (c.id, c.name))
                .collect(),
            keywords: cat
                .keywords()
                .unwrap_or_default()
                .into_iter()
                .map(|k| (k.id, k.name))
                .collect(),
        }
    }
}

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::EditRule {
        id,
        name,
        form,
        lists,
        unsupported,
    } = &mut app.modal
    else {
        return;
    };
    let (id, name) = (*id, name.clone());
    let unsupported = *unsupported;
    let mut form = form.clone();
    let lists = lists.clone();
    let mut save = false;
    let mut cancel = false;

    let out = dialog(
        ctx,
        &mut app.settings,
        "edit_rule",
        &format!("Rule for “{name}”"),
        theme::DialogSize::Wide,
        Footer::confirm("Save", !unsupported, "Cancel"),
        |ui| {
            if unsupported {
                // ⚠⚠ Declining to edit, never starting from empty. An empty form that replaced a
                // working rule on Save is exactly the data loss the refusal exists to prevent.
                crate::theme::hint(
                    ui,
                    "This rule uses nesting the editor cannot show, so it is left alone. \
                 Edit it in the catalog's JSON, or delete it and save a new one.",
                );
                return;
            }

            ui.horizontal(|ui| {
                ui.label("Match");
                egui::ComboBox::from_id_salt("match")
                    .selected_text(form.match_.label())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for m in Match::ALL {
                            ui.selectable_value(&mut form.match_, m, m.label());
                        }
                    });
                ui.label("of:");
            });
            ui.separator();

            let mut drop_row: Option<usize> = None;
            for (ri, row) in form.rows.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    if ui
                        .button("×")
                        .on_hover_text("Removes this condition")
                        .clicked()
                    {
                        drop_row = Some(ri);
                    }
                    // The kind dropdown replaces every value with a prototype of the new kind.
                    let current = row
                        .values
                        .first()
                        .map(|c| c.kind())
                        .unwrap_or("—")
                        .to_string();
                    egui::ComboBox::from_id_salt(("kind", ri))
                        .selected_text(&current)
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            // ⚠⚠ Grouped under headings — sixteen flat entries is more than
                            // a dropdown can be scanned for. ⚠ The groups come from
                            // `Criterion::group`, beside the criteria themselves, so a new
                            // criterion cannot land in a heading the editor does not draw.
                            let protos = prototypes();
                            for g in Criterion::GROUPS {
                                let mut first = true;
                                for proto in protos.iter().filter(|p| p.group() == g) {
                                    if first {
                                        if g != Criterion::GROUPS[0] {
                                            ui.separator();
                                        }
                                        ui.label(egui::RichText::new(g).weak().small());
                                        first = false;
                                    }
                                    if ui
                                        .selectable_label(proto.kind() == current, proto.kind())
                                        .clicked()
                                    {
                                        // ⚠ Changing the kind resets the values: a Rating
                                        // cannot carry a Camera's string, and keeping a stale
                                        // value would be a criterion nobody chose.
                                        row.values = vec![proto.clone()];
                                    }
                                }
                            }
                        });
                });
                let mut drop_val: Option<usize> = None;
                for (vi, c) in row.values.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        if vi > 0 {
                            ui.label(egui::RichText::new("or").weak());
                        }
                        value_widget(ui, ("v", ri, vi), c, &lists);
                        if row_len_gt_one(vi) && ui.button("×").clicked() {
                            drop_val = Some(vi);
                        }
                    });
                }
                if let Some(vi) = drop_val {
                    row.values.remove(vi);
                }
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    // ⚠ A second value on a row is what makes `Any([Is, Is])` reachable from the
                    // UI, rather than only from a captured filter.
                    if ui.small_button("+ or").clicked() {
                        if let Some(last) = row.values.last().cloned() {
                            row.values.push(last);
                        }
                    }
                });
                ui.separator();
            }
            if let Some(ri) = drop_row {
                form.rows.remove(ri);
            }
            if ui.button("+ Add criterion").clicked() {
                form.rows.push(Row {
                    values: vec![prototypes()[0].clone()],
                });
            }
        },
    );
    save |= out.primary;
    cancel |= out.dismissed;

    if save {
        app.save_rule(ctx, id, &name, &form);
        app.modal = Modal::None;
    } else if cancel {
        app.modal = Modal::None;
    } else if let Modal::EditRule { form: f, .. } = &mut app.modal {
        // ⚠ Written back so edits survive the frame — the dialog draws from a clone because the
        // closure cannot hold `app.modal` mutably while `dialog` also needs `ctx`.
        *f = form;
    }
}

/// ⚠ A row's only value has no `×`: removing it would leave an empty row, which `to_expr` drops
/// silently. Removing the *row* is what the row's own `×` is for.
fn row_len_gt_one(index: usize) -> bool {
    index > 0
}
