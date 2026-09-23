//! The one background worker for everything the UI needs from an Immich server.
//!
//! ⚠⚠ **One worker, not one per job kind.** `About` backs the Settings Test button, `Albums`
//! and `Album` the rail section, `Original` the open. Thumbnails are the single exception and do
//! **not** come through here: `ThumbLoader` already owns a pool whose queue the UI replaces
//! wholesale, which is exactly right for them and wrong for these.
//!
//! ⚠⚠ **Every server call goes through here, never inline in `show`.** A blocking HTTP call
//! inside a frame is a frozen window. So each is a hop: the click enqueues, the frame ends, the
//! result arrives on a later frame.
//!
//! ⚠ **No egui here.** The fetcher is injected, so every test below runs with no network.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// What the UI wants from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// `GET /server/about` — the Settings Test button.
    About {
        /// The connection to test, which is the one **typed** in the dialog rather than the one
        /// saved. Testing the saved key would defeat the point of testing before applying.
        conn: Option<crate::immich_source::Connection>,
    },
    /// `GET /albums`.
    Albums,
    /// `GET /timeline/buckets` — the whole library's month list, in one call.
    Buckets,
    /// One month of the timeline: a date-ranged search.
    Bucket { bucket: String, label: String },
    /// `GET /albums/{id}`.
    Album { id: String, name: String },
    /// `GET /assets/{id}/original`, streamed to `dest`.
    ///
    /// ⚠ `dest` is the **real** cache path. The worker writes to `dest.part` and renames; the
    /// fetcher is handed the `.part`.
    Original { asset_id: String, dest: PathBuf },
}

/// What came back.
pub enum Outcome {
    About(Result<String, String>),
    Albums(Result<Vec<immich::AlbumSummary>, String>),
    Buckets(Result<Vec<immich::Bucket>, String>),
    Bucket {
        bucket: String,
        label: String,
        result: Result<Vec<immich::Asset>, String>,
    },
    Album {
        id: String,
        name: String,
        result: Result<Vec<immich::Asset>, String>,
    },
    Original {
        dest: PathBuf,
        result: Result<(), String>,
    },
}

/// The injected fetcher. Takes the job **with `dest` already pointing at the `.part` file**.
pub type FetchFn = Arc<dyn Fn(Job) -> Outcome + Send + Sync>;

/// What travels on the channel.
///
/// ⚠ `asset_id` is carried beside the outcome rather than inferred from the path's stem. The stem
/// *is* the id today, because `cache_path` builds it that way — but making the in-flight
/// bookkeeping depend on that is a silent coupling between two files, and the day a cache name
/// gains a prefix the dedupe would simply stop working with nothing failing.
struct Msg {
    asset_id: Option<String>,
    /// The `open_seq` this reply belongs to, for a user-initiated open.
    seq: Option<u64>,
    outcome: Outcome,
}

/// Where the partial download lives until it is complete.
///
/// ⚠ A suffix rather than a sibling directory, so a crash leaves the evidence next to the thing
/// it belongs to and the cleanup is obvious.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

pub struct ImmichWorker {
    fetch: FetchFn,
    on_done: Arc<dyn Fn() + Send + Sync>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Which asset ids are being downloaded right now, so two opens do not race one path.
    in_flight: HashSet<String>,
    /// The sequence number of the most recent **user-initiated** open.
    ///
    /// ⚠⚠ Opens are **not** guarded like list fetches. Clicking album B while A is still loading must
    /// switch to B, and the older reply must not land on top of it afterwards. The
    /// `DecodeWorker` idiom: superseded requests are **abandoned, not aborted** — the doomed thread
    /// runs to completion and the caller drops its result by comparing sequence numbers.
    open_seq: u64,
    /// Whether a list request (albums or one album) is outstanding, so an expanded section does
    /// not refetch on every frame.
    pending_list: bool,
}

impl ImmichWorker {
    pub fn new(fetch: FetchFn, on_done: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = channel();
        Self {
            fetch,
            on_done: Arc::new(on_done),
            tx,
            rx,
            in_flight: HashSet::new(),
            open_seq: 0,
            pending_list: false,
        }
    }

