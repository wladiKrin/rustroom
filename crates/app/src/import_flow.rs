//! The Import dialog's state and flow (M9.12): choose a source, survey it, copy the ticked photos,
//! apply metadata and a preset on entry, open the result.
//!
//! ⚠ All the deciding lives in `rustroom_app::ingest` (no egui); this is the wiring. Its state is
//! one `ImportState` behind `RustroomApp::import`, so `main.rs` carries one field, not twelve.

use crate::RustroomApp;
use core_image::Capture;
use eframe::egui;
use rustroom_app::filename_template::Template;
use rustroom_app::ingest::options::ImportOptions;
use rustroom_app::ingest::plan::{self, KnownSet};
use rustroom_app::ingest::source::{self, Candidate, Card};
use rustroom_app::ingest::worker::{CopyItem, ImportEvent, ImportWorker};
use std::path::PathBuf;
use std::time::Duration;

/// One photo on the source, as the grid shows it.
pub(crate) struct ImportItem {
    pub cand: Candidate,
    /// From the survey; `None` until it gets there, or when the file records none.
    pub capture: Option<Capture>,
    pub thumb: Option<egui::TextureHandle>,
    /// Already in the catalog (`KnownSet`), re-decided when the survey brings a capture time.
    pub known: bool,
    pub ticked: bool,
}

/// Everything the open dialog and a running import hold.
pub(crate) struct ImportState {
    /// The dialog's working copy; committed to `Settings.import` only when an import starts.
    pub draft: ImportOptions,
    pub cards: Vec<Card>,
    pub source: Option<PathBuf>,
    pub items: Vec<ImportItem>,
    known: KnownSet,
    survey: Option<ImportWorker>,
    importer: Option<ImportWorker>,
    /// The progress line while copying; empty otherwise.
    pub progress: String,
    copied: Vec<(PathBuf, Capture)>,
    failed: usize,
    backup_failed: usize,
    /// Develop preset names for the combo, read once when the dialog opens.
    pub preset_names: Vec<String>,
}

impl ImportState {
    pub fn importing(&self) -> bool {
        self.importer.is_some()
    }

    pub fn ticked(&self) -> usize {
        self.items.iter().filter(|i| i.ticked).count()
    }

