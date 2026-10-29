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
    /// `GET /stacks` — every stack, in one call.
    ///
    /// ⚠⚠ **6.6 MB, ~2.9 s measured**, which is exactly why it is a job rather than an inline call:
    /// the grid regroups when it lands and nothing blocks meanwhile.
    Stacks,
    /// One month of the timeline: a date-ranged search.
    Bucket { bucket: String, label: String },
    /// `GET /albums/{id}`.
    Album { id: String, name: String },
    /// `GET /people` — the server's named, visible people (M10.18c).
    People,
    /// Every asset one person is in (M10.18c) — opened like an album.
    Person { id: String, name: String },
    /// `GET /assets/{id}/original`, streamed to `dest`.
    ///
    /// ⚠ `dest` is the **real** cache path. The worker writes to `dest.part` and renames; the
    /// fetcher is handed the `.part`.
    Original { asset_id: String, dest: PathBuf },
    /// M10.15d: the **current** list of the album or month a send acts on — fetched fresh, never from
    /// the cache, so the confirmation and the recorded prior values are the server's real ones.
    CheckMarks { source: MarksSource },
    /// M10.15d: `PUT /assets` per group, in order, on **its own** connection — the Write key, never
    /// the browse key — stopping at the first failure.
    SendMarks {
        conn: crate::immich_source::Connection,
        groups: Vec<crate::immich_send::Group>,
        undo: bool,
    },
}

/// Which list a send checks against (M10.15d).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarksSource {
    Album(String),
    /// A month's `YYYY-MM-DD` bucket.
    Bucket(String),
}

/// What came back.
pub enum Outcome {
    About(Result<String, String>),
    Albums(Result<Vec<immich::AlbumSummary>, String>),
    Buckets(Result<Vec<immich::Bucket>, String>),
    Stacks(Result<Vec<immich::Stack>, String>),
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
    People(Result<Vec<immich::Person>, String>),
    Person {
        id: String,
        name: String,
        result: Result<Vec<immich::Asset>, String>,
    },
    Original {
        dest: PathBuf,
        result: Result<(), String>,
    },
    CheckMarks {
        source: MarksSource,
        result: Result<Vec<immich::Asset>, String>,
    },
    /// The indices of the groups that landed, in order, and the first failure's message.
    SendMarks {
        landed: Vec<usize>,
        error: Option<String>,
        undo: bool,
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

/// How many originals download at once.
///
/// ⚠⚠ **Four, and a queue rather than a thread per job.** [`ImmichWorker::request`] spawns a thread and
/// `in_flight` only *dedupes* — it bounds nothing — so before this a bulk request for an 872-asset album
/// meant **872 threads and 872 concurrent HTTP requests** at one self-hosted server.
///
/// ⚠ The number follows `batch_export`'s four-wide default because these are network-bound against a
/// single machine; the *mechanism* is the queue below, not that precedent. ⚠ A guess, not a
/// measurement — one named constant to change when there is one.
pub const MAX_CONCURRENT_DOWNLOADS: usize = 4;

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
    /// Whether the **album list** fetch is outstanding, so an expanded section does not refetch on
    /// every frame.
    ///
    /// ⚠⚠ **Two flags, one per list, and deliberately not one shared flag.** A single flag had two
    /// faults: `request(Albums)` immediately followed by `request(Buckets)` dropped the second
    /// silently, and a *bucket* reply cleared the flag an outstanding *album* request had set. The
    /// rail survived both by re-requesting every frame; Reindex asks **once**, so it would not have.
    /// Same family as the guard that swallowed a second album click.
    pending_albums: bool,
    /// Whether the **bucket list** fetch is outstanding. See [`Self::pending_albums`].
    pending_buckets: bool,
    pending_stacks: bool,
    /// The people list's own guard — never shared (the album-click guard's lesson).
    pending_people: bool,
    /// Downloads waiting for a slot — see [`MAX_CONCURRENT_DOWNLOADS`].
    ///
    /// ⚠ Only `Job::Original` ever queues. A list fetch is one request and an open is never delayed.
    queued: std::collections::VecDeque<Job>,
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
            pending_albums: false,
            pending_buckets: false,
            pending_stacks: false,
            pending_people: false,
            queued: std::collections::VecDeque::new(),
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
        self.pending_albums || self.pending_buckets
    }

    /// Is the **album** list's automatic fetch outstanding? The Albums row's own flag.
    pub fn is_listing_albums(&self) -> bool {
        self.pending_albums
    }

