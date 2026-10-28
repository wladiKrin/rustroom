//! File → Tethered capture… (M10.18a): the dialog's camera search, the running session, and each shot
//! as it lands — catalogued, its preset applied, selected, and opened when Develop is showing.
//!
//! ⚠ The camera itself is the `tether` crate's: one thread owns it, and this file only reads its events.

use crate::library_state::LibrarySource;
use crate::{AppView, RustroomApp};
use eframe::egui;
use rustroom_app::ingest::options::ImportOptions;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::presets::Preset;
use rustroom_app::tether_options::{self, TetherOptions};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};
use tether::{Event, Opener, Session};

/// How often the open dialog looks for a camera again.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// The dialog: the options being edited and what the last look found.
pub(crate) struct TetherDialog {
    pub(crate) draft: TetherOptions,
    /// The camera found, by model.
    pub(crate) camera: Option<String>,
    /// Why no camera can be used, when the look said.
    pub(crate) trouble: Option<String>,
    pub(crate) preset_names: Vec<String>,
    look: Option<Receiver<Result<Option<String>, String>>>,
    looked: Option<Instant>,
}

/// A running session.
pub(crate) struct TetherRun {
    session: Session,
    /// The camera's model once it is open.
    pub(crate) camera: Option<String>,
    pub(crate) shots: usize,
    /// Stop was pressed; the session ends after any download in flight.
    pub(crate) stopping: bool,
    /// The last thing that went wrong, shown in the session's line until the next shot lands —
    /// the status row shows only that line while a session runs.
    pub(crate) notice: Option<String>,
    /// A shot that landed while a dialog was open: selected (and opened in Develop) once it
    /// closes, never under a dialog acting on the selection.
    deferred: Option<PhotoId>,
    folder: PathBuf,
    preset: Option<Preset>,
}

#[derive(Default)]
pub(crate) struct TetherState {
    pub(crate) dialog: Option<TetherDialog>,
    pub(crate) run: Option<TetherRun>,
}

/// The first camera on the ports — `None` in a build without camera support.
fn detect() -> Result<Option<String>, String> {
    #[cfg(feature = "camera")]
    return tether::gphoto::detect();
    #[cfg(not(feature = "camera"))]
    Err("This build of Rustroom has no camera support".into())
}

/// Opens the first camera, on the session's thread.
fn real_opener() -> Opener {
    #[cfg(feature = "camera")]
    return tether::gphoto::opener();
    #[cfg(not(feature = "camera"))]
    Box::new(|| Err("This build of Rustroom has no camera support".into()))
}

impl RustroomApp {
    /// File → Tethered capture…
    pub(crate) fn open_tether(&mut self) {
        if self.tether.run.is_some() {
            if let Some(r) = self.tether.run.as_mut() {
                r.notice = Some("Stop tethering before starting another session".into());
            }
            return;
        }
        self.tether.dialog = Some(TetherDialog {
            draft: self.settings.tether.clone(),
            camera: None,
            trouble: None,
            preset_names: self
                .preset_store
                .list()
                .into_iter()
                .map(|p| p.key())
                .collect(),
            look: None,
            looked: None,
        });
        self.modal = crate::modals::Modal::Tether;
    }

    pub(crate) fn close_tether_dialog(&mut self) {
        self.tether.dialog = None;
        self.modal = crate::modals::Modal::None;
    }

