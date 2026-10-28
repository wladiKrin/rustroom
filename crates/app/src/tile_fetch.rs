//! The Map view's tiles (M10.18b): fetched in the background, cached on disk, never more than two at once.
//! No egui — the bin turns the bytes into textures.
//!
//! ⚠⚠ OpenStreetMap's tile policy is the design: a real User-Agent, at most [`MAX_IN_FLIGHT`] downloads,
//! only the tiles the view shows ([`TileFetcher::retain_only`] drops the rest from the queue), and the disk
//! cache honoured — a tile on disk never touches the network. A failed tile waits [`RETRY_AFTER`] before
//! it is asked for again: the map requests its tiles every frame, and an immediate retry would hammer.

use crate::geo::{cache_path, tile_url, TileId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// At most this many tiles download at once.
pub const MAX_IN_FLIGHT: usize = 2;
/// How long a failed tile waits before it is fetched again.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);

/// Fetch one URL's bytes.
pub type FetchTile = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;

/// The network fetch: ureq with Rustroom's User-Agent.
pub fn live_fetch() -> FetchTile {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(crate::place_search::USER_AGENT)
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    Arc::new(move |url| {
        let mut r = agent.get(url).call().map_err(|e| e.to_string())?;
        r.body_mut()
            .with_config()
            .limit(4 << 20)
            .read_to_vec()
            .map_err(|e| e.to_string())
    })
}

/// The tile queue, the downloads in flight, and what landed.
pub struct TileFetcher {
    root: Option<PathBuf>,
    template: String,
    fetch: FetchTile,
    on_done: Arc<dyn Fn() + Send + Sync>,
    tx: Sender<(TileId, Result<Vec<u8>, String>)>,
    rx: Receiver<(TileId, Result<Vec<u8>, String>)>,
    queue: VecDeque<TileId>,
    in_flight: HashSet<TileId>,
    failed: HashMap<TileId, Instant>,
}

impl TileFetcher {
    /// Tiles from `template` (`{z}/{x}/{y}`), cached under `root` (`None`: no disk cache).
    pub fn new(
        root: Option<PathBuf>,
        template: String,
        fetch: FetchTile,
        on_done: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (tx, rx) = channel();
        Self {
            root,
            template,
            fetch,
            on_done,
            tx,
            rx,
            queue: VecDeque::new(),
            in_flight: HashSet::new(),
            failed: HashMap::new(),
        }
    }

    /// The template these tiles come from.
    pub fn template(&self) -> &str {
        &self.template
    }

    /// Ask for `t` — a no-op while it is queued, downloading, or failed less than [`RETRY_AFTER`] ago.
    pub fn request(&mut self, t: TileId, now: Instant) {
        if self.in_flight.contains(&t) || self.queue.contains(&t) {
            return;
        }
        if let Some(at) = self.failed.get(&t) {
            if now.saturating_duration_since(*at) < RETRY_AFTER {
                return;
            }
            self.failed.remove(&t);
        }
        self.queue.push_back(t);
        self.start_some();
    }

    /// Drop every queued tile not in `wanted` — the view moved on, so they are never fetched.
    pub fn retain_only(&mut self, wanted: &[TileId]) {
        let keep: HashSet<&TileId> = wanted.iter().collect();
        self.queue.retain(|t| keep.contains(t));
    }

    /// What landed since the last call: each tile's PNG bytes, or why not.
    pub fn poll(&mut self, now: Instant) -> Vec<(TileId, Result<Vec<u8>, String>)> {
        let got: Vec<_> = self.rx.try_iter().collect();
        for (t, r) in &got {
            self.in_flight.remove(t);
            if r.is_err() {
                self.failed.insert(*t, now);
            }
        }
        self.start_some();
        got
    }

    /// Tiles queued or downloading.
    pub fn busy(&self) -> usize {
        self.queue.len() + self.in_flight.len()
    }

    fn start_some(&mut self) {
        while self.in_flight.len() < MAX_IN_FLIGHT {
            let Some(t) = self.queue.pop_front() else {
                return;
            };
            self.in_flight.insert(t);
            let path = self.root.as_ref().map(|r| cache_path(r, t));
            let url = tile_url(&self.template, t);
            let (fetch, tx, done) = (self.fetch.clone(), self.tx.clone(), self.on_done.clone());
            std::thread::spawn(move || {
                let r = load(path.as_deref(), &url, &fetch);
                let _ = tx.send((t, r));
                done();
            });
        }
    }
}

/// From the disk cache when it is there, else the network — written to the cache through a temporary
/// file, so a half-written tile never reads as a picture.
fn load(path: Option<&std::path::Path>, url: &str, fetch: &FetchTile) -> Result<Vec<u8>, String> {
    if let Some(p) = path {
        if let Ok(b) = std::fs::read(p) {
            return Ok(b);
        }
    }
    let bytes = fetch(url)?;
    if let Some(p) = path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension("part");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex};

    const T: &str = "https://tiles.example/{z}/{x}/{y}.png";

    fn tile(x: u32) -> TileId {
        TileId { z: 3, x, y: 1 }
    }

    /// Poll until `n` tiles have landed, under the 120 s ceiling.
    fn landed(f: &mut TileFetcher, n: usize) -> Vec<(TileId, Result<Vec<u8>, String>)> {
        let t0 = Instant::now();
        let mut out = Vec::new();
        while out.len() < n {
            assert!(t0.elapsed() < Duration::from_secs(120), "the fetch hung");
            out.extend(f.poll(Instant::now()));
            std::thread::sleep(Duration::from_millis(2));
        }
        out
    }

    fn counting(calls: Arc<AtomicUsize>) -> FetchTile {
        Arc::new(move |url| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(url.as_bytes().to_vec())
        })
    }

    #[test]
    fn a_fetched_tile_is_written_to_the_cache_and_then_never_fetched() {
        let d = test_dir::TempDir::new("tiles-cache");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut f = TileFetcher::new(
            Some(d.path().into()),
            T.into(),
            counting(calls.clone()),
            Arc::new(|| {}),
        );
        f.request(tile(4), Instant::now());
        let got = landed(&mut f, 1);
        assert_eq!(
            got[0].1.as_deref(),
            Ok(b"https://tiles.example/3/4/1.png".as_slice())
        );
        assert!(d.join("3/4/1.png").is_file(), "not cached");
        let mut again = TileFetcher::new(
            Some(d.path().into()),
            T.into(),
            counting(calls.clone()),
            Arc::new(|| {}),
        );
        again.request(tile(4), Instant::now());
        landed(&mut again, 1);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a cached tile went to the network"
        );
    }

    /// The policy: never more than two downloads at once — held behind a gate until the test has looked.
    #[test]
    fn at_most_two_fetch_at_once() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let fetch: FetchTile = {
            let (gate, now, peak) = (gate.clone(), now.clone(), peak.clone());
            Arc::new(move |_| {
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                let (m, c) = &*gate;
                let mut open = m.lock().unwrap();
                while !*open {
                    open = c.wait(open).unwrap();
                }
                now.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![1])
            })
        };
        let mut f = TileFetcher::new(None, T.into(), fetch, Arc::new(|| {}));
        for x in 0..5 {
            f.request(tile(x), Instant::now());
        }
        let t0 = Instant::now();
        while now.load(Ordering::SeqCst) < 2 {
            assert!(
                t0.elapsed() < Duration::from_secs(120),
                "the first two never started"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(f.busy(), 5);
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        landed(&mut f, 5);
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "the policy is two, whatever the constant says"
        );
    }

    #[test]
    fn retain_only_drops_queued_tiles_off_view() {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let fetch: FetchTile = {
            let (gate, calls) = (gate.clone(), calls.clone());
            Arc::new(move |_| {
                drop(gate.lock().unwrap());
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![1])
            })
        };
        let mut f = TileFetcher::new(None, T.into(), fetch, Arc::new(|| {}));
        for x in 0..6 {
            f.request(tile(x), Instant::now());
        }
        f.retain_only(&[tile(5)]);
        drop(held);
        let got = landed(&mut f, 3);
        let mut xs: Vec<u32> = got.iter().map(|(t, _)| t.x).collect();
        xs.sort();
        assert_eq!(
            xs,
            vec![0, 1, 5],
            "the two in flight and the one still in view"
        );
        std::thread::sleep(Duration::from_millis(20));
        assert!(f.poll(Instant::now()).is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_failed_fetch_waits_before_it_is_tried_again() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fetch: FetchTile = {
            let calls = calls.clone();
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err("offline".into())
            })
        };
        let mut f = TileFetcher::new(None, T.into(), fetch, Arc::new(|| {}));
        let t0 = Instant::now();
        f.request(tile(1), t0);
        assert!(landed(&mut f, 1)[0].1.is_err());
        f.request(tile(1), Instant::now());
        assert_eq!(f.busy(), 0, "retried at once");
        f.request(tile(1), Instant::now() + RETRY_AFTER);
        landed(&mut f, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