    pub fn ticked_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.ticked)
            .map(|i| i.cand.size)
            .sum()
    }

    /// Where the first ticked photo would land, relative to the destination — the dialog's preview.
    pub fn preview(&self) -> Option<String> {
        let tpl = Template::parse(&self.draft.template).ok()?;
        let first = self.items.iter().find(|i| i.ticked)?;
        let root = PathBuf::new();
        let (taken, camera) = facts(first);
        let p = plan::destination(&root, &first.cand, taken, camera, &tpl, 1, &[]);
        Some(p.display().to_string())
    }

    /// Tick everything, nothing, or only the photos not already imported.
    pub fn tick(&mut self, how: Tick) {
        for i in &mut self.items {
            i.ticked = match how {
                Tick::All => true,
                Tick::None => false,
                Tick::New => !i.known,
            };
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Tick {
    All,
    None,
    New,
}

fn facts(i: &ImportItem) -> (Option<i64>, Option<&str>) {
    let c = i.capture.as_ref();
    (c.and_then(|c| c.taken), c.and_then(|c| c.camera.as_deref()))
}

impl RustroomApp {
    /// File → Import…: detect cards, open the first one found, show the dialog.
    pub(crate) fn open_import(&mut self) {
        let cards = source::cards(&source::default_roots());
        let first = cards.first().map(|c| c.root.clone());
        self.import = Some(ImportState {
            draft: self.settings.import.clone(),
            cards,
            source: None,
            items: Vec::new(),
            known: KnownSet::default(),
            survey: None,
            importer: None,
            progress: String::new(),
            copied: Vec::new(),
            failed: 0,
            backup_failed: 0,
            preset_names: self
                .preset_store
                .list()
                .into_iter()
                .map(|p| p.name)
                .collect(),
        });
        if let Some(dir) = first {
            self.set_import_source(dir);
        }
        self.modal = crate::modals::Modal::Import;
    }

    /// Read `dir`'s photos, mark the ones already in the catalog, and start surveying them.
    pub(crate) fn set_import_source(&mut self, dir: PathBuf) {
        let prints = self
            .catalog
            .as_ref()
            .and_then(|c| c.import_fingerprints().ok())
            .unwrap_or_default();
        let Some(st) = self.import.as_mut() else {
            return;
        };
        st.known = KnownSet::new(&prints);
        st.items = source::scan(&dir)
            .into_iter()
            .map(|cand| {
                // ⚠ By name and size until the survey brings the capture time, which can only
                // turn "new" into "known" — never the reverse (see `pump_import`).
                let known = st.known.contains(&cand, None);
                ImportItem {
                    cand,
                    capture: None,
                    thumb: None,
                    known,
                    ticked: !known,
                }
            })
            .collect();
        // ⚠ Replacing the worker drops the old one, whose `Drop` cancels it.
        st.survey = Some(ImportWorker::survey(
            st.items.iter().map(|i| i.cand.path.clone()).collect(),
        ));
        st.source = Some(dir);
    }

    /// Close the dialog: a survey is dropped (and so cancelled); a running import is cancelled and
    /// the dialog stays until its `Finished` arrives, so nothing it copied goes unapplied.
    pub(crate) fn close_import(&mut self) {
        match self.import.as_ref().and_then(|s| s.importer.as_ref()) {
            Some(w) => w.cancel(),
            None => {
                self.import = None;
                self.modal = crate::modals::Modal::None;
            }
        }
    }

    /// Plan every ticked photo's destination (and backup) and start copying.
    pub(crate) fn start_import(&mut self) {
        let Some(st) = self.import.as_mut() else {
            return;
        };
        if st
            .draft
            .blocker(st.source.as_deref(), st.ticked())
            .is_some()
        {
            return;
        }
        let Ok(tpl) = Template::parse(&st.draft.template) else {
            return;
        };
        let draft = st.draft.clone();
        let mut planned: Vec<PathBuf> = Vec::new();
        let mut planned_backup: Vec<PathBuf> = Vec::new();
        let mut items = Vec::new();
        for (index, it) in st.items.iter().enumerate().filter(|(_, i)| i.ticked) {
            let (taken, camera) = facts(it);
            let seq = items.len() + 1;
            let dest = plan::destination(
                &draft.destination,
                &it.cand,
                taken,
                camera,
                &tpl,
                seq,
                &planned,
            );
            planned.push(dest.clone());
            let backup = draft.backup_on.then(|| {
                let b = plan::destination(
                    &draft.backup,
                    &it.cand,
                    taken,
                    camera,
                    &tpl,
                    seq,
                    &planned_backup,
                );
                planned_backup.push(b.clone());
                b
            });
            items.push(CopyItem {
                index,
                src: it.cand.path.clone(),
                dest,
                backup,
                capture: it.capture.clone(),
            });
        }
        // ⚠ The survey is stopped: the card has one reader at a time, and the copy is the one
        // that matters now.
        st.survey = None;
        st.progress = format!("Importing 0 of {}…", items.len());
        st.importer = Some(ImportWorker::import(items));
        // ⚠ Persisted here and only here — `start_export`'s rule, so a cancelled dialog leaves the
        // remembered options alone.
        if self.settings.import != draft {
            self.settings.import = draft;
            self.save_settings();
        }
    }

    /// Drain both workers once per frame. Called beside `pump_export`.
    pub(crate) fn pump_import(&mut self, ctx: &egui::Context) {
        let Some(st) = self.import.as_mut() else {
            return;
        };
        let mut finished = None;
        if let Some(w) = &st.survey {
            while let Ok(e) = w.events().try_recv() {
                if let ImportEvent::Surveyed {
                    index,
                    capture,
                    thumb,
                } = e
                {
                    let Some(it) = st.items.get_mut(index) else {
                        continue;
                    };
                    it.thumb = thumb.map(|t| {
                        ctx.load_texture(
                            format!("import-{index}"),
                            egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba),
                            egui::TextureOptions::LINEAR,
                        )
                    });
                    let taken = capture.as_ref().and_then(|c| c.taken);
                    // ⚠ Only ever new → known, which unticks; a user's own untick of a new photo
                    // is kept, and nothing the survey learns ticks a photo back on.
                    if !it.known && st.known.contains(&it.cand, taken) {
                        it.known = true;
                        it.ticked = false;
                    }
                    it.capture = capture;
                }
            }
        }
        if let Some(w) = &st.importer {
            while let Ok(e) = w.events().try_recv() {
                match e {
                    ImportEvent::Progress { done, total, name } => {
                        st.progress = format!("Importing {} of {total} — {name}", done + 1);
                    }
                    ImportEvent::Copied {
                        dest,
                        capture,
                        backup_error,
                        ..
                    } => {
                        st.backup_failed += usize::from(backup_error.is_some());
                        st.copied.push((dest, capture));
                    }
                    ImportEvent::Failed { .. } => st.failed += 1,
                    ImportEvent::Finished { cancelled, .. } => finished = Some(cancelled),
                    _ => {}
                }
            }
        }
        let busy = st.survey.is_some() || st.importer.is_some();
        if let Some(cancelled) = finished {
            self.finish_import(ctx, cancelled);
        } else if busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Catalogue what was copied, apply the metadata and preset, write XMP, open it.
    fn finish_import(&mut self, ctx: &egui::Context, cancelled: bool) {
        let Some(st) = self.import.take() else {
            return;
        };
        self.modal = crate::modals::Modal::None;
        let files: Vec<PathBuf> = st.copied.iter().map(|(p, _)| p.clone()).collect();
        let opened = plan::common_dir(&files);
        // ⚠⚠ Scanned into the catalog BEFORE the metadata is applied — every catalog write is a
        // no-op for a folder it does not know — and the preset sidecars are written BEFORE the
        // folder is opened, so no thumbnail can render from a photo that has not got them yet.
        let mut days: Vec<PathBuf> = files
            .iter()
            .filter_map(|f| f.parent().map(|p| p.to_path_buf()))
            .collect();
        days.sort();
        days.dedup();
        for d in &days {
            let _ = self.scan_and_relink(d);
        }
        let preset = st
            .draft
            .preset
            .as_ref()
            .and_then(|n| self.preset_store.list().into_iter().find(|p| &p.name == n));
        if let Some(cat) = &self.catalog {
            for (path, capture) in &st.copied {
                rustroom_app::ingest::entry::apply(cat, path, capture, &st.draft, preset.as_ref());
            }
        }
        self.write_xmp_for(&files);
        if let Some(dir) = opened {
            self.open_folder_at(ctx, dir);
        }
        let mut parts = vec![format!("Imported {}", files.len())];
        if st.failed > 0 {
            parts.push(format!("{} failed", st.failed));
        }
        if st.backup_failed > 0 {
            parts.push(format!("backup failed for {}", st.backup_failed));
        }
        if cancelled {
            parts.push("cancelled".into());
        }
        self.status = parts.join(", ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rustroom_import_flow_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Pump until the import dialog's state is gone, i.e. `finish_import` has run.
    fn run_to_end(app: &mut RustroomApp, ctx: &egui::Context) {
        for _ in 0..500 {
            app.pump_import(ctx);
            if app.import.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the import never finished");
    }

    /// ⚠⚠ The whole wiring, headless: source → survey → plan → copy → catalogue → metadata →
    /// open. Every piece has its own unit test; this is the one that sees them joined, which is
    /// where this project's GUI defects have lived.
    #[test]
    fn an_import_copies_the_ticked_photos_catalogues_them_and_applies_the_metadata() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = temp_dir("e2e");
        let card = d.join("SD/DCIM/100");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("A.JPG"), b"not really a jpeg").unwrap();
        std::fs::write(card.join("B.JPG"), b"nor this").unwrap();

        app.open_import();
        app.set_import_source(d.join("SD"));
        {
            let st = app.import.as_mut().unwrap();
            assert_eq!(st.items.len(), 2, "fixture check: both photos found");
            st.items[1].ticked = false;
            st.draft.destination = d.join("Photos");
            st.draft.backup_on = true;
            st.draft.backup = d.join("Backup");
            st.draft.copyright = "© 2026 W".into();
            st.draft.keywords = "card".into();
        }
        app.start_import();
        run_to_end(&mut app, &ctx);

        // No EXIF in these files, so the day folder comes from the mtime — find it, don't assume.
        let copied: Vec<PathBuf> = walk(&d.join("Photos"));
        assert_eq!(
            copied.len(),
            1,
            "only the ticked photo is copied: {copied:?}"
        );
        assert_eq!(copied[0].file_name().unwrap(), "A.JPG");
        assert_eq!(walk(&d.join("Backup")).len(), 1, "and backed up");
        let cat = app.catalog.as_ref().unwrap();
        let desc = cat.description_of(&copied[0]).unwrap().unwrap();
        assert_eq!(desc.copyright, "© 2026 W");
        assert_eq!(cat.keywords().unwrap()[0].count, 1);
        assert!(app.status.starts_with("Imported 1"), "{}", app.status);
        assert_eq!(
            std::fs::read(card.join("A.JPG")).unwrap(),
            b"not really a jpeg",
            "the card was touched"
        );
        assert_eq!(
            app.settings.import.copyright, "© 2026 W",
            "the options were not remembered"
        );

        // A second import of the same card marks A as already imported.
        app.open_import();
        app.set_import_source(d.join("SD"));
        let st = app.import.as_ref().unwrap();
        let a = st
            .items
            .iter()
            .find(|i| i.cand.path.ends_with("A.JPG"))
            .unwrap();
        assert!(a.known && !a.ticked, "a re-inserted card offered A again");
    }

    fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
