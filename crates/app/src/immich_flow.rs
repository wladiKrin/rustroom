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
    pub(crate) fn immich_cache_root(&self) -> Option<std::path::PathBuf> {
        let conn = immich_source::connection(&self.settings)?;
        Some(immich_source::cache_root(&conn.url))
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
        }
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
        }
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
                Outcome::Original { dest, result } => match result {
                    // ⚠⚠ The file is now where the `PhotoId` always said it was, so the ordinary
                    // open path applies with no special case — and the view switch mirrors
                    // `open_entry`, which is the shape that already exists.
                    Ok(()) => {
                        self.open_path(ctx, PhotoId::original(dest));
                        self.view = AppView::Develop;
                    }
                    Err(e) => self.status = format!("Download failed: {e}"),
                },
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

    fn album(id: &str, count: u32, modified: Option<&str>) -> immich::AlbumSummary {
        immich::AlbumSummary {
            id: id.to_string(),
            album_name: format!("album {id}"),
            asset_count: count,
            last_modified_asset_timestamp: modified.map(|s| s.to_string()),
        }
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
