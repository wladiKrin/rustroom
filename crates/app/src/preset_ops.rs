//! Managing presets (M10.9a): rename, move, update, import, export, and their folders — each one
//! undo step (`ActionKind::PresetEdit`), and nothing ever overwritten.
//!
//! The files are the lib's `preset_store`; this is what the app does around it — the open photo's
//! edits, the status row, the undo log, the system pickers. ⚠ The pickers are thin wrappers: the
//! work is in functions taking paths, which the tests call.

use crate::RustroomApp;
use rustroom_app::action_log::{ActionKind, PresetState};
use rustroom_app::preset_store::{export_path, read_preset_file, unique_name, PresetError};
use rustroom_app::presets::Preset;
use std::path::{Path, PathBuf};

/// What is being renamed in place, and what has been typed so far.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PresetRename {
    Preset {
        folder: Option<String>,
        name: String,
        text: String,
    },
    Folder {
        name: String,
        text: String,
    },
}

/// "1 preset", "3 presets".
fn presets_n(n: usize) -> String {
    if n == 1 {
        "1 preset".to_string()
    } else {
        format!("{n} presets")
    }
}

/// How a refused change reads in the status row.
fn refused(e: &PresetError) -> String {
    match e {
        PresetError::Taken(n) => format!("There is already a “{n}” there"),
        PresetError::EmptyName => "A name is needed".to_string(),
        e => format!("Could not change the preset: {e}"),
    }
}

impl RustroomApp {
    /// Record one preset change, refresh the list, say what happened.
    fn preset_edited(&mut self, before: PresetState, after: PresetState, said: String) {
        let open = self.open_photo();
        self.actions
            .push(ActionKind::PresetEdit { before, after }, open.as_ref());
        self.reload_presets();
        self.status = format!("{said} — Ctrl+Z to undo");
    }

    fn one(p: Preset) -> PresetState {
        PresetState {
            presets: vec![p],
            folders: Vec::new(),
        }
    }

    /// Rename a preset to what was typed, in its folder.
    pub(crate) fn rename_preset(&mut self, folder: Option<&str>, name: &str, typed: &str) {
        let Some(before) = self.preset_store.get(folder, name) else {
            return;
        };
        if typed.trim() == name {
            return;
        }
        match self.preset_store.rename(folder, name, typed) {
            Ok(after) => {
                let said = format!("Renamed “{name}” to “{}”", after.name);
                self.preset_edited(Self::one(before), Self::one(after), said);
            }
            Err(e) => self.status = refused(&e),
        }
    }

    /// Move a preset into `to` (`None`: no folder).
    pub(crate) fn move_preset(&mut self, folder: Option<&str>, name: &str, to: Option<&str>) {
        if folder == to {
            return;
        }
        let Some(before) = self.preset_store.get(folder, name) else {
            return;
        };
        match self.preset_store.move_to(folder, name, to) {
            Ok(after) => {
                let said = match to {
                    Some(f) => format!("Moved “{name}” to “{f}”"),
                    None => format!("Moved “{name}” out of its folder"),
                };
                self.preset_edited(Self::one(before), Self::one(after), said);
            }
            Err(e) => self.status = refused(&e),
        }
    }

    /// Move a preset into a new folder — named "New folder", numbered when taken — and start
    /// renaming that folder. One undo step for the move and the folder.
    pub(crate) fn move_preset_to_new_folder(&mut self, folder: Option<&str>, name: &str) {
        let Some(before) = self.preset_store.get(folder, name) else {
            return;
        };
        let wanted = unique_name(&self.preset_store.folders(), "New folder");
        let made = match self.preset_store.create_folder(&wanted) {
            Ok(f) => f,
            Err(e) => {
                self.status = refused(&e);
                return;
            }
        };
        match self.preset_store.move_to(folder, name, Some(&made)) {
            Ok(after) => {
                let after = PresetState {
                    presets: vec![after],
                    folders: vec![made.clone()],
                };
                let said = format!("Moved “{name}” to a new folder");
                self.preset_edited(Self::one(before), after, said);
                self.preset_rename = Some(PresetRename::Folder {
                    name: made.clone(),
                    text: made,
                });
            }
            Err(e) => {
                self.preset_store.remove_empty_folder(&made);
                self.status = refused(&e);
            }
        }
    }