    /// Is the **month** list's automatic fetch outstanding? The Timeline row's own flag.
    pub fn is_listing_buckets(&self) -> bool {
        self.pending_buckets
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
            // none, so without a guard that is a thread per frame. ⚠⚠ One guard **each**: they are
            // two independent fetches, and sharing a guard dropped whichever came second.
            Job::Albums => {
                if self.pending_albums {
                    return;
                }
                self.pending_albums = true;
                None
            }
            Job::Buckets => {
                if self.pending_buckets {
                    return;
                }
                self.pending_buckets = true;
                None
            }
            Job::Stacks => {
                // ⚠ Its own flag, never shared with the list kinds: a guard shared by two callers
                // with different retry behaviour is a bug waiting for the one that does not retry —
                // which `pending_list` already was, for exactly this pair of jobs.
                if self.pending_stacks {
                    return;
                }
                self.pending_stacks = true;
                None
            }
            // ⚠⚠ A **user-initiated open is never dropped.** It used to share the guard above, so
            // clicking a second album while the first was loading silently discarded the click —
            // reported in use as "it doesn't switch immediately". It bumps the sequence instead, and
            // `latest_open` lets the caller ignore a superseded reply.
            Job::People => {
                if self.pending_people {
                    return;
                }
                self.pending_people = true;
                None
            }
            Job::Album { .. } | Job::Bucket { .. } | Job::Person { .. } => {
                self.open_seq += 1;
                None
            }
            Job::About { .. } | Job::CheckMarks { .. } | Job::SendMarks { .. } => None,
        };

        let seq = matches!(
            job,
            Job::Album { .. } | Job::Bucket { .. } | Job::Person { .. }
        )
        .then_some(self.open_seq);

        // ⚠⚠ Past the cap, a download **waits** instead of spawning. `in_flight` already holds this
        // asset's id — the dedupe above inserted it — so a queued asset is still deduped, and `poll`
        // starts the next one as each completes.
        if matches!(job, Job::Original { .. }) && self.in_flight.len() > MAX_CONCURRENT_DOWNLOADS {
            self.queued.push_back(job);
            return;
        }
        self.spawn(job, asset_id, seq);
    }

    /// How many downloads are waiting for a slot.
    pub fn queued_downloads(&self) -> usize {
        self.queued.len()
    }

    /// Drop everything still waiting; returns how many were dropped.
    ///
    /// ⚠⚠ The **in-flight** downloads are deliberately left to finish — `DecodeWorker`'s idiom, which
    /// this project follows everywhere: *superseded work is abandoned, not aborted.* Killing a download
    /// mid-write would leave a `.part` file, which [`part_path`] exists to make survivable but which
    /// there is no reason to create on purpose.
    pub fn cancel_downloads(&mut self) -> usize {
        let n = self.queued.len();
        // ⚠ The ids of dropped jobs must leave `in_flight`, or those assets could never be requested
        // again for the life of the process.
        for job in self.queued.drain(..) {
            if let Job::Original { asset_id, .. } = job {
                self.in_flight.remove(&asset_id);
            }
        }
        n
    }

    /// The one place work starts, so `request` and `poll` cannot drift about how.
    fn spawn(&self, job: Job, asset_id: Option<String>, seq: Option<u64>) {
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
                // ⚠⚠ **After** the removal, not before: the cap counts `in_flight`, so starting the
                // next job first would see the finished one as still live and stall the queue one short
                // for ever. ⚠ This is the half that makes the queue drain — without it a bulk request
                // runs its first four and silently stops.
                if let Some(next) = self.queued.pop_front() {
                    let id = match &next {
                        Job::Original { asset_id, .. } => Some(asset_id.clone()),
                        _ => None,
                    };
                    self.spawn(next, id, None);
                }
            }
            // ⚠ Each reply clears **its own** flag. Clearing a shared one let a bucket reply free an
            // album fetch that was still running.
            match msg.outcome {
                Outcome::Albums(_) => self.pending_albums = false,
                Outcome::Buckets(_) => self.pending_buckets = false,
                Outcome::Stacks(_) => self.pending_stacks = false,
                Outcome::People(_) => self.pending_people = false,
                _ => {}
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
        let client = match wanted_conn(&job, &conn)
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
                    Job::People => Outcome::People(Err(e)),
                    Job::Person { id, name } => Outcome::Person {
                        id,
                        name,
                        result: Err(e),
                    },
                    Job::Buckets => Outcome::Buckets(Err(e)),
                    Job::Stacks => Outcome::Stacks(Err(e)),
                    Job::Bucket { bucket, label } => Outcome::Bucket {
                        bucket,
                        label,
                        result: Err(e),
                    },
                    Job::Original { dest, .. } => Outcome::Original {
                        dest,
                        result: Err(e),
                    },
                    Job::CheckMarks { source } => Outcome::CheckMarks {
                        source,
                        result: Err(e),
                    },
                    Job::SendMarks { undo, .. } => Outcome::SendMarks {
                        landed: vec![],
                        error: Some(e),
                        undo,
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
            Job::People => Outcome::People(client.people().map_err(|e| e.to_string())),
            Job::Person { id, name } => {
                let result = client.person(&id).map_err(|e| e.to_string());
                Outcome::Person { id, name, result }
            }
            Job::Buckets => Outcome::Buckets(client.buckets().map_err(|e| e.to_string())),
            Job::Stacks => Outcome::Stacks(client.stacks().map_err(|e| e.to_string())),
            Job::Bucket { bucket, label } => {
                let result = month_search(&client, &bucket);
                Outcome::Bucket {
                    bucket,
                    label,
                    result,
                }
            }
            Job::CheckMarks { source } => {
                let result = match &source {
                    MarksSource::Album(id) => client.album(id).map_err(|e| e.to_string()),
                    MarksSource::Bucket(bucket) => month_search(&client, bucket),
                };
                Outcome::CheckMarks { source, result }
            }
            Job::SendMarks { groups, undo, .. } => {
                let mut landed = Vec::new();
                let mut error = None;
                for (i, g) in groups.iter().enumerate() {
                    match client.update_marks(&g.ids, g.rating, g.favourite) {
                        Ok(()) => landed.push(i),
                        Err(e) => {
                            error = Some(e.to_string());
                            break;
                        }
                    }
                }
                Outcome::SendMarks {
                    landed,
                    error,
                    undo,
                }
            }
            Job::Original { asset_id, dest } => Outcome::Original {
                dest: dest.clone(),
                result: client.original(&asset_id, &dest).map_err(|e| e.to_string()),
            },
        }
    })
}