    /// Is this asset's original being fetched right now?
    pub fn is_downloading(&self, asset_id: &str) -> bool {
        self.in_flight.contains(asset_id)
    }

    /// Is an **automatic list fetch** (the album list or the bucket list) outstanding?
    ///
    /// ⚠ Not user-initiated opens: those are never queued or guarded, so there is nothing to report.
    pub fn is_listing(&self) -> bool {
        self.pending_list
    }

    /// Enqueue a job. A duplicate download of one asset, and a second list while one is in
    /// flight, are dropped rather than queued.
    pub fn request(&mut self, job: Job) {
        let asset_id = match &job {
            Job::Original { asset_id, .. } => {
                if !self.in_flight.insert(asset_id.clone()) {
                    return;
                }
                Some(asset_id.clone())
            }
            // ⚠ The **automatic** list fetches. The rail asks for these on every frame while it has
            // none, so without a guard that is a thread per frame.
            Job::Albums | Job::Buckets => {
                if self.pending_list {
                    return;
                }
                self.pending_list = true;
                None
            }
            // ⚠⚠ A **user-initiated open is never dropped.** It used to share the guard above, so
            // clicking a second album while the first was loading silently discarded the click —
            // reported in use as "it doesn't switch immediately". It bumps the sequence instead, and
            // `latest_open` lets the caller ignore a superseded reply.
            Job::Album { .. } | Job::Bucket { .. } => {
                self.open_seq += 1;
                None
            }
            Job::About { .. } => None,
        };

        let seq = matches!(job, Job::Album { .. } | Job::Bucket { .. }).then_some(self.open_seq);
        let fetch = self.fetch.clone();
        let tx = self.tx.clone();
        let on_done = self.on_done.clone();
        std::thread::spawn(move || {
            let outcome = match &job {
                Job::Original { asset_id, dest } => run_download(&fetch, asset_id, dest),
                _ => fetch(job.clone()),
            };
            let _ = tx.send(Msg {
                asset_id,
                seq,
                outcome,
            });
            on_done();
        });
    }

    /// Take whatever has finished. Call once per frame.
    pub fn poll(&mut self) -> Vec<Outcome> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            if let Some(id) = &msg.asset_id {
                self.in_flight.remove(id);
            }
            if matches!(msg.outcome, Outcome::Albums(_) | Outcome::Buckets(_)) {
                self.pending_list = false;
            }
            // ⚠⚠ A superseded open is **dropped here**, not acted on: its grid would otherwise land
            // on top of the album the user actually asked for.
            if let Some(seq) = msg.seq {
                if seq != self.open_seq {
                    continue;
                }
            }
            out.push(msg.outcome);
        }
        out
    }
}

/// Fetch one original into `<dest>.part` and commit it by renaming.
///
/// ⚠⚠ The rename is the commit. Until it happens the real path does not exist, so nothing can
/// read a half-written RAW — and the whole cache design reads "the path exists" as "the bytes are
/// all there".
fn run_download(fetch: &FetchFn, asset_id: &str, dest: &Path) -> Outcome {
    let part = part_path(dest);
    // ⚠ A stale .part from a crashed run must not be appended to or mistaken for progress.
    let _ = std::fs::remove_file(&part);
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let out = fetch(Job::Original {
        asset_id: asset_id.to_string(),
        dest: part.clone(),
    });
    let failed = match &out {
        Outcome::Original { result, .. } => result.as_ref().err().cloned(),
        _ => Some("the fetcher answered a download with the wrong outcome".to_string()),
    };
    if let Some(e) = failed {
        let _ = std::fs::remove_file(&part);
        return Outcome::Original {
            dest: dest.to_path_buf(),
            result: Err(e),
        };
    }
    match std::fs::rename(&part, dest) {
        Ok(()) => Outcome::Original {
            dest: dest.to_path_buf(),
            result: Ok(()),
        },
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Outcome::Original {
                dest: dest.to_path_buf(),
                result: Err(format!("could not move the download into place: {e}")),
            }
        }
    }
}