    /// Update a preset with the open photo's settings — its groups stay what they were.
    pub(crate) fn update_preset(&mut self, folder: Option<&str>, name: &str) {
        let Some(edits) = self.session.as_ref().map(|s| s.edits.clone()) else {
            self.status = "Open a photo to update a preset from".to_string();
            return;
        };
        let Some(before) = self.preset_store.get(folder, name) else {
            return;
        };
        let after = Preset {
            edits,
            ..before.clone()
        };
        if let Err(e) = self.preset_store.save(&after) {
            self.status = refused(&e);
            return;
        }
        let said = format!("Updated “{name}” with this photo's settings");
        self.preset_edited(Self::one(before), Self::one(after), said);
    }

    /// Import preset files into `into` (`None`: no folder). A file that is not a preset is skipped;
    /// a name the folder already has is numbered. Returns how many were imported.
    pub(crate) fn import_presets(&mut self, files: &[PathBuf], into: Option<&str>) -> usize {
        let mut taken: Vec<String> = self
            .preset_store
            .list()
            .into_iter()
            .filter(|p| p.folder.as_deref() == into)
            .map(|p| p.name)
            .collect();
        let mut made: Vec<Preset> = Vec::new();
        let mut skipped = 0;
        for f in files {
            let Some(mut p) = read_preset_file(f) else {
                skipped += 1;
                continue;
            };
            p.name = unique_name(&taken, &p.name);
            p.folder = into.map(str::to_string);
            match self.preset_store.save(&p) {
                Ok(()) => {
                    taken.push(p.name.clone());
                    made.push(p);
                }
                Err(_) => skipped += 1,
            }
        }
        let n = made.len();
        let skipped_said = match skipped {
            0 => String::new(),
            1 => " · 1 file was not a preset".to_string(),
            k => format!(" · {k} files were not presets"),
        };
        if n == 0 {
            self.status = format!("Nothing imported{skipped_said}");
            return 0;
        }
        let said = format!("Imported {}{skipped_said}", presets_n(n));
        let after = PresetState {
            presets: made,
            folders: Vec::new(),
        };
        self.preset_edited(PresetState::default(), after, said);
        n
    }

    /// Make a folder — "New folder", numbered when taken — and start renaming it.
    pub(crate) fn new_preset_folder(&mut self) {
        let wanted = unique_name(&self.preset_store.folders(), "New folder");
        match self.preset_store.create_folder(&wanted) {
            Ok(f) => {
                let after = PresetState {
                    presets: Vec::new(),
                    folders: vec![f.clone()],
                };
                self.preset_edited(PresetState::default(), after, format!("Made “{f}”"));
                self.preset_rename = Some(PresetRename::Folder {
                    name: f.clone(),
                    text: f,
                });
            }
            Err(e) => self.status = refused(&e),
        }
    }

    /// Rename a folder to what was typed; its presets go with it.
    pub(crate) fn rename_preset_folder(&mut self, old: &str, typed: &str) {
        let inside: Vec<Preset> = self
            .preset_store
            .list()
            .into_iter()
            .filter(|p| p.folder.as_deref() == Some(old))
            .collect();
        match self.preset_store.rename_folder(old, typed) {
            Ok(new) if new == old => {}
            Ok(new) => {
                let moved = inside
                    .iter()
                    .cloned()
                    .map(|p| Preset {
                        folder: Some(new.clone()),
                        ..p
                    })
                    .collect();
                let before = PresetState {
                    presets: inside,
                    folders: vec![old.to_string()],
                };
                let after = PresetState {
                    presets: moved,
                    folders: vec![new.clone()],
                };
                self.preset_edited(before, after, format!("Renamed “{old}” to “{new}”"));
            }
            Err(e) => self.status = refused(&e),
        }
    }

    /// Delete a folder: its presets go to the trash, and one undo step brings them all back.
    pub(crate) fn delete_preset_folder(&mut self, name: &str) {
        match self.preset_store.remove_folder(name) {
            Ok(gone) => {
                let n = gone.len();
                let before = PresetState {
                    presets: gone,
                    folders: vec![name.to_string()],
                };
                let said = match n {
                    0 => format!("Deleted “{name}”"),
                    n => format!("Deleted “{name}” and its {}", presets_n(n)),
                };
                self.preset_edited(before, PresetState::default(), said);
            }
            Err(e) => self.status = refused(&e),
        }
    }

