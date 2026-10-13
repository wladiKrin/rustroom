//! Send marks to Immich, and undo the last send (M10.15d) — the window's half. Every decision is
//! `rustroom_app::immich_send`'s; this module asks, waits and records.
//!
//! ⚠⚠ **Nothing reaches the server except from the confirmation's button** — `confirm_send` and
//! `confirm_undo` are the only callers of `Job::SendMarks`, and only the two dialogs call them.

use crate::{LibrarySource, Modal, RustroomApp};
use rustroom_app::immich_list_cache as list_cache;
use rustroom_app::immich_send::{self, Candidate, Change, Group, ServerMarks};
use rustroom_app::immich_source;
use rustroom_app::immich_worker::{Job, MarksSource};

/// A send or an undo whose reply has not come back: its groups, and — for a send — each changed
/// photo's value before it, which is what the record keeps for an undo. (Which of the two it is
/// comes back with the reply.)
pub(crate) struct PendingSend {
    pub groups: Vec<Group>,
    pub before: Vec<(String, ServerMarks)>,
}

/// What the server says when a key may not do this — rewritten to say which permission is missing.
const REFUSED: &str = "the server rejected the API key";

fn photos(n: usize) -> String {
    if n == 1 {
        "1 photo".to_string()
    } else {
        format!("{n} photos")
    }
}

impl RustroomApp {
    /// The Write key's connection — the upload key, which `asset.update` makes able to change marks.
    fn write_connection(&self) -> Option<immich_source::Connection> {
        rustroom_app::immich_upload::config(&self.settings).map(|c| immich_source::Connection {
            url: c.base_url,
            key: c.key,
        })
    }

    /// The open album or month, if a send can act on it.
    fn marks_source(&self) -> Option<MarksSource> {
        match &self.library.as_ref()?.source {
            LibrarySource::ImmichAlbum { id, .. } => Some(MarksSource::Album(id.clone())),
            LibrarySource::ImmichBucket { bucket, .. } => Some(MarksSource::Bucket(bucket.clone())),
            _ => None,
        }
    }

