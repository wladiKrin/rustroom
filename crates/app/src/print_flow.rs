//! File → Print… in the app (M10.18e): the export into a staging folder, then the pages assembled and written
//! or sent to the printer on their own thread — `rustroom_app::print` holds every rule.
//!
//! ⚠ One print at a time; the staging folder is deleted whatever the outcome.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::batch_export::{ExportEvent, ExportReport, ExportWorker};
use rustroom_app::photo_id::PhotoId;
use rustroom_app::print::job::{self, Delivered};
use rustroom_app::print::layout::PrintSettings;
use rustroom_app::print::printer::{self, Runner};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

/// Where a print goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Pdf(PathBuf),
    Printer(String),
}

/// The print half of the app's state.
#[derive(Default)]
pub(crate) struct PrintFlow {
    run: Option<Run>,
    /// CUPS, or a test's stand-in. `None` is the system's.
    pub(crate) runner: Option<Arc<dyn Runner + Send + Sync>>,
    /// Where staging folders go; `None` is the cache directory. A test's own folder in tests.
    pub(crate) staging_root: Option<PathBuf>,
}

/// A running print.
struct Run {
    worker: Option<ExportWorker>,
    settings: PrintSettings,
    photos: Vec<(PhotoId, String)>,
    staging: PathBuf,
    to: Target,
    progress: String,
    /// The pages being assembled and delivered, on their own thread: the status line's sentence.
    delivering: Option<Receiver<Result<String, String>>>,
}

impl PrintFlow {
    pub(crate) fn runner(&self) -> Arc<dyn Runner + Send + Sync> {
        self.runner
            .clone()
            .unwrap_or_else(|| Arc::new(printer::System))
    }

    /// The status line's progress, while a print runs.
    pub(crate) fn line(&self) -> Option<String> {
        self.run.as_ref().map(|r| r.progress.clone())
    }

    pub(crate) fn running(&self) -> bool {
        self.run.is_some()
    }
}

/// A unique staging folder under `root`.
fn staging_in(root: PathBuf) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    root.join(format!("print-{}-{stamp}", std::process::id()))
}

impl RustroomApp {
    /// The photos a print takes, each with its caption — the selection, or the open photo — in grid order.
    pub(crate) fn print_photos(&self) -> Vec<(PhotoId, String)> {
        self.export_jobs()
            .into_iter()
            .map(|j| {
                let caption = j.name.clone().unwrap_or_else(|| {
                    j.id.path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                });
                (j.id, caption)
            })
            .collect()
    }

    /// File → Print…: the dialog on the selection, the printers read once.
    pub(crate) fn open_print(&mut self) {
        let photos = self.print_photos();
        if photos.is_empty() {
            self.status = "Nothing selected to print".into();
            return;
        }
        let printers = printer::printers(self.print.runner().as_ref());
        crate::modals::print::open(self, photos, printers);
    }