    /// Write a preset's file into `dir`, never over a file there. Returns where it went.
    pub(crate) fn export_preset(
        &mut self,
        folder: Option<&str>,
        name: &str,
        dir: &Path,
    ) -> Option<PathBuf> {
        let from = self.preset_store.path_of(folder, name)?;
        let stem = from.file_stem()?.to_string_lossy().into_owned();
        let to = export_path(dir, &stem, "json");
        match std::fs::copy(&from, &to) {
            Ok(_) => {
                self.status = format!("Exported “{name}”");
                Some(to)
            }
            Err(e) => {
                self.status = format!("Could not export “{name}”: {e}");
                None
            }
        }
    }

    /// Write a folder of presets into `dir` as a folder of their files, never over one there.
    pub(crate) fn export_preset_folder(&mut self, name: &str, dir: &Path) -> Option<PathBuf> {
        let to = export_path(dir, name, "");
        let from = self.preset_store.folder_dir(Some(name));
        let files: Vec<PathBuf> = std::fs::read_dir(&from)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        let copied = std::fs::create_dir_all(&to).and_then(|()| {
            files.iter().try_for_each(|f| {
                std::fs::copy(f, to.join(f.file_name().unwrap_or_default())).map(|_| ())
            })
        });
        match copied {
            Ok(()) => {
                self.status = format!("Exported “{name}” with {}", presets_n(files.len()));
                Some(to)
            }
            Err(e) => {
                self.status = format!("Could not export “{name}”: {e}");
                None
            }
        }
    }

    /// Undo (`forward: false`) or redo a [`ActionKind::PresetEdit`]: the side being left goes, the
    /// other comes back.
    pub(crate) fn replay_preset_edit(&mut self, kind: &ActionKind, forward: bool) {
        let ActionKind::PresetEdit { before, after } = kind else {
            return;
        };
        let (leave, enter) = if forward {
            (before, after)
        } else {
            (after, before)
        };
        let store = &self.preset_store;
        for p in &leave.presets {
            let _ = store.remove(p.folder.as_deref(), &p.name);
        }
        for f in &enter.folders {
            let _ = store.ensure_folder(f);
        }
        for p in &enter.presets {
            let _ = store.save(p);
        }
        for f in &leave.folders {
            if !enter.folders.contains(f) {
                store.remove_empty_folder(f);
            }
        }
        self.preset_rename = None;
        self.reload_presets();
        self.status = if forward { "Redone" } else { "Undone" }.to_string();
    }

    /// ⋯ → Import presets…, or a folder's Import presets here…: the system picker, then the import.
    pub(crate) fn import_presets_picked(&mut self, into: Option<String>) {
        let Some(files) = rfd::FileDialog::new()
            .set_title("Import presets")
            .add_filter("Rustroom preset", &["json"])
            .pick_files()
        else {
            return;
        };
        self.import_presets(&files, into.as_deref());
    }