/// The real fetcher: a `FetchFn` that talks to the server, or reports that none is configured.
///
/// ⚠ Built from a snapshot of the connection rather than reading `Settings` on the worker thread.
/// `Settings` lives on the UI side; a snapshot also means a job already in flight finishes against
/// the server it started with.
///
/// ⚠⚠ `Job::About` carries its **own** connection and that one wins. The Settings dialog tests
/// what is typed in it, which is by definition not yet the saved connection this closure captured
/// — testing the saved one would make the button useless for its only purpose.
pub fn live_fetcher(conn: Option<crate::immich_source::Connection>) -> FetchFn {
    Arc::new(move |job: Job| {
        let wanted = match &job {
            Job::About { conn: typed } => typed.clone(),
            _ => conn.clone(),
        };
        let client = match wanted
            .as_ref()
            .ok_or_else(|| "no Immich server is configured — set one in Settings".to_string())
            .and_then(|c| immich::Client::new(&c.url, &c.key).map_err(|e| e.to_string()))
        {
            Ok(c) => c,
            Err(e) => {
                return match job {
                    Job::About { .. } => Outcome::About(Err(e)),
                    Job::Albums => Outcome::Albums(Err(e)),
                    Job::Album { id, name } => Outcome::Album {
                        id,
                        name,
                        result: Err(e),
                    },
                    Job::Buckets => Outcome::Buckets(Err(e)),
                    Job::Bucket { bucket, label } => Outcome::Bucket {
                        bucket,
                        label,
                        result: Err(e),
                    },
                    Job::Original { dest, .. } => Outcome::Original {
                        dest,
                        result: Err(e),
                    },
                }
            }
        };
        match job {
            Job::About { .. } => {
                Outcome::About(client.about().map(|a| a.version).map_err(|e| e.to_string()))
            }
            Job::Albums => Outcome::Albums(client.albums().map_err(|e| e.to_string())),
            Job::Album { id, name } => {
                let result = client.album(&id).map_err(|e| e.to_string());
                Outcome::Album { id, name, result }
            }
            Job::Buckets => Outcome::Buckets(client.buckets().map_err(|e| e.to_string())),
            Job::Bucket { bucket, label } => {
                // ⚠ The bucket identifier is `YYYY-MM-DD`; the search wants the whole month.
                let range = bucket
                    .get(0..4)
                    .and_then(|y| y.parse::<i32>().ok())
                    .zip(bucket.get(5..7).and_then(|m| m.parse::<u32>().ok()))
                    .and_then(|(y, m)| crate::immich_source::month_range(y, m));
                let result = match range {
                    Some((after, before)) => client
                        .search(&immich::Filter::TakenBetween { after, before })
                        .map_err(|e| e.to_string()),
                    None => Err(format!("“{bucket}” is not a month this build understands")),
                };
                Outcome::Bucket {
                    bucket,
                    label,
                    result,
                }
            }
            Job::Original { asset_id, dest } => Outcome::Original {
                dest: dest.clone(),
                result: client.original(&asset_id, &dest).map_err(|e| e.to_string()),
            },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rustroom_iw_{}_{}", tag, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Block until `f` is true or the deadline passes, polling the worker.
    fn settle(w: &mut ImmichWorker, mut f: impl FnMut(&mut ImmichWorker) -> bool) {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            if f(w) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the worker never produced a result");
    }

    /// ⚠⚠ **The bug reported in use:** clicking a second album while the first was still loading
    /// silently discarded the click, because opens shared the list fetch's one-in-flight guard. Both
    /// must be dispatched.
    #[test]
    fn a_second_open_while_the_first_is_loading_is_not_dropped() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut w = ImmichWorker::new(
            Arc::new(move |job: Job| {
                c.fetch_add(1, Ordering::SeqCst);
                match job {
                    Job::Album { id, name } => Outcome::Album {
                        id,
                        name,
                        result: Ok(vec![]),
                    },
                    _ => unreachable!(),
                }
            }),
            || {},
        );
        w.request(Job::Album {
            id: "a".into(),
            name: "A".into(),
        });
        w.request(Job::Album {
            id: "b".into(),
            name: "B".into(),
        });
        let start = std::time::Instant::now();
        while calls.load(Ordering::SeqCst) < 2
            && start.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the second open was dropped — the click would do nothing"
        );
    }

    /// ⚠⚠ And the **older** reply must not land on top of the newer one. Without this, a slow album A
    /// finishing after B would replace the grid the user actually asked for.
    #[test]
    fn a_superseded_open_is_dropped_rather_than_overwriting_the_newer_one() {
        let mut w = ImmichWorker::new(
            Arc::new(|job: Job| match job {
                Job::Album { id, name } => Outcome::Album {
                    id,
                    name,
                    result: Ok(vec![]),
                },
                _ => unreachable!(),
            }),
            || {},
        );
        w.request(Job::Album {
            id: "a".into(),
            name: "A".into(),
        });
        w.request(Job::Album {
            id: "b".into(),
            name: "B".into(),
        });
        let mut names = Vec::new();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            for o in w.poll() {
                if let Outcome::Album { name, .. } = o {
                    names.push(name);
                }
            }
            if !names.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                for o in w.poll() {
                    if let Outcome::Album { name, .. } = o {
                        names.push(name);
                    }
                }
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            names,
            vec!["B".to_string()],
            "only the newest open may be acted on"
        );
    }

    /// ⚠ The automatic list fetch keeps its guard: the rail asks on every frame while it has none.
    #[test]
    fn a_second_album_list_fetch_while_one_is_in_flight_is_still_dropped() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut w = ImmichWorker::new(
            Arc::new(move |_job: Job| {
                c.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(80));
                Outcome::Albums(Ok(vec![]))
            }),
            || {},
        );
        w.request(Job::Albums);
        w.request(Job::Albums);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = w.poll();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one fetch for two frames");
    }

    #[test]
    fn a_buckets_job_comes_back_as_a_buckets_result() {
        let mut w = ImmichWorker::new(
            Arc::new(|job: Job| match job {
                Job::Buckets => Outcome::Buckets(Ok(vec![])),
                _ => unreachable!(),
            }),
            || {},
        );
        w.request(Job::Buckets);
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(matches!(got[0], Outcome::Buckets(Ok(_))));
    }

    /// ⚠ An unconfigured server must report, not panic — the section shows the message.
    #[test]
    fn a_buckets_job_without_a_connection_reports_it() {
        let f = live_fetcher(None);
        match f(Job::Buckets) {
            Outcome::Buckets(Err(msg)) => assert!(!msg.is_empty()),
            _ => panic!("an unconfigured fetcher must report an error"),
        }
    }

    /// ⚠⚠ A bucket identifier the build cannot parse must be an error outcome, not a search over
    /// a nonsense range.
    #[test]
    fn an_unparseable_bucket_is_an_error_not_a_wild_search() {
        let f = live_fetcher(Some(crate::immich_source::Connection {
            url: "http://immich.invalid".into(),
            key: "k".into(),
        }));
        match f(Job::Bucket {
            bucket: "not-a-month".into(),
            label: "?".into(),
        }) {
            Outcome::Bucket { result: Err(m), .. } => {
                assert!(m.contains("not a month"), "got: {m}")
            }
            _ => panic!("an unparseable bucket must be an error"),
        }
    }

    /// ⚠ Without a configured connection the fetcher must produce an error outcome rather than
    /// panicking or silently doing nothing — the section shows that message.
    #[test]
    fn the_real_fetcher_without_a_connection_reports_it() {
        let f = live_fetcher(None);
        match f(Job::Albums) {
            Outcome::Albums(Err(msg)) => assert!(!msg.is_empty()),
            _ => panic!("an unconfigured fetcher must report an error"),
        }
    }

    /// ⚠ An unreachable host must come back as an error, not a hang or a panic. `.invalid` is
    /// reserved by RFC 2606 and never resolves, so this needs no network.
    #[test]
    fn an_unreachable_server_is_an_error_not_a_panic() {
        let f = live_fetcher(Some(crate::immich_source::Connection {
            url: "http://immich.invalid".into(),
            key: "k".into(),
        }));
        match f(Job::Albums) {
            Outcome::Albums(Err(_)) => {}
            _ => panic!("an unreachable server must be an error"),
        }
    }

    /// ⚠⚠ A download job whose connection is missing must still clean up after itself: the
    /// worker creates the parent directory and a `.part` before the fetcher runs, and an error
    /// path that skipped the cleanup would leave litter behind on every failed open.
    #[test]
    fn an_unconfigured_download_leaves_no_part_file() {
        let dir = temp_dir("noconf");
        let dest = dir.join("55555555.RAF");
        let mut w = ImmichWorker::new(live_fetcher(None), || {});
        w.request(Job::Original {
            asset_id: "55555555".into(),
            dest: dest.clone(),
        });
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists(), "no .part may be left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_albums_job_comes_back_as_an_albums_result() {
        let mut w = ImmichWorker::new(
            Arc::new(|job: Job| match job {
                Job::Albums => Outcome::Albums(Ok(vec![])),
                _ => unreachable!(),
            }),
            || {},
        );
        w.request(Job::Albums);
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(matches!(got[0], Outcome::Albums(Ok(_))));
    }

    /// ⚠⚠ **The discipline the whole cache rests on.** A download that fails partway must leave
    /// *nothing* at the destination — the design reads "the path exists" as "the bytes are all
    /// there", so a truncated RAW sitting at the real path would be decoded as if it were whole.
    #[test]
    fn a_download_that_fails_partway_leaves_no_file_at_the_destination() {
        let dir = temp_dir("fail");
        let dest = dir.join("11111111.RAF");
        let mut w = ImmichWorker::new(
            Arc::new({
                let dest = dest.clone();
                move |job: Job| match job {
                    Job::Original { dest: part, .. } => {
                        // Write some bytes to the .part, then fail — a dropped connection.
                        std::fs::write(&part, b"half a raw").unwrap();
                        assert_ne!(part, dest, "the fetcher must be given the .part path");
                        Outcome::Original {
                            dest: dest.clone(),
                            result: Err("connection reset".into()),
                        }
                    }
                    _ => unreachable!(),
                }
            }),
            || {},
        );
        w.request(Job::Original {
            asset_id: "11111111".into(),
            dest: dest.clone(),
        });
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(
            !dest.exists(),
            "a failed download must not leave the real path in place"
        );
        assert!(
            !part_path(&dest).exists(),
            "and it must not leave a .part behind either"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_successful_download_renames_into_place_exactly_once() {
        let dir = temp_dir("ok");
        let dest = dir.join("22222222.RAF");
        let mut w = ImmichWorker::new(
            Arc::new(move |job: Job| match job {
                Job::Original { dest: part, .. } => {
                    std::fs::write(&part, b"a whole raw").unwrap();
                    Outcome::Original {
                        dest: part,
                        result: Ok(()),
                    }
                }
                _ => unreachable!(),
            }),
            || {},
        );
        w.request(Job::Original {
            asset_id: "22222222".into(),
            dest: dest.clone(),
        });
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(
            dest.is_file(),
            "a finished download must be at the real path"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"a whole raw");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ Two opens of the same photo must not race two downloads onto one path.
    #[test]
    fn the_same_asset_requested_twice_is_only_fetched_once() {
        let dir = temp_dir("dedupe");
        let dest = dir.join("33333333.RAF");
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut w = ImmichWorker::new(
            Arc::new(move |job: Job| {
                c.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(50));
                match job {
                    Job::Original { dest: part, .. } => {
                        std::fs::write(&part, b"x").unwrap();
                        Outcome::Original {
                            dest: part,
                            result: Ok(()),
                        }
                    }
                    _ => unreachable!(),
                }
            }),
            || {},
        );
        let job = Job::Original {
            asset_id: "33333333".into(),
            dest: dest.clone(),
        };
        w.request(job.clone());
        w.request(job);
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        got.extend(w.poll());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one fetch for two requests"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