    /// Start a print of `photos` with `settings`, to `to`.
    pub(crate) fn start_print(
        &mut self,
        settings: PrintSettings,
        photos: Vec<(PhotoId, String)>,
        to: Target,
    ) {
        if self.print.running() {
            self.status = "A print is already running".into();
            return;
        }
        // ⚠ The last-used settings, written only here (`start_export`'s rule), so Cancel keeps them.
        if self.settings.print != settings {
            self.settings.print = settings.clone();
            self.save_settings();
        }
        let ids: Vec<PhotoId> = photos.iter().map(|(id, _)| id.clone()).collect();
        let jobs = self.export_jobs_for(&ids);
        if jobs.is_empty() {
            self.status = "Nothing to print".into();
            return;
        }
        let root = self.print.staging_root.clone().unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("rustroom")
                .join("print")
        });
        let staging = staging_in(root);
        if let Err(e) = std::fs::create_dir_all(&staging) {
            self.status = format!("Could not print: {e}");
            return;
        }
        let opts = job::export_options(&settings, &staging, self.settings.export.demosaic);
        let demosaic = opts.demosaic;
        let profile = self.camera_profile_path();
        let config = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let total = jobs.len();
        let worker = ExportWorker::spawn(
            jobs,
            opts,
            move |p: &std::path::Path| {
                crate::decode::decode_for_export(p, demosaic, profile.clone())
            },
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rustroom_app::batch_export::ExportRender {
                ai_subject: self.settings.ai_subject_model,
                resampler: self.settings.resampler,
                gpu_tail: self.settings.gpu_tail,
                denoise: rustroom_app::denoised::DenoiseUse::Compute,
                network: self.settings.denoise_network,
            },
            self.settings.export_width(),
            rustroom_app::batch_export::ExportDirs {
                looks: Some(
                    rustroom_app::looks::LookStore::user_default(&config)
                        .dir()
                        .to_path_buf(),
                ),
                // ⚠ No watermark on a print, so no store to read one from.
                watermarks: None,
            },
            None,
        );
        self.print.run = Some(Run {
            worker: Some(worker),
            settings,
            photos,
            staging,
            to,
            progress: format!("Preparing print 0 of {total}…"),
            delivering: None,
        });
    }

    /// Stop the running print; nothing is printed or saved.
    pub(crate) fn cancel_print(&mut self) {
        if let Some(run) = self.print.run.take() {
            if let Some(w) = &run.worker {
                w.cancel();
            }
            // ⚠ A delivery already under way finishes on its own thread; only the export can be stopped.
            if run.delivering.is_none() {
                let _ = std::fs::remove_dir_all(&run.staging);
            }
        }
        self.status = "Print cancelled".into();
    }

    /// Once per frame, beside `pump_export`.
    pub(crate) fn pump_print(&mut self, ctx: &egui::Context) {
        if self.print.run.is_none() {
            return;
        }
        let runner = self.print.runner();
        let Some(run) = &mut self.print.run else {
            return;
        };
        let mut finished: Option<ExportReport> = None;
        if let Some(w) = &run.worker {
            while let Ok(ev) = w.events().try_recv() {
                match ev {
                    ExportEvent::Item { done, total, name } => {
                        run.progress = format!("Preparing print {done} of {total} — {name}");
                    }
                    ExportEvent::Denoising { name } => {
                        run.progress = format!("Denoising {name} for the print — minutes a photo…");
                    }
                    ExportEvent::Finished(r) => finished = Some(r),
                    // ⚠ Staged names are numbered, so no collision is ever asked; nothing is uploaded.
                    ExportEvent::Started { .. }
                    | ExportEvent::Collision { .. }
                    | ExportEvent::UploadUnconfirmed { .. } => {}
                }
            }
        }
        if let Some(report) = finished {
            run.worker = None;
            run.progress = match &run.to {
                Target::Pdf(_) => "Writing the PDF…".into(),
                Target::Printer(p) => format!("Sending to {p}…"),
            };
            run.delivering = Some(deliver(
                run.settings.clone(),
                run.photos.clone(),
                report,
                run.staging.clone(),
                run.to.clone(),
                runner,
            ));
        }
        let landed = run.delivering.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = landed {
            self.print.run = None;
            self.status = match result {
                Ok(s) => s,
                Err(e) => format!("Could not print: {e}"),
            };
        }
        if self.print.running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

/// Assemble the pages and deliver them, on a thread; the staging folder is deleted after.
fn deliver(
    settings: PrintSettings,
    photos: Vec<(PhotoId, String)>,
    report: ExportReport,
    staging: PathBuf,
    to: Target,
    runner: Arc<dyn Runner + Send + Sync>,
) -> Receiver<Result<String, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let written = report
                .written
                .iter()
                .filter_map(|w| Some((w.id.clone(), w.file.clone()?)))
                .collect();
            let a = job::assemble(&settings, &photos, &written)?;
            let to = match to {
                Target::Pdf(file) => {
                    std::fs::write(&file, &a.pdf)
                        .map_err(|e| format!("could not write {}: {e}", file.display()))?;
                    Delivered::Saved { file }
                }
                Target::Printer(name) => {
                    let file = staging.join("print.pdf");
                    std::fs::write(&file, &a.pdf).map_err(|e| e.to_string())?;
                    printer::send(
                        runner.as_ref(),
                        &name,
                        settings.copies,
                        settings.paper,
                        &file,
                    )?;
                    Delivered::Printed { printer: name }
                }
            };
            Ok(job::outcome(
                &to,
                &a,
                report.failed.len(),
                report.below_ppi,
                settings.ppi,
            ))
        })();
        // ⚠ `lp` has copied the file into its spool before it returns, so the folder can go.
        let _ = std::fs::remove_dir_all(&staging);
        let _ = tx.send(result);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::Modal;
    use rustroom_app::print::layout::Layout;
    use std::sync::Mutex;

    /// A constructed CUPS: one printer, and every `lp` call recorded.
    #[derive(Default)]
    struct Cups {
        printers: Vec<String>,
        asked: Mutex<Vec<Vec<String>>>,
    }

    impl Runner for Cups {
        fn run(&self, prog: &str, args: &[String]) -> std::io::Result<(bool, String)> {
            match prog {
                "lpstat" if self.printers.is_empty() => {
                    Err(std::io::Error::from(std::io::ErrorKind::NotFound))
                }
                "lpstat" => Ok((true, self.printers.join("\n"))),
                _ => {
                    self.asked.lock().unwrap().push(args.to_vec());
                    Ok((true, "request id is Canon-1".into()))
                }
            }
        }
    }

    /// An app whose Library shows three real JPEGs (constructed), all selected, printing through `cups` and
    /// staging under the test's own folder.
    fn app_with_three_photos(dir: &std::path::Path, cups: Arc<Cups>) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        let names = ["a.jpg", "b.jpg", "c.jpg"];
        let mut lib = crate::test_app::library_of_names(&names);
        for (i, e) in lib.entries.iter_mut().enumerate() {
            let p = dir.join(names[i]);
            let (w, h) = if i == 1 { (40, 60) } else { (60, 40) };
            image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, 80]))
                .save(&p)
                .unwrap();
            e.id.path = p;
        }
        lib.selection.click(0);
        lib.selection.toggle(1);
        lib.selection.toggle(2);
        app.library = Some(lib);
        app.print.runner = Some(cups);
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        app.print.staging_root = Some(staging);
        app
    }

    /// Pump until the print lands, under the 120 s ceiling against a hang.
    fn wait(app: &mut RustroomApp) {
        let ctx = crate::test_ui::ctx();
        let until = std::time::Instant::now() + Duration::from_secs(120);
        while app.print.running() {
            assert!(
                std::time::Instant::now() < until,
                "the print never finished"
            );
            app.pump_print(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_print_command_opens_the_dialog_on_the_selection() {
        let dir = test_dir::TempDir::new("print-open");
        let mut app = app_with_three_photos(dir.path(), Arc::new(Cups::default()));
        let ctx = crate::test_ui::ctx();
        crate::commands::execute(&mut app, &ctx, rustroom_app::menu::Command::Print);
        let Modal::Print(d) = &app.modal else {
            panic!("the dialog did not open");
        };
        let captions: Vec<&str> = d.photos.iter().map(|(_, c)| c.as_str()).collect();
        assert_eq!(captions, ["a.jpg", "b.jpg", "c.jpg"]);
        assert!(d.printers.is_empty(), "no lpstat, no printer");
    }

    #[test]
    fn with_no_printer_the_dialog_saves_a_pdf_and_with_one_it_prints() {
        let dir = test_dir::TempDir::new("print-button");
        let ctx = crate::test_ui::ctx();
        let mut app = app_with_three_photos(dir.path(), Arc::new(Cups::default()));
        app.open_print();
        let labels = |app: &mut RustroomApp| -> Vec<String> {
            crate::test_ui::labelled_centres(&ctx, Vec::new(), |_| {
                crate::modals::print::show(app, &ctx)
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect()
        };
        let l = labels(&mut app);
        assert!(l.iter().any(|s| s == "Save PDF…"), "{l:?}");
        assert!(!l.iter().any(|s| s == "Print"), "{l:?}");
        let cups = Arc::new(Cups {
            printers: vec!["Canon_G620".into()],
            ..Cups::default()
        });
        app.print.runner = Some(cups);
        app.settings.print.printer = Some("Canon_G620".into());
        app.open_print();
        let l = labels(&mut app);
        assert!(l.iter().any(|s| s == "Print"), "{l:?}");
    }

    #[test]
    fn saving_a_contact_sheet_writes_its_pages_and_clears_the_staging_folder() {
        let dir = test_dir::TempDir::new("print-save");
        let mut app = app_with_three_photos(dir.path(), Arc::new(Cups::default()));
        let settings = PrintSettings {
            layout: Layout::ContactSheet,
            columns: 2,
            rows: 1,
            ..PrintSettings::default()
        };
        let out = dir.path().join("sheet.pdf");
        let photos = app.print_photos();
        app.start_print(settings.clone(), photos, Target::Pdf(out.clone()));
        wait(&mut app);
        // ⚠ 60-px photos cannot reach 300 PPI on paper — the note is the wiring under test too.
        assert_eq!(
            app.status,
            "Saved 2 pages to sheet.pdf, 3 photos print below 300 PPI"
        );
        let pdf = std::fs::read(&out).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 2"));
        assert_eq!(app.settings.print, settings, "the settings are remembered");
        let left = std::fs::read_dir(dir.path().join("staging"))
            .unwrap()
            .count();
        assert_eq!(left, 0, "the staging folder is deleted");
    }

    #[test]
    fn printing_hands_the_pdf_to_lp() {
        let dir = test_dir::TempDir::new("print-lp");
        let cups = Arc::new(Cups {
            printers: vec!["Canon_G620".into()],
            ..Cups::default()
        });
        let mut app = app_with_three_photos(dir.path(), cups.clone());
        let settings = PrintSettings {
            copies: 2,
            ..PrintSettings::default()
        };
        let photos = app.print_photos();
        app.start_print(settings, photos, Target::Printer("Canon_G620".into()));
        wait(&mut app);
        assert_eq!(
            app.status,
            "Printed 3 photos on 3 pages to Canon_G620, 3 photos print below 300 PPI"
        );
        let asked = cups.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(
            asked[0][..6],
            ["-d", "Canon_G620", "-n", "2", "-o", "media=A4"]
        );
        assert!(asked[0][7].ends_with("print.pdf"));
    }
}