    /// A preset's Export…: the folder picker, then the export.
    pub(crate) fn export_preset_picked(&mut self, folder: Option<String>, name: String) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Export preset to")
            .pick_folder()
        {
            self.export_preset(folder.as_deref(), &name, &dir);
        }
    }

    /// A folder's Export folder…: the folder picker, then the export.
    pub(crate) fn export_preset_folder_picked(&mut self, name: String) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Export folder to")
            .pick_folder()
        {
            self.export_preset_folder(&name, &dir);
        }
    }

    /// Show a preset's file — or, with no name, the presets folder — in the file manager.
    pub(crate) fn reveal_preset(&mut self, folder: Option<&str>, name: Option<&str>) {
        let path = match name {
            Some(n) => self.preset_store.path_of(folder, n),
            None => {
                let _ = std::fs::create_dir_all(self.preset_store.dir());
                Some(self.preset_store.dir().to_path_buf())
            }
        };
        if let Some(p) = path {
            if let Err(e) = rustroom_app::reveal::in_file_manager(&p) {
                self.status = format!("Could not show the file: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{EditGroups, Edits};
    use rustroom_app::menu::Command;

    fn preset(folder: Option<&str>, name: &str, exposure: f32) -> Preset {
        Preset {
            name: name.to_string(),
            groups: EditGroups::only(1),
            edits: Edits {
                exposure,
                ..Edits::default()
            },
            folder: folder.map(str::to_string),
        }
    }

    /// An app whose store holds `A` (no folder) and `Film/Warm`.
    fn app() -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.preset_store.save(&preset(None, "A", 1.0)).unwrap();
        app.preset_store
            .save(&preset(Some("Film"), "Warm", 2.0))
            .unwrap();
        app.reload_presets();
        app
    }

    /// `folder/name` of every preset, then the folders — what the panel would show.
    fn shown(app: &RustroomApp) -> (Vec<String>, Vec<String>) {
        (
            app.preset_list.iter().map(|p| p.key()).collect(),
            app.preset_folders.clone(),
        )
    }

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn run(app: &mut RustroomApp, c: Command) {
        crate::commands::execute(app, &egui::Context::default(), c);
    }

    /// Do `change`, check `then`, undo back to the start, redo to `then` again.
    fn round_trip(
        mut app: RustroomApp,
        change: impl FnOnce(&mut RustroomApp),
        then: (Vec<String>, Vec<String>),
    ) {
        let start = shown(&app);
        change(&mut app);
        assert_eq!(shown(&app), then, "after: {}", app.status);
        run(&mut app, Command::Undo);
        assert_eq!(shown(&app), start, "undone: {}", app.status);
        run(&mut app, Command::Redo);
        assert_eq!(shown(&app), then, "redone: {}", app.status);
    }

    #[test]
    fn a_rename_is_one_undo_step() {
        round_trip(
            app(),
            |a| a.rename_preset(None, "A", "Bright"),
            (keys(&["Bright", "Film/Warm"]), keys(&["Film"])),
        );
    }

    /// ⚠⚠ A taken name is refused: nothing changes and no undo step is left behind.
    #[test]
    fn a_rename_onto_a_taken_name_is_refused() {
        let mut app = app();
        app.preset_store.save(&preset(None, "B", 3.0)).unwrap();
        app.reload_presets();
        app.rename_preset(None, "A", "B");
        assert!(app.status.contains("already"), "{}", app.status);
        assert_eq!(app.preset_store.get(None, "B").unwrap().edits.exposure, 3.0);
        assert!(app.actions.peek_undo(None).is_none(), "no undo step");
    }

    #[test]
    fn a_move_is_one_undo_step() {
        round_trip(
            app(),
            |a| a.move_preset(None, "A", Some("Film")),
            (keys(&["Film/A", "Film/Warm"]), keys(&["Film"])),
        );
    }

    #[test]
    fn a_move_to_a_new_folder_makes_it_and_names_it_next() {
        let mut app = app();
        app.move_preset_to_new_folder(None, "A");
        assert_eq!(
            shown(&app),
            (
                keys(&["Film/Warm", "New folder/A"]),
                keys(&["Film", "New folder"])
            )
        );
        assert!(matches!(
            app.preset_rename,
            Some(PresetRename::Folder { ref name, .. }) if name == "New folder"
        ));
        run(&mut app, Command::Undo);
        assert_eq!(shown(&app), (keys(&["A", "Film/Warm"]), keys(&["Film"])));
    }

    /// Update takes the open photo's settings and keeps the groups the preset carries.
    #[test]
    fn an_update_takes_the_photo_and_keeps_the_groups() {
        let mut app = app();
        let ctx = egui::Context::default();
        crate::test_app::open_test_photo(&mut app, &ctx, 8, 4);
        app.session.as_mut().unwrap().edits.exposure = 0.5;
        app.update_preset(Some("Film"), "Warm");
        let p = app.preset_store.get(Some("Film"), "Warm").unwrap();
        assert_eq!(p.edits.exposure, 0.5, "{}", app.status);
        assert_eq!(p.groups, EditGroups::only(1), "the groups stay");
        run(&mut app, Command::Undo);
        let p = app.preset_store.get(Some("Film"), "Warm").unwrap();
        assert_eq!(p.edits.exposure, 2.0, "undone");
    }

    /// An import numbers a taken name and skips what is not a preset — one undo step.
    #[test]
    fn an_import_numbers_a_taken_name_and_skips_junk() {
        let d = test_dir::TempDir::new("preset-import");
        let good = d.join("A.json");
        std::fs::write(
            &good,
            serde_json::to_string(&preset(None, "A", 9.0)).unwrap(),
        )
        .unwrap();
        let junk = d.join("notes.json");
        std::fs::write(&junk, "{ nope").unwrap();
        let mut app = app();
        assert_eq!(app.import_presets(&[good, junk], None), 1);
        assert!(
            app.status.contains("1 file was not a preset"),
            "{}",
            app.status
        );
        assert_eq!(app.preset_store.get(None, "A").unwrap().edits.exposure, 1.0);
        assert_eq!(
            app.preset_store.get(None, "A 2").unwrap().edits.exposure,
            9.0
        );
        run(&mut app, Command::Undo);
        assert!(!app.preset_store.exists(None, "A 2"));
    }

    #[test]
    fn a_folder_rename_is_one_undo_step() {
        round_trip(
            app(),
            |a| a.rename_preset_folder("Film", "Analog"),
            (keys(&["A", "Analog/Warm"]), keys(&["Analog"])),
        );
    }

    /// ⚠ A folder deleted with its presets comes back whole with one Ctrl+Z.
    #[test]
    fn a_folder_delete_is_one_undo_step() {
        round_trip(
            app(),
            |a| a.delete_preset_folder("Film"),
            (keys(&["A"]), Vec::new()),
        );
    }

    #[test]
    fn a_new_folder_is_numbered_and_one_undo_step() {
        let mut app = app();
        app.new_preset_folder();
        app.new_preset_folder();
        assert_eq!(
            app.preset_folders,
            keys(&["Film", "New folder", "New folder 2"])
        );
        run(&mut app, Command::Undo);
        assert_eq!(app.preset_folders, keys(&["Film", "New folder"]));
    }

    /// Save current… puts the preset in the folder chosen in its dialog.
    #[test]
    fn save_current_goes_into_the_chosen_folder() {
        let mut app = app();
        let ctx = crate::test_ui::ctx();
        crate::test_app::open_test_photo(&mut app, &ctx, 8, 4);
        app.modal = crate::Modal::SavePreset {
            name: "Mine".into(),
            groups: EditGroups::ALL,
            folder: Some("Film".into()),
        };
        // ⚠ One frame first: a window's first frame only sizes it (M10.8d).
        let _ = crate::test_ui::nodes(&ctx, |_| crate::modals::presets::save(&mut app, &ctx));
        crate::test_ui::click(&ctx, "Save", |_| {
            crate::modals::presets::save(&mut app, &ctx)
        });
        assert!(
            app.preset_store.exists(Some("Film"), "Mine"),
            "{}",
            app.status
        );
        assert!(!app.preset_store.exists(None, "Mine"));
    }

    /// Delete folder… asks first; Delete takes the folder and its presets.
    #[test]
    fn deleting_a_folder_asks_and_then_deletes() {
        let mut app = app();
        let ctx = crate::test_ui::ctx();
        app.modal = crate::Modal::ConfirmDeletePresetFolder {
            name: "Film".into(),
            count: 1,
        };
        let draw = |app: &mut RustroomApp| crate::modals::presets::confirm_delete_folder(app, &ctx);
        let _ = crate::test_ui::nodes(&ctx, |_| draw(&mut app));
        crate::test_ui::click(&ctx, "Delete", |_| draw(&mut app));
        assert!(matches!(app.modal, crate::Modal::None));
        assert_eq!(shown(&app), (keys(&["A"]), Vec::new()), "{}", app.status);
    }

    /// ⚠ An export never replaces a file; a folder exports as a folder of its presets.
    #[test]
    fn an_export_never_overwrites() {
        let d = test_dir::TempDir::new("preset-export");
        let mut app = app();
        let first = app.export_preset(None, "A", d.path()).unwrap();
        let second = app.export_preset(None, "A", d.path()).unwrap();
        assert_eq!((first, second), (d.join("A.json"), d.join("A-1.json")));
        let folder = app.export_preset_folder("Film", d.path()).unwrap();
        assert!(folder.join("Warm.json").exists());
        let again = app.export_preset_folder("Film", d.path()).unwrap();
        assert_eq!(again, d.join("Film-1"));
    }
}
