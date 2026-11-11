//! Opening an Immich album, and draining the worker once a frame.
//!
//! ⚠ A separate bin module rather than more of [`crate::sources`], which is 571 lines and whose
//! subject is the local sources. Everything Immich-specific about *showing* a library lives here;
//! the reusable logic is in [`rustroom_app::immich_source`] and the worker in
//! [`rustroom_app::immich_worker`], neither of which knows about egui.

use crate::{AppView, LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::immich_list_cache as list_cache;
use rustroom_app::immich_source;
use rustroom_app::immich_stack_cache as stack_cache;
use rustroom_app::immich_worker::{Job, Outcome};
use rustroom_app::photo_id::PhotoId;

impl RustroomApp {
    /// The validator for an album, taken from the rail's own already-fetched list.
    ///
    /// ⚠ `None` when the rail has no list, or does not know this album: there is then nothing to
    /// validate against, and an unvalidated load could put a stale grid on screen.
    fn album_validator(&self, id: &str) -> Option<list_cache::Validator> {
        let albums = self.immich_albums.as_ref()?.as_ref().ok()?;
        let a = albums.iter().find(|a| a.id == id)?;
        Some(list_cache::Validator::Album {
            asset_count: a.asset_count,
            last_modified: a.last_modified_asset_timestamp.clone(),
        })
    }

    /// The validator for one month, from the rail's own bucket list.
    fn bucket_validator(&self, bucket: &str) -> Option<list_cache::Validator> {
        let buckets = self.immich_buckets.as_ref()?.as_ref().ok()?;
        let b = buckets.iter().find(|b| b.time_bucket == bucket)?;
        Some(list_cache::Validator::Bucket { count: b.count })
    }

    /// The directory this server's lists live in, if a server is configured at all.
    fn immich_lists_dir(&self) -> Option<std::path::PathBuf> {
        let conn = immich_source::connection(&self.settings)?;
        Some(list_cache::lists_dir(&conn.url))
    }

    /// The configured server's cache root, if there is one — what `row_is_browsable` needs.
    ///
    /// ⚠ `None` when no server is configured, which leaves every catalog row browsable rather than
    /// hiding things because a setting is blank.
    /// Where this server's stack projection lives, or `None` with no server configured.
    pub(crate) fn immich_stacks_path(&self) -> Option<std::path::PathBuf> {
        let url = self.settings.immich_url.trim();
        if url.is_empty() {
            return None;
        }
        Some(stack_cache::stacks_path(url))
    }

    /// Load the stored projection, and ask for a fresh one if there is none.
    ///
    /// ⚠ Called at startup and after Reindex drops the file. ⚠⚠ A fetch is requested only when the
    /// file is **absent**, because there is no validator to check a present one against — freshness
    /// rests on Reindex, which `immich_stack_cache`'s doc states as a limitation rather than hides.
    pub(crate) fn load_immich_stacks(&mut self) {
        let Some(path) = self.immich_stacks_path() else {
            return;
        };
        match stack_cache::load(&path) {
            Some(p) => self.immich_stacks = p,
            None => {
                self.immich_stacks = Default::default();
                self.immich.request(Job::Stacks);
            }
        }
    }

    pub(crate) fn immich_cache_root(&self) -> Option<std::path::PathBuf> {
        // ⚠⚠ **Derived from the URL alone, NOT through `connection`** — a path is not a behaviour.
        // Going through the gate meant that switching Immich off made this `None`, and
        // `folder_display_name` then fell through to the directory's basename: the Folders row
        // stopped reading "immich (downloaded)" and started showing the raw host slug
        // `https___host_example`. Found by use, 2026-09-24 — the user's report was "the name still
        // changes", and the rename I had *already* reverted was not the cause.
        //
        // ⚠ The gate belongs where behaviour is gated. Everything that *acts* on the server still
        // goes through `connection`; this only says where the downloads live, which is true whether
        // or not the integration is switched on.
        let url = self.settings.immich_url.trim();
        if url.is_empty() {
            return None;
        }
        Some(immich_source::cache_root(url))
    }

    /// Re-read which of this server's assets are on disk.
    ///
    /// ⚠ On opening a library rather than per frame, so a photo downloaded — or deleted — outside
    /// Rustroom is picked up at the next open without costing a stat per thumbnail.
    pub(crate) fn refresh_immich_present(&mut self) {
        self.immich_present = match self.immich_cache_root() {
            Some(root) => immich_source::present_names(&root),
            None => Default::default(),
        };
    }

    /// An album's stored asset list, if it is still current.
    pub(crate) fn immich_cached_album(&self, id: &str) -> Option<Vec<immich::Asset>> {
        let dir = self.immich_lists_dir()?;
        let want = self.album_validator(id)?;
        list_cache::load(&dir, &list_cache::Key::Album(id.to_string()), &want)
    }

    /// One month's stored asset list, if it is still current.
    pub(crate) fn immich_cached_bucket(&self, bucket: &str) -> Option<Vec<immich::Asset>> {
        let dir = self.immich_lists_dir()?;
        let want = self.bucket_validator(bucket)?;
        list_cache::load(&dir, &list_cache::Key::Bucket(bucket.to_string()), &want)
    }

    /// Remember a freshly fetched list on disk, so the next launch does not refetch it.
    ///
    /// ⚠ Stamped with the validator **as the rail currently sees it**. If the rail's own list is
    /// stale then so is the stamp, and the next launch refetches — a wasted fetch, never a wrong
    /// grid. No validator at all means nothing is written.
    fn remember_immich_list(
        &self,
        key: list_cache::Key,
        want: Option<list_cache::Validator>,
        assets: &[immich::Asset],
    ) {
        let (Some(dir), Some(v)) = (self.immich_lists_dir(), want) else {
            return;
        };
        list_cache::store(&dir, &key, &v, assets);
    }

    /// How many of this album's photos are downloaded, out of how many — or `None` when its asset list
    /// is not in hand.
    ///
    /// ⚠ `None` rather than `Some((0, 0))`: "we do not know" and "there is nothing here" lead to
    /// different buttons, and collapsing them would hide the action that loads the list.
    ///
    /// ⚠⚠ **Every album row asks on EVERY Library frame** (2026-10-12). It used to clone the list and
    /// build catalog entries — a sidecar `stat` per asset — and, for an album not opened this
    /// session, read and parse its stored list: over 12 albums of 3,538 assets, most of the
    /// Library's UI time. Now the names are derived once per list (`immich_album_names`) and only
    /// looked up in `immich_present` per frame — live, so a download shows at once.
    pub(crate) fn immich_downloaded_of(&self, id: &str) -> Option<(usize, usize)> {
        // ⚠ With no cache root nothing is downloaded or counted: `Some((0, 0))` for a list in hand,
        // exactly what the empty path list gave before — never `None`, which means "not in hand".
        let rooted = self.immich_cache_root().is_some();
        use rustroom_app::immich_album_names::{fingerprint, ListKey};
        let mut memo = self.immich_album_names.borrow_mut();
        let names = match self.immich_assets.get(id) {
            Some(assets) => memo.names(id, ListKey::Session(fingerprint(assets)), || {
                Some(immich_source::shown_names(assets))
            }),
            None => memo.names(id, ListKey::Stored(self.album_validator(id)), || {
                self.immich_cached_album(id)
                    .map(|a| immich_source::shown_names(&a))
            }),
        }?;
        Some(if rooted {
            immich_source::count_present(names, &self.immich_present)
        } else {
            (0, 0)
        })
    }

    /// An album's asset list if it is in hand — the session map, then the stored list.
    ///
    /// ⚠ Deliberately does **not** fetch: a row action that silently started a network request would be
    /// indistinguishable from one that did nothing. The caller decides what to say.
    pub(crate) fn immich_album_assets(&self, id: &str) -> Option<Vec<immich::Asset>> {
        self.immich_assets
            .get(id)
            .cloned()
            .or_else(|| self.immich_cached_album(id))
    }

    /// The cache paths these assets occupy — the same derivation the grid uses, never a second one.
    pub(crate) fn immich_paths_for(&self, assets: &[immich::Asset]) -> Vec<std::path::PathBuf> {
        let Some(root) = self.immich_cache_root() else {
            return Vec::new();
        };
        immich_source::entries_from_assets(assets, &root)
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    /// One finished original download.
    ///
    /// ⚠ Its own method so a test can drive the **branch that actually runs**, rather than calling
    /// `open_path` directly and proving nothing about `pump_immich`.
    pub(crate) fn finish_immich_download(
        &mut self,
        ctx: &egui::Context,
        dest: std::path::PathBuf,
        result: Result<(), String>,
    ) {
        // ⚠ M10.19b: a download a Move to folder waits for opens nothing; the move runs when the last lands.
        if self.move_download_landed(ctx, &dest, result.is_ok()) {
            return;
        }
        match result {
            Ok(()) => {
                // ⚠ One insert rather than re-reading the directory: this download's own path
                // is already in hand, and it is what the grid's indicator reads.
                if let Some(name) = dest.file_name() {
                    self.immich_present.insert(name.to_os_string());
                }
                // ⚠⚠ Only a **bulk** member advances the count. A single photo opened while a
                // bulk runs is simply not in the set, which is why this is a set and not a
                // counter.
                let bulk = immich_source::asset_id_of(&dest)
                    .is_some_and(|id| self.immich_download_pending.remove(&id));
                if bulk && self.immich_download_pending.is_empty() {
                    self.status = format!("Downloaded {} photos", self.immich_download_total);
                    self.immich_download_total = 0;
                }
                // ⚠⚠ **A predownload must NOT open anything.** Reported in use: pressing Download
                // threw the user into Develop on photo after photo. *Downloading* and *opening*
                // are two acts, and only the second is a request to look at something — which is
                // why opening a photo is a double-click and this is a button.
                if !bulk {
                    self.open_path(ctx, PhotoId::original(dest));
                    self.view = AppView::Develop;
                }
            }
            Err(e) => self.status = format!("Download failed: {e}"),
        }
    }

    /// Predownload every asset in `paths` that is not already here.
    ///
    /// ⚠ Assets already on disk are **skipped**, which is what makes pressing the button twice cheap
    /// rather than a second full download.
    ///
    /// ⚠⚠ The worker queues past `MAX_CONCURRENT_DOWNLOADS`, so this may hand it hundreds of jobs
    /// safely — before that queue existed, an 872-asset album would have spawned 872 threads.
    pub(crate) fn download_assets(&mut self, paths: &[std::path::PathBuf]) {
        let wanted: Vec<(String, std::path::PathBuf)> = paths
            .iter()
            .filter(|p| {
                !p.file_name()
                    .is_some_and(|n| self.immich_present.contains(n))
            })
            .filter_map(|p| immich_source::asset_id_of(p).map(|id| (id, p.clone())))
            .collect();

        if wanted.is_empty() {
            self.status = "Every photo here is already downloaded".to_string();
            return;
        }
        self.immich_download_total = wanted.len();
        self.immich_download_pending = wanted.iter().map(|(id, _)| id.clone()).collect();
        for (asset_id, dest) in wanted {
            self.immich.request(Job::Original { asset_id, dest });
        }
        self.status = self.immich_download_progress();
    }

    /// What the status line says while a bulk download runs, or empty when none is.
    pub(crate) fn immich_download_progress(&self) -> String {
        if self.immich_download_total == 0 {
            return String::new();
        }
        let done = self
            .immich_download_total
            .saturating_sub(self.immich_download_pending.len());
        let what = if self
            .pending_move
            .as_ref()
            .is_some_and(|m| !m.is_cancelled())
        {
            "to move"
        } else {
            "from Immich"
        };
        format!(
            "Downloading {done} of {} {what}…",
            self.immich_download_total
        )
    }

    /// Abandon whatever is still queued. ⚠ The in-flight few finish — see `cancel_downloads`.
    pub(crate) fn cancel_immich_downloads(&mut self) {
        let dropped = self.immich.cancel_downloads();
        self.immich_download_pending.clear();
        self.immich_download_total = 0;
        // ⚠ M10.19b: a move waiting for these downloads moves nothing — after `cancel_downloads`, so only
        // the in-flight few are left for it to absorb.
        if self.cancel_pending_move() {
            self.status = "Move cancelled — nothing moved".to_string();
            return;
        }
        self.status = if dropped == 0 {
            "Nothing left to cancel".to_string()
        } else {
            format!("Stopped — {dropped} downloads not started")
        };
    }

    /// Delete the local copies of `paths`, keeping every edit and mark.
    ///
    /// ⚠⚠ Nothing here reaches the server, and it cannot: the API key is read-only by the Settings help
    /// text's own instruction. See `immich_cache::remove_downloads` for what is deliberately left behind.
    pub(crate) fn remove_downloaded(&mut self, paths: &[std::path::PathBuf]) {
        // ⚠⚠ **The open photo is removed too, and Develop is emptied** (user's call, 2026-09-24,
        // overriding the earlier decision to skip it). Removing everything *except* the one you are
        // looking at is a surprising exception, and the session is closed first so no render is holding
        // a file that is about to vanish.
        //
        // ⚠ `persist_current_session` first, so the edits reach the sidecar — which the removal keeps.
        // Without this, removing the open photo would discard whatever had not settled.
        let open = self.session.as_ref().map(|s| s.path.clone());
        if let Some(open) = &open {
            if paths.iter().any(|p| p == open) {
                self.persist_current_session();
                self.session = None;
                self.view = AppView::Library;
            }
        }
        let out = rustroom_app::immich_cache::remove_downloads(paths);
        // ⚠ So the rail's count and the grid's ↓ agree with the disk immediately.
        self.refresh_immich_present();
        self.status = if out.removed == 0 {
            "Nothing to remove — these are not downloaded".to_string()
        } else {
            format!(
                "Removed {} local copies — they are still on Immich",
                out.removed
            )
        };
    }

    /// Clear every downloaded original of the server cached at `root` — Remove downloads applied to
    /// all of them (M10.15b), so the open photo, the sidecars and the grid's ↓ are handled the same.
    pub(crate) fn clear_immich_cache(&mut self, root: &std::path::Path) {
        let paths = rustroom_app::immich_cache::downloads(root);
        self.remove_downloaded(&paths);
        self.immich_originals = None;
    }

    /// Once, after bootstrap: offer to clear when Immich is connected, the warning is on and the
    /// downloads are over the soft limit.
    pub(crate) fn check_immich_cache_at_startup(&mut self) {
        let connected = immich_source::connection(&self.settings).is_some();
        if let Some(root) = self.immich_cache_root() {
            self.check_immich_cache_in(connected, root);
        }
    }

    /// [`Self::check_immich_cache_at_startup`] with the root given — what the tests drive.
    pub(crate) fn check_immich_cache_in(&mut self, connected: bool, root: std::path::PathBuf) {
        // ⚠ Not even measured while Immich is off: the integration shows no trace of itself then.
        if !connected {
            return;
        }
        let usage = rustroom_app::immich_cache::usage(&root);
        let s = &self.settings;
        if rustroom_app::immich_cache::warn_at_startup(
            connected,
            s.immich_cache_warn,
            s.immich_cache_limit_gb,
            usage,
        ) {
            self.modal = crate::Modal::ImmichCache {
                startup: true,
                root,
                usage,
            };
        }
    }

    /// Show every photo of an Immich person (M10.18c) — the server's, and the local ones named after them.
    pub(crate) fn open_immich_person(
        &mut self,
        ctx: &egui::Context,
        id: String,
        name: String,
        assets: &[immich::Asset],
    ) {
        let Some(conn) = immich_source::connection(&self.settings) else {
            return;
        };
        let root = immich_source::cache_root(&conn.url);
        let mut scan = immich_source::entries_from_assets(assets, &root);
        let person = self.catalog.as_ref().and_then(|cat| {
            rustroom_app::immich_catalog::record_assets(cat, &root, &scan).ok();
            let _ = rustroom_app::immich_catalog::fill_marks(cat, &root, assets);
            let _ = rustroom_app::immich_catalog::fill_locations(cat, &root, assets);
            let _ = rustroom_app::immich_catalog::fill_faces(cat, &root, assets);
            cat.people()
                .ok()?
                .into_iter()
                .find(|p| p.immich_id.as_deref() == Some(&id))
                .map(|p| p.id)
        });
        // The local photos named after them, beside the server's.
        if let (Some(cat), Some(pid)) = (&self.catalog, person) {
            if let Ok(local) = cat.person_members(pid) {
                let have: std::collections::HashSet<std::path::PathBuf> =
                    scan.iter().map(|e| e.path.clone()).collect();
                let more: Vec<(std::path::PathBuf, u32)> = local
                    .into_iter()
                    .filter(|(p, _)| !have.contains(p))
                    .collect();
                scan.extend(rustroom_app::collections::scan_of(
                    &more,
                    rustroom_app::sidecar::edited_at,
                    crate::library_state::fs_modified,
                ));
            }
        }
        catalog::sort(&mut scan, self.settings.library_sort);
        self.refresh_immich_present();
        let n = self.open_scan(
            ctx,
            LibrarySource::ImmichPerson {
                id,
                person: person.unwrap_or(0),
                name: name.clone(),
            },
            scan,
        );
        self.status = format!("{n} photos of {name}");
    }

    /// Show one Immich album's photos, downloading nothing.
    ///
    /// ⚠⚠ Goes through `open_scan`, the one place a `Library` is constructed, exactly like a
    /// folder — which is the entire point of shape A. The rows name cache paths that mostly do not
    /// exist yet, and `open_scan`'s own callees have to tolerate that.
    pub(crate) fn open_immich_album(
        &mut self,
        ctx: &egui::Context,
        id: String,
        name: String,
        assets: &[immich::Asset],
    ) -> usize {
        let Some(conn) = immich_source::connection(&self.settings) else {
            self.status = "No Immich server is configured — set one in Settings".to_string();
            return 0;
        };
        let root = immich_source::cache_root(&conn.url);
        let mut scan = immich_source::entries_from_assets(assets, &root);
        // ⚠⚠ `open_scan` is documented as taking an **already-sorted** scan, and every other
        // loader sorts here. Omitting it made the library sort setting silently do nothing for an
        // album — found by reading the other five, not by a test.
        catalog::sort(&mut scan, self.settings.library_sort);
        // ⚠⚠ Without this there is no `photos` row, and `set_marks` then returns `Ok(())` having
        // done **nothing** — which is how every rating, flag and label on an Immich photo used to
        // disappear, with nothing failing. See `immich_catalog::record_assets` for why the folder is
        // the server's cache root rather than the album.
        //
        // ⚠ Logged and ignored on failure, exactly as `open_folder_at` does: a catalog that cannot be
        // written must not stop the album opening.
        if let Some(cat) = &self.catalog {
            if let Err(e) = rustroom_app::immich_catalog::record_assets(cat, &root, &scan) {
                eprintln!("catalog: {e}");
            }
            // M10.15c: Immich's own stars and favourite, filled once where Rustroom has none —
            // before the grid is built, so they show as the album opens.
            if let Err(e) = rustroom_app::immich_catalog::fill_marks(cat, &root, assets) {
                eprintln!("catalog: {e}");
            }
            // M10.18b: Immich's positions, the Map's pins for phone photos.
            if let Err(e) = rustroom_app::immich_catalog::fill_locations(cat, &root, assets) {
                eprintln!("catalog: {e}");
            }
            // M10.18c: Immich's people and their faces, the People list's.
            if let Err(e) = rustroom_app::immich_catalog::fill_faces(cat, &root, assets) {
                eprintln!("catalog: {e}");
            }
        }
        self.refresh_immich_present();
        let n = self.open_scan(
            ctx,
            LibrarySource::ImmichAlbum {
                id,
                name: name.clone(),
            },
            scan,
        );
        // ⚠ Said after `open_scan`, which sets its own status for a local scan.
        self.status = if n == assets.len() {
            format!("{n} photos in “{name}”")
        } else {
            // ⚠ The difference is worth saying out loud: videos and RAW formats this build cannot
            // decode are skipped, and a silently shorter grid reads as photos having gone missing.
            format!(
                "{n} of {} photos in “{name}” — the rest are videos or formats Rustroom cannot open",
                assets.len()
            )
        };
        n
    }

    /// Show one month of the Immich timeline.
    ///
    /// ⚠ Identical in shape to [`Self::open_immich_album`] — including `catalog::sort`, which that
    /// function was missing at first and which silently ignored the library sort setting.
    pub(crate) fn open_immich_bucket(
        &mut self,
        ctx: &egui::Context,
        bucket: String,
        label: String,
        assets: &[immich::Asset],
    ) -> usize {
        let Some(conn) = immich_source::connection(&self.settings) else {
            self.status = "No Immich server is configured — set one in Settings".to_string();
            return 0;
        };
        let root = immich_source::cache_root(&conn.url);
        let mut scan = immich_source::entries_from_assets(assets, &root);
        catalog::sort(&mut scan, self.settings.library_sort);
        // ⚠ Rows for the marks, as in `open_immich_album` above — the reasoning is there and in
        // `immich_catalog::record_assets`, not repeated here.
        if let Some(cat) = &self.catalog {
            if let Err(e) = rustroom_app::immich_catalog::record_assets(cat, &root, &scan) {
                eprintln!("catalog: {e}");
            }
            // M10.15c: as for an album.
            if let Err(e) = rustroom_app::immich_catalog::fill_marks(cat, &root, assets) {
                eprintln!("catalog: {e}");
            }
            // M10.18b: Immich's positions, the Map's pins for phone photos.
            if let Err(e) = rustroom_app::immich_catalog::fill_locations(cat, &root, assets) {
                eprintln!("catalog: {e}");
            }
            // M10.18c: Immich's people and their faces, the People list's.
            if let Err(e) = rustroom_app::immich_catalog::fill_faces(cat, &root, assets) {
                eprintln!("catalog: {e}");
            }
        }
        self.refresh_immich_present();
        let n = self.open_scan(
            ctx,
            LibrarySource::ImmichBucket {
                bucket,
                label: label.clone(),
            },
            scan,
        );
        self.status = if n == assets.len() {
            format!("{n} photos in {label}")
        } else {
            format!(
                "{n} of {} photos in {label} — the rest are videos or formats Rustroom cannot open",
                assets.len()
            )
        };
        n
    }

    /// Drain the Immich worker. Called once a frame from [`crate::shell`].
    pub(crate) fn pump_immich(&mut self, ctx: &egui::Context) {
        for outcome in self.immich.poll() {
            match outcome {
                Outcome::Albums(result) => self.immich_albums = Some(result),
                Outcome::Buckets(result) => self.immich_buckets = Some(result),
                Outcome::Stacks(result) => {
                    // ⚠⚠ A failure is a **miss, never an error**: the projection stays empty, the grid
                    // shows both halves, and nothing is reported. A cache that can stop a folder
                    // opening is worse than no cache — `immich_list_cache`'s rule.
                    if let Ok(stacks) = result {
                        self.immich_stacks = stack_cache::project(&stacks);
                        if let Some(p) = self.immich_stacks_path() {
                            stack_cache::store(&p, &self.immich_stacks);
                        }
                        // ⚠ The grid was grouped with the old (empty) map, so it must be **rebuilt** —
                        // patching it would leave `selection` and `view_region` on a stale list.
                        self.rebuild_entries(None);
                        ctx.request_repaint();
                    }
                }
                Outcome::Bucket {
                    bucket,
                    label,
                    result,
                } => {
                    self.immich_opening = None;
                    match result {
                        Ok(assets) => {
                            self.immich_assets.insert(bucket.clone(), assets.clone());
                            let v = self.bucket_validator(&bucket);
                            self.remember_immich_list(
                                list_cache::Key::Bucket(bucket.clone()),
                                v,
                                &assets,
                            );
                            self.open_immich_bucket(ctx, bucket, label, &assets);
                        }
                        Err(e) => self.status = format!("Could not open {label}: {e}"),
                    }
                }
                Outcome::People(Ok(people)) => {
                    let named: Vec<(String, String)> = people
                        .into_iter()
                        .map(|p| (p.id, p.name.trim().to_string()))
                        .collect();
                    if let Some(cat) = &self.catalog {
                        // The server's whole list: people gone from it leave ours (the review's #9).
                        if let Err(e) = cat.replace_immich_people(&named) {
                            eprintln!("catalog: {e}");
                        }
                    }
                }
                // ⚠ A key without `person.read` answers 403: said once, and the People list is simply local.
                Outcome::People(Err(e)) => {
                    eprintln!("immich: people: {e}");
                    self.status =
                        "Immich's people could not be read — the key may lack person.read".into();
                }
                Outcome::Person { id, name, result } => {
                    self.immich_opening = None;
                    match result {
                        Ok(assets) => self.open_immich_person(ctx, id, name, &assets),
                        Err(e) => self.status = format!("Could not open {name}: {e}"),
                    }
                }
                Outcome::Album { id, name, result } => {
                    self.immich_opening = None;
                    match result {
                        Ok(assets) => {
                            // ⚠ Remembered so switching back is instant — the round trip is a
                            // couple of seconds for a large album.
                            self.immich_assets.insert(id.clone(), assets.clone());
                            // ⚠ And on disk too, so the *first* visit after the next launch is
                            // instant as well — that is what the list cache buys over this map.
                            let v = self.album_validator(&id);
                            self.remember_immich_list(
                                list_cache::Key::Album(id.clone()),
                                v,
                                &assets,
                            );
                            self.open_immich_album(ctx, id, name, &assets);
                        }
                        Err(e) => self.status = format!("Could not open “{name}”: {e}"),
                    }
                }
                // ⚠ Into the dialog's own field, never the status line: the status line is behind
                // the modal, so the answer to the Test button was invisible.
                Outcome::About(Ok(v)) => self.immich_test = Some(Ok(v)),
                Outcome::About(Err(e)) => self.immich_test = Some(Err(e)),
                // M10.15d: a send's check and its reply — `send_flow`.
                Outcome::CheckMarks { source, result } => self.on_marks_checked(source, result),
                Outcome::SendMarks {
                    landed,
                    error,
                    undo,
                } => self.on_marks_sent(landed, error, undo),
                Outcome::Original { dest, result } => {
                    self.finish_immich_download(ctx, dest, result)
                }
            }
        }
    }

    /// Ask the server for its albums, unless a list request is already in flight.
    pub(crate) fn request_immich_albums(&mut self) {
        self.immich.request(Job::Albums);
    }
}

impl RustroomApp {
    /// The download job for `path`, if it names an Immich cache file on a configured server.
    ///
    /// ⚠ `None` for every ordinary local path, which is what keeps [`Self::open_path`]'s handling
    /// of a missing local file exactly as it was.
    pub(crate) fn immich_download_for(&self, path: &std::path::Path) -> Option<Job> {
        let conn = immich_source::connection(&self.settings)?;
        if !immich_source::is_immich_path(&immich_source::cache_root(&conn.url), path) {
            return None;
        }
        Some(Job::Original {
            asset_id: immich_source::asset_id_of(path)?,
            dest: path.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use rustroom_app::immich_list_cache as list_cache;

    /// An image asset named `name` — the ten fields a list reply carries.
    fn photo(id: &str, name: &str) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: immich::AssetType::Image,
            original_file_name: name.to_string(),
            original_mime_type: None,
            width: Some(100),
            height: Some(100),
            thumbhash: None,
            file_modified_at: "2025-08-14T09:21:33.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            is_favorite: false,
            exif_info: None,
            people: None,
        }
    }

    /// The album row's count through the method the rail calls on every frame (2026-10-12): the
    /// session's list borrowed, one of two photos present — and `None` for a list not in hand,
    /// which the row turns into a different pair of buttons.
    #[test]
    fn an_album_row_counts_its_downloaded_photos() {
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        let (a, b) = (photo("aaa", "A.JPG"), photo("bbb", "B.jpg"));
        let root = app.immich_cache_root().expect("a URL gives a cache root");
        let here = rustroom_app::immich_source::cache_path(&root, &a);
        app.immich_present
            .insert(here.file_name().unwrap().to_os_string());
        app.immich_assets.insert("al".to_string(), vec![a, b]);

        assert_eq!(app.immich_downloaded_of("al"), Some((1, 2)));
        assert_eq!(app.immich_downloaded_of("not-in-hand"), None);
    }

    /// ⚠⚠ What is downloaded is read live, never cached with the names: a finished download shows in
    /// the row's count on the next frame, and a removal likewise.
    #[test]
    fn a_download_shows_in_the_album_count_at_once() {
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        let (a, b) = (photo("aaa", "A.JPG"), photo("bbb", "B.jpg"));
        let root = app.immich_cache_root().expect("a URL gives a cache root");
        let name = |x: &immich::Asset| {
            rustroom_app::immich_source::cache_path(&root, x)
                .file_name()
                .unwrap()
                .to_os_string()
        };
        let (na, nb) = (name(&a), name(&b));
        app.immich_assets.insert("al".to_string(), vec![a, b]);
        assert_eq!(app.immich_downloaded_of("al"), Some((0, 2)));
        app.immich_present.insert(na.clone());
        app.immich_present.insert(nb);
        assert_eq!(app.immich_downloaded_of("al"), Some((2, 2)), "downloaded");
        app.immich_present.remove(&na);
        assert_eq!(app.immich_downloaded_of("al"), Some((1, 2)), "one removed");
    }

    /// ⚠ A list fetched again with another photo in it is counted again.
    #[test]
    fn a_refetched_album_list_is_recounted() {
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        app.immich_assets
            .insert("al".to_string(), vec![photo("aaa", "A.JPG")]);
        assert_eq!(app.immich_downloaded_of("al"), Some((0, 1)));
        app.immich_assets.insert(
            "al".to_string(),
            vec![photo("aaa", "A.JPG"), photo("ccc", "C.jpg")],
        );
        assert_eq!(app.immich_downloaded_of("al"), Some((0, 2)));
    }

    fn album(id: &str, count: u32, modified: Option<&str>) -> immich::AlbumSummary {
        immich::AlbumSummary {
            id: id.to_string(),
            album_name: format!("album {id}"),
            asset_count: count,
            last_modified_asset_timestamp: modified.map(|s| s.to_string()),
        }
    }

    /// ⚠⚠ **Reported in use: pressing Download threw the user into Develop, photo after photo.**
    /// *Downloading* and *opening* are two acts, and only the second is a request to look at something —
    /// which is why opening a photo is a double-click and this is a button.
    ///
    /// ⚠ Driven through `pump_immich`'s own arm rather than by calling `open_path` directly, so it tests
    /// the branch that actually ran.
    #[test]
    fn a_bulk_download_completing_does_not_open_anything() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        app.settings.immich_key = "k".to_string();

        // ⚠⚠ Set explicitly, because `test_app::for_test` starts in **Develop** — without this the
        // assertion below fails against correct code, and its sibling passes without proving anything.
        app.view = crate::AppView::Library;
        // A bulk is running and this asset belongs to it.
        app.immich_download_total = 2;
        app.immich_download_pending = ["aaa".to_string(), "bbb".to_string()].into_iter().collect();
        let root = app.immich_cache_root().unwrap();

        app.finish_immich_download(&ctx, root.join("aaa.jpg"), Ok(()));

        assert!(app.session.is_none(), "a predownload must not open a photo");
        assert!(
            matches!(app.view, crate::AppView::Library),
            "and must not switch to Develop"
        );
        assert!(
            app.immich_download_pending.contains("bbb"),
            "the other one is still pending"
        );
        assert_eq!(app.immich_download_total, 2, "still running");
    }

    /// ⚠ The other half: a photo downloaded because it was **opened** still opens. Without this the fix
    /// above could have disabled opening altogether and nothing would have failed.
    #[test]
    fn a_download_that_was_an_open_still_opens() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        app.settings.immich_key = "k".to_string();
        let root = app.immich_cache_root().unwrap();
        // ⚠⚠ **Library first, or this test is vacuous**: `for_test` starts in Develop, so "it reached
        // Develop" would be true before the call. Asserted as a real transition.
        app.view = crate::AppView::Library;
        assert!(
            app.immich_download_pending.is_empty(),
            "no bulk is running, which is what makes this an open"
        );

        app.finish_immich_download(&ctx, root.join("ccc.jpg"), Ok(()));

        assert!(
            matches!(app.view, crate::AppView::Develop),
            "an open must reach Develop"
        );
    }

    /// ⚠ An asset already on disk is not asked for again — what makes pressing Download twice cheap.
    #[test]
    fn a_download_skips_what_is_already_present() {
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        app.settings.immich_key = "k".to_string();
        let root = std::path::Path::new("/cache/immich/host");
        let here = root.join("aaa.jpg");
        let missing = root.join("bbb.jpg");
        app.immich_present.insert("aaa.jpg".into());

        app.download_assets(&[here, missing]);

        assert_eq!(app.immich_download_total, 1, "only the absent one");
        assert!(app.immich_download_pending.contains("bbb"));
        assert!(!app.immich_download_pending.contains("aaa"));
    }

    /// ⚠ And when there is nothing to do it says so rather than showing a progress line that never moves.
    #[test]
    fn a_download_of_only_present_assets_says_so_and_starts_nothing() {
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = "https://example.invalid".to_string();
        app.settings.immich_key = "k".to_string();
        app.immich_present.insert("aaa.jpg".into());

        app.download_assets(&[std::path::PathBuf::from("/cache/immich/host/aaa.jpg")]);

        assert_eq!(app.immich_download_total, 0);
        assert!(app.immich_download_pending.is_empty());
        assert!(app.status.contains("already downloaded"), "{}", app.status);
    }

    /// ⚠ The progress line is empty when nothing is running, so the toolbar has nothing to draw.
    #[test]
    fn the_progress_line_is_empty_when_no_download_is_running() {
        let app = crate::test_app::for_test();
        assert!(app.immich_download_progress().is_empty());
    }

    /// ⚠⚠ **The seam the module tests could not reach.** `immich_list_cache` proves that a matching
    /// validator hits and a differing one misses; nothing there proves the validator is built from
    /// the right two fields of the **right album**. Two albums with different counts, so a lookup
    /// that ignored the id and took the first row would fail here — and every module test would
    /// still have passed, because both halves would agree with each other on the wrong album.
    ///
    /// ⚠ Verified by sabotage: replacing the `find` with `albums.first()` fails this test.
    #[test]
    fn an_albums_validator_is_its_own_count_and_last_modified() {
        let mut app = crate::test_app::for_test();
        app.immich_albums = Some(Ok(vec![
            album("a", 872, Some("2026-08-14T08:29:23.862Z")),
            album("b", 112, Some("2026-06-14T12:11:35.623Z")),
        ]));
        assert_eq!(
            app.album_validator("b"),
            Some(list_cache::Validator::Album {
                asset_count: 112,
                last_modified: Some("2026-06-14T12:11:35.623Z".to_string()),
            })
        );
    }

    /// ⚠ An album the rail does not list cannot be validated, so it must refetch rather than guess.
    #[test]
    fn an_unknown_album_has_no_validator() {
        let mut app = crate::test_app::for_test();
        app.immich_albums = Some(Ok(vec![album("a", 1, Some("x"))]));
        assert!(app.album_validator("nope").is_none());
    }

    /// ⚠⚠ The three states of the rail's list — never fetched, failed, empty — must all yield no
    /// validator. A failed fetch especially: treating it as "nothing changed" would pin a stale grid
    /// in place for as long as the server stayed unreachable.
    #[test]
    fn no_album_list_and_a_failed_one_both_yield_no_validator() {
        let mut app = crate::test_app::for_test();
        assert!(app.album_validator("a").is_none(), "never fetched");
        app.immich_albums = Some(Err("offline".to_string()));
        assert!(app.album_validator("a").is_none(), "failed");
        app.immich_albums = Some(Ok(vec![]));
        assert!(app.album_validator("a").is_none(), "empty");
    }

    #[test]
    fn a_buckets_validator_is_that_months_count() {
        let mut app = crate::test_app::for_test();
        app.immich_buckets = Some(Ok(vec![
            immich::Bucket {
                time_bucket: "2026-08-01".to_string(),
                count: 76,
            },
            immich::Bucket {
                time_bucket: "2026-07-01".to_string(),
                count: 5,
            },
        ]));
        assert_eq!(
            app.bucket_validator("2026-07-01"),
            Some(list_cache::Validator::Bucket { count: 5 })
        );
        assert!(app.bucket_validator("2026-06-01").is_none());
    }

    /// ⚠⚠ With no server configured there is no cache directory, so a lookup must answer `None`
    /// rather than reaching for `dirs::cache_dir()` and reading, or writing, anything at all. This is
    /// also what keeps this test module out of the real user cache directory.
    #[test]
    fn with_no_server_configured_nothing_is_looked_up_or_written() {
        let mut app = crate::test_app::for_test();
        app.settings.immich_url = String::new();
        app.settings.immich_key = String::new();
        app.immich_albums = Some(Ok(vec![album("a", 1, Some("x"))]));
        assert!(app.immich_cached_album("a").is_none());
        assert!(app.immich_lists_dir().is_none());
        // And a store with no validator is a no-op, which is what an un-fetched rail produces.
        app.remember_immich_list(list_cache::Key::Album("a".into()), None, &[]);
    }
}