/// Which connection a job travels on (M10.15d): the Settings Test its **typed** one, a send its
/// **Write-key** one, everything else the browse connection the fetcher was built with.
fn wanted_conn(
    job: &Job,
    default: &Option<crate::immich_source::Connection>,
) -> Option<crate::immich_source::Connection> {
    match job {
        Job::About { conn: typed } => typed.clone(),
        Job::SendMarks { conn, .. } => Some(conn.clone()),
        _ => default.clone(),
    }
}

/// One month of the timeline — `bucket` is `YYYY-MM-DD`; the search wants the whole month.
fn month_search(client: &immich::Client, bucket: &str) -> Result<Vec<immich::Asset>, String> {
    let range = bucket
        .get(0..4)
        .and_then(|y| y.parse::<i32>().ok())
        .zip(bucket.get(5..7).and_then(|m| m.parse::<u32>().ok()))
        .and_then(|(y, m)| crate::immich_source::month_range(y, m));
    match range {
        Some((after, before)) => client
            .search(&immich::Filter::TakenBetween { after, before })
            .map_err(|e| e.to_string()),
        None => Err(format!("“{bucket}” is not a month this build understands")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        test_dir::path(&format!("iw-{tag}"))
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

    /// M10.15d: a send goes out on **its own** connection — the Write key — and nothing else does;
    /// the Settings Test keeps its typed one; everything else uses the browse connection.
    #[test]
    fn a_send_uses_its_own_connection_and_nothing_else_does() {
        let browse = crate::immich_source::Connection {
            url: "https://s".into(),
            key: "browse".into(),
        };
        let write = crate::immich_source::Connection {
            url: "https://s".into(),
            key: "write".into(),
        };
        let send = Job::SendMarks {
            conn: write.clone(),
            groups: vec![],
            undo: false,
        };
        assert_eq!(wanted_conn(&send, &Some(browse.clone())), Some(write));
        let check = Job::CheckMarks {
            source: MarksSource::Album("a".into()),
        };
        assert_eq!(
            wanted_conn(&check, &Some(browse.clone())),
            Some(browse.clone())
        );
        assert_eq!(
            wanted_conn(&Job::Albums, &Some(browse.clone())),
            Some(browse)
        );
    }

    /// M10.15d: a check and a send are both dispatched — no list guard swallows either.
    #[test]
    fn a_check_and_a_send_are_both_dispatched() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut w = ImmichWorker::new(
            Arc::new(move |job: Job| {
                c.fetch_add(1, Ordering::SeqCst);
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
                    _ => unreachable!(),
                }
            }),
            || {},
        );
        w.request(Job::CheckMarks {
            source: MarksSource::Bucket("2026-08-01".into()),
        });
        w.request(Job::SendMarks {
            conn: crate::immich_source::Connection {
                url: "https://s".into(),
                key: "w".into(),
            },
            groups: vec![],
            undo: true,
        });
        let start = std::time::Instant::now();
        let mut got = Vec::new();
        while got.len() < 2 && start.elapsed() < std::time::Duration::from_secs(5) {
            got.extend(w.poll());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(got.iter().any(|o| matches!(o, Outcome::CheckMarks { .. })));
        assert!(got
            .iter()
            .any(|o| matches!(o, Outcome::SendMarks { undo: true, .. })));
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

    /// A worker whose fetcher records every job it is handed, so a *dropped* request is visible.
    fn worker_recording_jobs() -> (ImmichWorker, Arc<std::sync::Mutex<Vec<Job>>>) {
        let seen: Arc<std::sync::Mutex<Vec<Job>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let rec = seen.clone();
        let w = ImmichWorker::new(
            Arc::new(move |job: Job| {
                rec.lock().unwrap().push(job.clone());
                match job {
                    Job::Buckets => Outcome::Buckets(Ok(vec![])),
                    Job::Stacks => Outcome::Stacks(Ok(vec![])),
                    _ => Outcome::Albums(Ok(vec![])),
                }
            }),
            || {},
        );
        (w, seen)
    }

    /// ⚠⚠ **The whole point of the queue.** `request` spawns a thread per job and `in_flight` only
    /// *dedupes*, so before this an 872-asset album meant **872 threads and 872 concurrent HTTP
    /// requests** at the user's own server.
    ///
    /// ⚠ Counts **concurrent** entries, not total calls: a fetcher counting calls would pass against no
    /// queue at all. ⚠ And it asserts the fixture actually overlaps, or the cap proves nothing.
    #[test]
    fn no_more_than_the_cap_download_at_once() {
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (l, pk) = (live.clone(), peak.clone());
        let mut w = ImmichWorker::new(
            Arc::new(move |_job: Job| {
                let n = l.fetch_add(1, Ordering::SeqCst) + 1;
                pk.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(25));
                l.fetch_sub(1, Ordering::SeqCst);
                Outcome::Original {
                    dest: PathBuf::from("x"),
                    result: Ok(()),
                }
            }),
            || {},
        );
        const N: usize = 30;
        for i in 0..N {
            w.request(Job::Original {
                asset_id: format!("a{i}"),
                dest: PathBuf::from(format!("/tmp/rr_dl_{i}.jpg")),
            });
        }
        let mut done = 0;
        settle(&mut w, |w| {
            done += w.poll().len();
            done >= N
        });
        assert_eq!(done, N, "every queued download must eventually run");

        let got = peak.load(Ordering::SeqCst);
        assert!(
            got > 1,
            "the fixture never overlapped, so the cap proves nothing (peak {got})"
        );
        assert!(
            got <= MAX_CONCURRENT_DOWNLOADS + 1,
            "{got} downloads ran at once; the cap is {MAX_CONCURRENT_DOWNLOADS}"
        );
    }

    /// ⚠ Cancel clears the **queue** and leaves the in-flight to finish — `DecodeWorker`'s idiom.
    /// ⚠⚠ And it must return the dropped ids to `in_flight`'s free pool, or those assets could never be
    /// requested again for the life of the process.
    #[test]
    fn cancelling_drops_the_queue_and_frees_those_assets() {
        let mut w = ImmichWorker::new(
            Arc::new(|_job: Job| {
                std::thread::sleep(std::time::Duration::from_millis(40));
                Outcome::Original {
                    dest: PathBuf::from("x"),
                    result: Ok(()),
                }
            }),
            || {},
        );
        for i in 0..20 {
            w.request(Job::Original {
                asset_id: format!("b{i}"),
                dest: PathBuf::from(format!("/tmp/rr_c_{i}.jpg")),
            });
        }
        assert!(
            w.queued_downloads() > 0,
            "the fixture must have queued something"
        );
        let dropped = w.cancel_downloads();
        assert!(dropped > 0);
        assert_eq!(w.queued_downloads(), 0);
        assert_eq!(w.cancel_downloads(), 0, "a second cancel is a no-op");

        // ⚠ A dropped asset can be asked for again.
        w.request(Job::Original {
            asset_id: "b19".to_string(),
            dest: PathBuf::from("/tmp/rr_c_19.jpg"),
        });
        assert!(
            w.queued_downloads() > 0 || w.is_downloading("b19"),
            "a cancelled asset must be requestable again"
        );
    }

    /// ⚠ The single-photo path is unchanged: one request never queues.
    #[test]
    fn a_lone_download_never_queues() {
        let (mut w, _seen) = worker_recording_jobs();
        w.request(Job::Original {
            asset_id: "solo".to_string(),
            dest: PathBuf::from("/tmp/rr_solo.jpg"),
        });
        assert_eq!(w.queued_downloads(), 0);
        assert!(w.is_downloading("solo"));
    }

    /// ⚠⚠ **The regression test for a shared guard.** One `pending_list` flag covered both list kinds,
    /// so asking for the album list and the bucket list in the same frame **silently dropped the
    /// second**. The rail never noticed, because it re-requests on every frame while its list is
    /// `None`; a one-shot request from Reindex would have been lost for good. Same shape as the guard
    /// that swallowed a second album click.
    #[test]
    fn an_albums_and_a_buckets_request_do_not_drop_each_other() {
        let (mut w, seen) = worker_recording_jobs();
        w.request(Job::Albums);
        w.request(Job::Buckets);
        settle(&mut w, |_| seen.lock().unwrap().len() >= 2);
        let jobs = seen.lock().unwrap().clone();
        assert!(
            jobs.contains(&Job::Albums),
            "the album list must be requested: {jobs:?}"
        );
        assert!(
            jobs.contains(&Job::Buckets),
            "and the bucket list must NOT be swallowed by it: {jobs:?}"
        );
    }

    /// ⚠ Each row of the rail reads its own list's flag: the Timeline must not say "Loading…"
    /// while only the albums are on their way, nor hold back its own request for them.
    #[test]
    fn each_list_reports_only_its_own_fetch() {
        let (mut w, _seen) = worker_recording_jobs();
        assert!(!w.is_listing_albums() && !w.is_listing_buckets());
        w.request(Job::Albums);
        assert!(w.is_listing_albums(), "the album fetch is outstanding");
        assert!(!w.is_listing_buckets(), "and says nothing about the months");
        w.request(Job::Buckets);
        assert!(w.is_listing_buckets());
    }

    /// ⚠ The per-frame guard must still guard: the rail asks on every frame while it has no list, and
    /// without this that is a thread per frame.
    #[test]
    fn a_second_albums_request_while_one_is_outstanding_is_still_dropped() {
        let (mut w, _seen) = worker_recording_jobs();
        w.request(Job::Albums);
        assert!(w.is_listing(), "one is outstanding");
        w.request(Job::Albums);
        // ⚠ Counted through the guard rather than the fetcher: the first job may already have
        // finished, and a race there would make this test flaky rather than wrong.
        assert!(w.is_listing());
    }

    /// ⚠⚠ A **bucket** reply used to clear the one flag an outstanding **album** request had set, so
    /// the rail could fire a second album fetch while the first was still running. Two flags, two
    /// lifetimes.
    #[test]
    fn a_buckets_reply_does_not_clear_an_outstanding_albums_request() {
        let mut w = ImmichWorker::new(
            Arc::new(|job: Job| match job {
                // ⚠ Albums is slow, Buckets is instant, so the bucket reply is certain to be drained
                // while the album fetch is still in flight — which is the situation being pinned.
                Job::Albums => {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    Outcome::Albums(Ok(vec![]))
                }
                _ => Outcome::Buckets(Ok(vec![])),
            }),
            || {},
        );
        w.request(Job::Albums);
        w.request(Job::Buckets);
        settle(&mut w, |w| {
            matches!(w.poll().first(), Some(Outcome::Buckets(_)))
        });
        assert!(
            w.is_listing(),
            "the album fetch is still outstanding, so a list is still in flight"
        );
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