    /// **Send marks**: fetch the open album's or month's current list — never the cache — so the
    /// confirmation and the recorded prior values are the server's real ones.
    pub(crate) fn begin_send_marks(&mut self) {
        if self.write_connection().is_none() {
            self.status = "Send marks needs a Write key — set one in Settings → Immich".to_string();
            return;
        }
        let Some(source) = self.marks_source() else {
            return;
        };
        let paths: Vec<std::path::PathBuf> = self
            .library
            .as_ref()
            .map(|l| {
                l.selection
                    .iter()
                    .filter_map(|i| l.entries.get(i))
                    .map(|e| e.id.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let ids = immich_send::selected_asset_ids(&paths);
        if ids.is_empty() {
            return;
        }
        self.immich_send_selection = ids;
        self.status = "Checking Immich…".to_string();
        self.immich.request(Job::CheckMarks { source });
    }

    /// The check's reply: plan the send and ask — or say there is nothing to send.
    pub(crate) fn on_marks_checked(
        &mut self,
        source: MarksSource,
        result: Result<Vec<immich::Asset>, String>,
    ) {
        let assets = match result {
            Ok(a) => a,
            Err(e) => {
                self.status = format!("Could not check Immich: {e}");
                return;
            }
        };
        let (Some(cat), Some(conn)) = (&self.catalog, immich_source::connection(&self.settings))
        else {
            return;
        };
        let root = immich_source::cache_root(&conn.url);
        let by_id: std::collections::HashMap<&str, &immich::Asset> =
            assets.iter().map(|a| (a.id.as_str(), a)).collect();
        let candidates: Vec<Candidate> = self
            .immich_send_selection
            .iter()
            .filter_map(|id| by_id.get(id.as_str()))
            .map(|a| Candidate {
                asset_id: a.id.clone(),
                name: a.original_file_name.clone(),
                mine: cat
                    .marks(&immich_source::cache_path(&root, a), 0)
                    .unwrap_or_default(),
                server: immich_send::server_marks_of(a),
            })
            .collect();
        let changes = immich_send::plan(&candidates);
        if changes.is_empty() {
            self.status = "Immich already has these marks".to_string();
            return;
        }
        self.status.clear();
        self.immich_send_source = Some(source);
        self.modal = Modal::SendMarks { changes };
    }

    /// The confirmation's **Send** — the only way a send reaches the server.
    pub(crate) fn confirm_send(&mut self, changes: Vec<Change>) {
        let Some(conn) = self.write_connection() else {
            self.status = "Send marks needs a Write key — set one in Settings → Immich".to_string();
            return;
        };
        let groups = immich_send::groups(&changes);
        self.immich_send_pending = Some(PendingSend {
            groups: groups.clone(),
            before: changes
                .iter()
                .map(|c| (c.asset_id.clone(), c.before))
                .collect(),
        });
        self.status = "Sending marks to Immich…".to_string();
        self.immich.request(Job::SendMarks {
            conn,
            groups,
            undo: false,
        });
    }

    /// A send's or an undo's reply: record what landed — a send's prior values, or an undo's removal
    /// from the record — and say how it went.
    ///
    /// ⚠⚠ **Only what landed.** A send that stops part-way records the photos of its landed groups, so
    /// an undo restores exactly those; one refused outright records nothing and keeps the previous
    /// last send.
    pub(crate) fn on_marks_sent(&mut self, landed: Vec<usize>, error: Option<String>, undo: bool) {
        let Some(p) = self.immich_send_pending.take() else {
            return;
        };
        let landed_ids: Vec<String> = landed
            .iter()
            .filter_map(|i| p.groups.get(*i))
            .flat_map(|g| g.ids.iter().cloned())
            .collect();
        let total: usize = p.groups.iter().map(|g| g.ids.len()).sum();
        if let Some(cat) = &self.catalog {
            let saved = if undo {
                cat.forget_sent(&landed_ids)
            } else if landed_ids.is_empty() {
                Ok(())
            } else {
                let rows: Vec<(String, Option<i32>, bool)> = p
                    .before
                    .iter()
                    .filter(|(id, _)| landed_ids.contains(id))
                    .map(|(id, m)| (id.clone(), m.rating, m.favourite))
                    .collect();
                cat.record_send(&rows)
            };
            if let Err(e) = saved {
                eprintln!("catalog: {e}");
            }
        }
        self.immich_last_send_count = None;
        if !undo && !landed_ids.is_empty() {
            self.forget_send_source_list();
        }
        let done = landed_ids.len();
        self.status = match error {
            None if undo => format!("Restored {} in Immich", photos(done)),
            None => format!("Sent marks for {} to Immich", photos(done)),
            Some(e) => {
                let why = if e == REFUSED {
                    "The Write key cannot change photos — add asset.update to it in Immich"
                        .to_string()
                } else {
                    e
                };
                if done == 0 {
                    why
                } else {
                    format!("{done} of {} done — {why}", photos(total))
                }
            }
        };
    }

    /// Drop the sent album's or month's list — in memory and on disk — so its next opening fetches
    /// the new state.
    fn forget_send_source_list(&mut self) {
        let Some(source) = self.immich_send_source.take() else {
            return;
        };
        let (mem, key) = match source {
            MarksSource::Album(id) => (id.clone(), list_cache::Key::Album(id)),
            MarksSource::Bucket(b) => (b.clone(), list_cache::Key::Bucket(b)),
        };
        self.immich_assets.remove(&mem);
        if let Some(conn) = immich_source::connection(&self.settings) {
            list_cache::forget(&list_cache::lists_dir(&conn.url), &key);
        }
    }

    /// **Undo last send**: ask first.
    pub(crate) fn begin_undo_send(&mut self) {
        let count = self.last_send_count();
        if count > 0 {
            self.modal = Modal::UndoSend { count };
        }
    }

    /// The undo confirmation's button: write back what the last send replaced.
    pub(crate) fn confirm_undo(&mut self) {
        let Some(conn) = self.write_connection() else {
            self.status = "Send marks needs a Write key — set one in Settings → Immich".to_string();
            return;
        };
        let record = self
            .catalog
            .as_ref()
            .and_then(|c| c.last_send().ok())
            .unwrap_or_default();
        let prior: Vec<(String, ServerMarks)> = record
            .into_iter()
            .map(|(id, rating, favourite)| (id, ServerMarks { rating, favourite }))
            .collect();
        if prior.is_empty() {
            return;
        }
        let groups = immich_send::undo_groups(&prior);
        self.immich_send_pending = Some(PendingSend {
            groups: groups.clone(),
            before: Vec::new(),
        });
        self.status = "Restoring in Immich…".to_string();
        self.immich.request(Job::SendMarks {
            conn,
            groups,
            undo: true,
        });
    }

    /// How many photos the last send holds — read once per catalog change, never per frame.
    pub(crate) fn last_send_count(&mut self) -> usize {
        let Some(cat) = &self.catalog else {
            return 0;
        };
        let key = (cat.instance(), cat.change_count());
        if let Some((k, n)) = self.immich_last_send_count {
            if k == key {
                return n;
            }
        }
        let n = cat.last_send().map(|r| r.len()).unwrap_or(0);
        self.immich_last_send_count = Some((key, n));
        n
    }
}

#[cfg(test)]
mod tests {
    use crate::test_app;
    use catalog::db::{Flag, Marks};
    use rustroom_app::immich_send::ServerMarks;
    use rustroom_app::immich_worker::{ImmichWorker, Job, MarksSource, Outcome};
    use std::sync::{Arc, Mutex};

    const URL: &str = "https://example.invalid";

    fn asset(id: &str, rating: Option<i32>, favourite: bool) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: format!("IMG_{id}.jpg"),
            original_mime_type: None,
            width: Some(1),
            height: Some(1),
            thumbhash: None,
            file_modified_at: "2025-01-01T00:00:00.000Z".into(),
            updated_at: "2025-01-02T00:00:00.000Z".into(),
            is_favorite: favourite,
            exif_info: Some(immich::ExifInfo { rating }),
        }
    }

    type Recorder = Arc<Mutex<Vec<Job>>>;

    /// An app with an Immich album of `assets` open and all of it selected, a Write key set, and a
    /// worker that records every job and answers each send as fully landed.
    fn setup(assets: &[immich::Asset]) -> (egui::Context, crate::RustroomApp, Recorder) {
        let ctx = egui::Context::default();
        let mut app = test_app::for_test();
        app.settings.immich_url = URL.to_string();
        app.settings.immich_key = "k".to_string();
        app.settings.immich_upload_key = "w".to_string();
        let rec: Recorder = Arc::new(Mutex::new(Vec::new()));
        let r = rec.clone();
        app.immich = ImmichWorker::new(
            Arc::new(move |job: Job| {
                r.lock().unwrap().push(job.clone());
                match job {
                    Job::CheckMarks { source } => Outcome::CheckMarks {
                        source,
                        result: Ok(vec![]),
                    },
                    Job::SendMarks { groups, undo, .. } => Outcome::SendMarks {
                        landed: (0..groups.len()).collect(),
                        error: None,
                        undo,
                    },
                    Job::Album { id, name } => Outcome::Album {
                        id,
                        name,
                        result: Ok(vec![]),
                    },
                    other => panic!("unexpected job {other:?}"),
                }
            }),
            || {},
        );
        // As the real path does: the rail's click keeps the list in memory, then opens it.
        app.immich_assets.insert("al".into(), assets.to_vec());
        app.open_immich_album(&ctx, "al".into(), "Al".into(), assets);
        let n = app.library.as_ref().unwrap().entries.len();
        let all: Vec<usize> = (0..n).collect();
        app.library.as_mut().unwrap().selection.select_indices(&all);
        (ctx, app, rec)
    }

    fn set_marks(app: &crate::RustroomApp, a: &immich::Asset, m: Marks) {
        let root = rustroom_app::immich_source::cache_root(URL);
        let path = rustroom_app::immich_source::cache_path(&root, a);
        app.catalog
            .as_ref()
            .unwrap()
            .set_marks(&path, 0, m)
            .unwrap();
    }

    fn stars(n: u8) -> Marks {
        Marks {
            rating: n,
            ..Default::default()
        }
    }

    /// Wait until the recorder holds `n` jobs, then return them.
    fn jobs(rec: &Recorder, n: usize) -> Vec<Job> {
        let t = std::time::Instant::now();
        while rec.lock().unwrap().len() < n && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        rec.lock().unwrap().clone()
    }

    fn sends(rec: &Recorder) -> usize {
        rec.lock()
            .unwrap()
            .iter()
            .filter(|j| matches!(j, Job::SendMarks { .. }))
            .count()
    }

    fn album() -> MarksSource {
        MarksSource::Album("al".into())
    }

    #[test]
    fn the_check_uses_the_fresh_list_not_the_cache() {
        let cached = asset("a", Some(3), false);
        let (_ctx, mut app, rec) = setup(std::slice::from_ref(&cached));
        set_marks(&app, &cached, stars(5));
        app.begin_send_marks();
        assert!(matches!(jobs(&rec, 1)[0], Job::CheckMarks { .. }));
        app.on_marks_checked(album(), Ok(vec![asset("a", Some(4), false)]));
        let crate::Modal::SendMarks { changes } = &app.modal else {
            panic!("no confirmation")
        };
        assert_eq!(changes[0].before.rating, Some(4), "planned from the cache");
    }

    #[test]
    fn nothing_reaches_the_server_before_the_confirmation() {
        let a = asset("a", None, false);
        let (_ctx, mut app, rec) = setup(std::slice::from_ref(&a));
        set_marks(&app, &a, stars(5));
        app.begin_send_marks();
        jobs(&rec, 1);
        app.on_marks_checked(album(), Ok(vec![a.clone()]));
        assert!(matches!(app.modal, crate::Modal::SendMarks { .. }));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(sends(&rec), 0, "a send went out before the confirmation");
    }

    #[test]
    fn confirm_sends_the_planned_groups_and_records_what_landed() {
        let (a, b) = (asset("a", Some(2), false), asset("b", None, false));
        let (_ctx, mut app, rec) = setup(&[a.clone(), b.clone()]);
        set_marks(&app, &a, stars(5));
        set_marks(
            &app,
            &b,
            Marks {
                flag: Flag::Pick,
                ..Default::default()
            },
        );
        app.immich_send_selection = vec!["a".into(), "b".into()];
        app.on_marks_checked(album(), Ok(vec![a, b]));
        let crate::Modal::SendMarks { changes } = std::mem::take(&mut app.modal) else {
            panic!("no confirmation")
        };
        let want = rustroom_app::immich_send::groups(&changes);
        app.confirm_send(changes);
        let sent = jobs(&rec, 1);
        let Some(Job::SendMarks { groups, .. }) =
            sent.iter().find(|j| matches!(j, Job::SendMarks { .. }))
        else {
            panic!("no send")
        };
        assert_eq!(groups, &want);
        app.on_marks_sent((0..want.len()).collect(), None, false);
        let mut rec_rows = app.catalog.as_ref().unwrap().last_send().unwrap();
        rec_rows.sort();
        assert_eq!(
            rec_rows,
            vec![
                ("a".to_string(), Some(2), false),
                ("b".to_string(), None, false)
            ]
        );
        assert!(
            app.status.contains("Sent marks for 2 photos"),
            "{}",
            app.status
        );
    }

    #[test]
    fn a_half_landed_send_records_only_what_landed() {
        let (a, b) = (asset("a", None, false), asset("b", None, false));
        let (_ctx, mut app, _rec) = setup(&[a.clone(), b.clone()]);
        set_marks(&app, &a, stars(5));
        set_marks(&app, &b, stars(3));
        app.immich_send_selection = vec!["a".into(), "b".into()];
        app.on_marks_checked(album(), Ok(vec![a, b]));
        let crate::Modal::SendMarks { changes } = std::mem::take(&mut app.modal) else {
            panic!("no confirmation")
        };
        app.confirm_send(changes);
        app.on_marks_sent(
            vec![0],
            Some("could not reach the server: timeout".into()),
            false,
        );
        let rows = app.catalog.as_ref().unwrap().last_send().unwrap();
        assert_eq!(rows, vec![("a".to_string(), None, false)]);
        assert!(app.status.contains("1 of 2"), "{}", app.status);
    }

    #[test]
    fn a_refused_send_records_nothing_and_names_the_permission() {
        let a = asset("a", None, false);
        let (_ctx, mut app, _rec) = setup(std::slice::from_ref(&a));
        set_marks(&app, &a, stars(5));
        app.immich_send_selection = vec!["a".into()];
        app.on_marks_checked(album(), Ok(vec![a]));
        let crate::Modal::SendMarks { changes } = std::mem::take(&mut app.modal) else {
            panic!("no confirmation")
        };
        app.confirm_send(changes);
        app.on_marks_sent(
            vec![],
            Some("the server rejected the API key".into()),
            false,
        );
        assert!(app
            .catalog
            .as_ref()
            .unwrap()
            .last_send()
            .unwrap()
            .is_empty());
        assert!(app.status.contains("asset.update"), "{}", app.status);
    }

    #[test]
    fn undo_sends_the_priors_and_empties_the_record() {
        let (_ctx, mut app, rec) = setup(&[asset("a", None, false)]);
        let record = vec![("a".to_string(), Some(3), true)];
        app.catalog.as_ref().unwrap().record_send(&record).unwrap();
        app.begin_undo_send();
        assert!(matches!(app.modal, crate::Modal::UndoSend { count: 1 }));
        app.confirm_undo();
        let sent = jobs(&rec, 1);
        let Some(Job::SendMarks { groups, undo, .. }) =
            sent.iter().find(|j| matches!(j, Job::SendMarks { .. }))
        else {
            panic!("no undo send")
        };
        assert!(*undo);
        let want = rustroom_app::immich_send::undo_groups(&[(
            "a".to_string(),
            ServerMarks {
                rating: Some(3),
                favourite: true,
            },
        )]);
        assert_eq!(groups, &want);
        app.on_marks_sent(vec![0], None, true);
        assert!(app
            .catalog
            .as_ref()
            .unwrap()
            .last_send()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn nothing_to_change_says_so_and_opens_nothing() {
        let a = asset("a", Some(4), false);
        let (_ctx, mut app, _rec) = setup(std::slice::from_ref(&a));
        set_marks(&app, &a, stars(4));
        app.immich_send_selection = vec!["a".into()];
        app.on_marks_checked(album(), Ok(vec![a]));
        assert!(matches!(app.modal, crate::Modal::None));
        assert_eq!(app.status, "Immich already has these marks");
    }

    #[test]
    fn without_a_write_key_the_send_says_where_to_set_one() {
        let (_ctx, mut app, rec) = setup(&[asset("a", None, false)]);
        app.settings.immich_upload_key.clear();
        app.begin_send_marks();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            rec.lock().unwrap().is_empty(),
            "a job went out without a Write key"
        );
        assert!(app.status.contains("Settings → Immich"), "{}", app.status);
    }
}