    /// While the dialog is open: look for a camera every `LOOK_EVERY`, on a thread of its own.
    pub(crate) fn pump_tether_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = self.tether.dialog.as_mut() else {
            return;
        };
        if let Some(rx) = &d.look {
            match rx.try_recv() {
                Ok(found) => {
                    d.look = None;
                    match found {
                        Ok(c) => {
                            d.camera = c;
                            d.trouble = None;
                        }
                        Err(e) => {
                            d.camera = None;
                            d.trouble = Some(e);
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => d.look = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if d.look.is_none() && d.looked.is_none_or(|t| t.elapsed() >= LOOK_EVERY) {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(detect());
            });
            d.look = Some(rx);
            d.looked = Some(Instant::now());
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    /// The dialog's Start.
    pub(crate) fn start_tether(&mut self, ctx: &egui::Context) {
        let Some(d) = self.tether.dialog.take() else {
            return;
        };
        self.modal = crate::modals::Modal::None;
        self.start_tether_with(ctx, d.draft, real_opener());
    }

    /// Make the session's folder, show it, and start the session on `open`'s camera.
    pub(crate) fn start_tether_with(
        &mut self,
        ctx: &egui::Context,
        opts: TetherOptions,
        open: Opener,
    ) {
        let folder = opts.folder(&tether_options::today());
        if let Err(e) = std::fs::create_dir_all(&folder) {
            self.status = format!("Could not make {}: {e}", folder.display());
            return;
        }
        let preset = opts
            .preset
            .as_ref()
            .and_then(|n| self.preset_store.list().into_iter().find(|p| p.key() == *n));
        // ⚠ Remembered here and only here — the import's rule.
        if self.settings.tether != opts {
            self.settings.tether = opts;
            self.save_settings();
        }
        // ⚠ The folder is OPENED first, so every shot lands as a row of the view.
        self.open_folder_at(ctx, folder.clone());
        self.tether.run = Some(TetherRun {
            session: Session::start(open, folder.clone()),
            camera: None,
            shots: 0,
            stopping: false,
            notice: None,
            deferred: None,
            folder,
            preset,
        });
        self.status = tether_options::status_line(None, 0, false, None);
    }

    /// Shoot (F12): fire the camera's shutter.
    pub(crate) fn shoot_tether(&mut self) {
        match &self.tether.run {
            Some(r) if !r.stopping => r.session.shoot(),
            _ => self.status = "No tethered session is running".into(),
        }
    }

    /// Stop tethering — after any download in flight.
    pub(crate) fn stop_tether(&mut self) {
        if let Some(r) = self.tether.run.as_mut() {
            r.session.stop();
            r.stopping = true;
        }
    }

    /// Once per frame: the session's events.
    pub(crate) fn pump_tether(&mut self, ctx: &egui::Context) {
        self.pump_tether_dialog(ctx);
        let mut landed = Vec::new();
        let mut ended = None;
        if let Some(r) = self.tether.run.as_mut() {
            loop {
                match r.session.events().try_recv() {
                    Ok(Event::Connected(name)) => r.camera = Some(name),
                    Ok(Event::Landed(p)) => landed.push(p),
                    Ok(Event::Failed(s)) => r.notice = Some(s),
                    Ok(Event::Ended(why)) => {
                        ended = Some(why);
                        break;
                    }
                    // ⚠ A thread gone without a last word ends the session, or the row would wait forever.
                    Err(TryRecvError::Disconnected) => {
                        ended = Some((!r.stopping).then(|| "Tethering ended unexpectedly".into()));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
        }
        for p in landed {
            self.land_shot(ctx, &p);
        }
        if matches!(self.modal, crate::modals::Modal::None) {
            if let Some(id) = self.tether.run.as_mut().and_then(|r| r.deferred.take()) {
                self.show_shot(ctx, &id);
            }
        }
        if let Some(why) = ended {
            let shots = self.tether.run.take().map_or(0, |r| r.shots);
            self.status = match why {
                Some(s) => s,
                None => format!("Tethering stopped · {}", tether_options::shots(shots)),
            };
        }
        if self.tether.run.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// One shot is on disk: catalogue it, apply the preset, write XMP, show it — and in Develop,
    /// open it (the newest frame is what is on screen).
    fn land_shot(&mut self, ctx: &egui::Context, path: &Path) {
        let Some(run) = self.tether.run.as_mut() else {
            return;
        };
        run.shots += 1;
        run.notice = None;
        let folder = run.folder.clone();
        let preset = run.preset.clone();
        // ⚠⚠ Scanned into the catalog BEFORE the preset — a catalog write is a no-op for a folder it
        // does not know — and the preset's sidecar written BEFORE the row exists, so no thumbnail
        // renders from a photo without its edits (the import's order).
        let scanned = self.scan_and_relink(&folder);
        if let Some(cat) = &self.catalog {
            // ⚠ The EXIF block only (~0.5 ms), never the whole raw (~113 ms and 31 MB a RAF) — this
            // runs on the UI thread at every shot. The full read is the fallback.
            let capture = match image_decode::quick_look(path) {
                Ok(Some(q)) => q.capture,
                _ => rustroom_app::metadata_scanner::read_capture(path),
            };
            let opts = ImportOptions::default();
            rustroom_app::ingest::entry::apply(cat, path, &capture, &opts, preset.as_ref());
        }
        self.write_xmp_for(&[path.to_path_buf()]);
        let shows = self
            .library
            .as_ref()
            .is_some_and(|l| matches!(&l.source, LibrarySource::Folder(d) if d == &folder));
        if shows {
            let entry = scanned.and_then(|(scan, _)| scan.into_iter().find(|e| e.path == path));
            if let (Some(lib), Some(entry)) = (self.library.as_mut(), entry) {
                if !lib.scan.iter().any(|e| e.path == path) {
                    lib.scan.push(entry);
                }
            }
            let id = PhotoId::original(path.to_path_buf());
            if matches!(self.modal, crate::modals::Modal::None) {
                self.show_shot(ctx, &id);
            } else {
                // ⚠ `None` keeps the selection by id: an Export or Sync dialog acts on it.
                self.rebuild_entries(None);
                if let Some(r) = self.tether.run.as_mut() {
                    r.deferred = Some(id);
                }
            }
        }
    }

    /// Select the shot `id` — and in Develop open it, so the newest frame is what is on screen.
    fn show_shot(&mut self, ctx: &egui::Context, id: &PhotoId) {
        self.rebuild_entries(Some(id));
        if self.view == AppView::Develop {
            let at = self
                .library
                .as_ref()
                .and_then(|l| l.entries.iter().position(|e| e.id == *id));
            if let Some(i) = at {
                self.open_entry(ctx, i);
            }
        }
    }

    /// The status row's line while a session runs, `None` otherwise.
    pub(crate) fn tether_line(&self) -> Option<String> {
        let r = self.tether.run.as_ref()?;
        Some(tether_options::status_line(
            r.camera.as_deref(),
            r.shots,
            r.stopping,
            r.notice.as_deref(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tether::fake::{Fake, Step};

    /// Pump until `done` holds — under the 120 s ceiling against a hang.
    fn pump_until(app: &mut RustroomApp, ctx: &egui::Context, done: impl Fn(&RustroomApp) -> bool) {
        let t = Instant::now();
        while !done(app) {
            assert!(t.elapsed() < Duration::from_secs(120), "the session hung");
            app.pump_tether(ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn opts(dest: &Path) -> TetherOptions {
        TetherOptions {
            destination: dest.to_path_buf(),
            session: "Studio".into(),
            preset: None,
        }
    }

    fn jpg(name: &str) -> Step {
        Step::File(name.into(), b"not really a jpeg".to_vec())
    }

    fn shots(app: &RustroomApp) -> usize {
        app.tether.run.as_ref().map_or(0, |r| r.shots)
    }

    /// ⚠⚠ The whole wiring, headless: start → camera → download → catalogue → row → selection.
    #[test]
    fn a_tethered_session_lands_each_shot_catalogued_and_selected() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("tether-flow-e2e");
        let cam = Fake::new("Fujifilm X-T4", vec![jpg("A.JPG"), jpg("B.JPG")]);
        app.start_tether_with(&ctx, opts(d.path()), cam.opener());
        pump_until(&mut app, &ctx, |a| shots(a) == 2);

        let folder = d.join("Studio");
        assert!(folder.join("A.JPG").is_file() && folder.join("B.JPG").is_file());
        let cat = app.catalog.as_ref().unwrap();
        assert_eq!(cat.photo_count().unwrap(), 2, "both catalogued");
        let lib = app.library.as_ref().expect("the session's folder is open");
        assert!(matches!(&lib.source, LibrarySource::Folder(f) if *f == folder));
        assert_eq!(lib.entries.len(), 2);
        let active = lib
            .selection
            .active()
            .map(|i| lib.entries[i].id.path.clone());
        assert_eq!(active, Some(folder.join("B.JPG")), "the newest is selected");
        assert_eq!(
            app.tether_line().as_deref(),
            Some("Tethered · Fujifilm X-T4 · 2 shots")
        );
        assert_eq!(
            app.settings.tether.session, "Studio",
            "the options were not remembered"
        );
    }

    /// Review focus 3: in Develop the newest shot is opened.
    #[test]
    fn in_develop_the_newest_shot_is_opened() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("tether-flow-develop");
        let cam = Fake::new("X", vec![jpg("A.JPG")]);
        app.start_tether_with(&ctx, opts(d.path()), cam.opener());
        app.view = AppView::Develop;
        pump_until(&mut app, &ctx, |a| shots(a) == 1);
        // ⚠ The decode is debounced and in the background: the photo being opened is `pending`
        // until it lands, the session after.
        let opening = app
            .pending
            .as_ref()
            .map(|p| p.id.clone())
            .or_else(|| app.open_photo());
        assert_eq!(
            opening.map(|id| id.path),
            Some(d.join("Studio").join("A.JPG"))
        );
        assert!(app.view == AppView::Develop);
    }

    /// Review focus 4 and the Stop: the row is freed either way, and says why.
    #[test]
    fn stop_frees_the_status_row() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("tether-flow-stop");
        app.start_tether_with(&ctx, opts(d.path()), Fake::new("X", vec![]).opener());
        pump_until(&mut app, &ctx, |a| {
            a.tether.run.as_ref().is_some_and(|r| r.camera.is_some())
        });
        app.stop_tether();
        assert_eq!(app.tether_line().as_deref(), Some("Stopping tethering"));
        pump_until(&mut app, &ctx, |a| a.tether.run.is_none());
        assert_eq!(app.status, "Tethering stopped · 0 shots");

        let cam = Fake::new(
            "X",
            vec![Step::WaitError("The camera was disconnected".into())],
        );
        app.start_tether_with(&ctx, opts(d.path()), cam.opener());
        pump_until(&mut app, &ctx, |a| a.tether.run.is_none());
        assert_eq!(app.status, "The camera was disconnected");
    }

    /// The review's #1: a failure during a session reaches the status row, which shows only the
    /// session's line while it runs — and the next shot clears it.
    #[test]
    fn a_failure_shows_in_the_session_line_until_the_next_shot() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("tether-flow-failure");
        let cam = Fake::new("X", vec![Step::Broken("A.RAF".into())]);
        app.start_tether_with(&ctx, opts(d.path()), cam.opener());
        pump_until(&mut app, &ctx, |a| {
            a.tether_line()
                .is_some_and(|l| l.contains("Could not download A.RAF"))
        });
        app.shoot_tether();
        pump_until(&mut app, &ctx, |a| shots(a) == 1);
        assert_eq!(app.tether_line().as_deref(), Some("Tethered · X · 1 shot"));
    }

    /// The review's #2: a shot landing behind an open dialog never changes the selection that
    /// dialog acts on — it is selected once the dialog closes.
    #[test]
    fn a_shot_behind_an_open_dialog_waits_to_be_selected() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("tether-flow-modal");
        let cam = Fake::new("X", vec![jpg("A.JPG")]);
        app.start_tether_with(&ctx, opts(d.path()), cam.opener());
        pump_until(&mut app, &ctx, |a| shots(a) == 1);
        let selected = |a: &RustroomApp| {
            let lib = a.library.as_ref().unwrap();
            lib.selection
                .active()
                .map(|i| lib.entries[i].id.path.clone())
        };
        let folder = d.join("Studio");
        assert_eq!(selected(&app), Some(folder.join("A.JPG")), "fixture check");

        app.modal = crate::modals::Modal::ExportSettings;
        app.shoot_tether();
        pump_until(&mut app, &ctx, |a| shots(a) == 2);
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            2,
            "the row is added"
        );
        assert_eq!(
            selected(&app),
            Some(folder.join("A.JPG")),
            "the export's selection moved"
        );

        app.modal = crate::modals::Modal::None;
        app.pump_tether(&ctx);
        assert_eq!(selected(&app), Some(folder.join("SHOT1.JPG")));
    }

    /// The review's #5: a session thread that died is not reported as a Stop.
    #[test]
    fn a_session_that_died_says_so() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("tether-flow-died");
        app.start_tether_with(&ctx, opts(d.path()), Fake::new("X", vec![]).opener());
        app.tether.run.as_mut().unwrap().session = Session::ended();
        app.pump_tether(&ctx);
        assert!(app.tether.run.is_none());
        assert_eq!(app.status, "Tethering ended unexpectedly");
    }

    #[test]
    fn shoot_without_a_session_says_so() {
        let mut app = crate::test_app::for_test();
        app.shoot_tether();
        assert_eq!(app.status, "No tethered session is running");
    }
}
