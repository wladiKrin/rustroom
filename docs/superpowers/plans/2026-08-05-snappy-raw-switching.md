# Snappy RAW Switching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make photo switching non-blocking — the target photo's thumbnail appears instantly, the ~1 s decode runs on a background thread, and recently viewed photos come back from a small cache.

**Architecture:** Two new unit-testable modules in `crates/app` — `decode_worker` (thread per request, capped at 2, sequence-numbered results, computes the neutral baseline frame off the UI thread) and `buffer_cache` (3-entry LRU of `Arc<Decoded>`). `RustroomApp` gains a `pending: Option<PendingOpen>` state that holds the placeholder texture and a debounce deadline; `Session` keeps its existing meaning of "fully decoded photo", so nothing downstream changes. Superseded decodes are **abandoned, not aborted** — their results are dropped by sequence number but still cached.

**Tech Stack:** Rust, `std::thread` + `std::sync::mpsc` (same pattern as the existing `render_engine.rs` / `thumb_loader.rs`), egui 0.35 for the view layer.

**Spec:** `docs/superpowers/specs/2026-08-04-snappy-raw-switching-design.md`

**Build environment (macOS):**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/app/src/buffer_cache.rs` | Create | LRU cache of decoded photos + its unit tests |
| `crates/app/src/decode_worker.rs` | Create | Background decode threads, seq numbering, concurrency cap + unit tests |
| `crates/app/src/lib.rs` | Modify | Register both modules |
| `crates/app/src/main.rs` | Modify | `pending` state, non-blocking `open_path`, per-frame pump, placeholder canvas, disabled controls while decoding |
| `CLAUDE.md` | Modify | Status, crate layout, struck M1.4a limitation, cancellation follow-up |

**Key egui 0.35 API notes for this work:** `Ui::set_enabled` does **not** exist — use `ui.disable()` at the top of a scope (or `ui.add_enabled_ui(cond, …)`). `egui::TextureHandle` is cheap to clone (it is `Arc`-backed). `ctx.request_repaint_after(duration)` is what makes a debounce deadline actually fire.

---

### Task 1: `buffer_cache` — LRU of decoded photos

**Files:**
- Create: `crates/app/src/buffer_cache.rs`
- Modify: `crates/app/src/lib.rs`

This task depends on the `Decoded` type from Task 2, so define it here in `decode_worker` first as a stub? **No** — do it the other way around: this module imports `crate::decode_worker::Decoded`, so Task 2's file must exist first. **Therefore: do Task 2 before Task 1 if you are working out of order.** The steps below assume `decode_worker.rs` already exists (it is created in Task 2).

- [ ] **Step 1: Write the failing test**

Create `crates/app/src/buffer_cache.rs` with only the imports and tests:

```rust
//! A tiny LRU cache of decoded photos, so stepping back to a recently viewed photo is
//! instant instead of costing another ~1 s decode.

use crate::decode_worker::Decoded;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_engine::RenderedFrame;
    use core_image::ImageBuf;

    fn decoded(label: &str) -> Arc<Decoded> {
        Arc::new(Decoded {
            buffer: Arc::new(ImageBuf::new(2, 2)),
            label: label.to_string(),
            baseline: RenderedFrame {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
        })
    }

    fn label_of(d: &Arc<Decoded>) -> String {
        d.label.clone()
    }

    #[test]
    fn miss_then_hit() {
        let mut c = BufferCache::new(3);
        assert!(c.get(Path::new("/a")).is_none());
        c.insert(PathBuf::from("/a"), decoded("a"));
        assert_eq!(c.get(Path::new("/a")).map(|d| label_of(&d)).as_deref(), Some("a"));
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut c = BufferCache::new(2);
        c.insert(PathBuf::from("/a"), decoded("a"));
        c.insert(PathBuf::from("/b"), decoded("b"));
        // Touch /a so /b becomes the least recently used.
        assert!(c.get(Path::new("/a")).is_some());
        c.insert(PathBuf::from("/c"), decoded("c"));
        assert_eq!(c.len(), 2);
        assert!(c.get(Path::new("/b")).is_none(), "/b should have been evicted");
        assert!(c.get(Path::new("/a")).is_some());
        assert!(c.get(Path::new("/c")).is_some());
    }

    #[test]
    fn reinserting_replaces_without_growing() {
        let mut c = BufferCache::new(2);
        c.insert(PathBuf::from("/a"), decoded("a"));
        c.insert(PathBuf::from("/a"), decoded("a2"));
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(Path::new("/a")).map(|d| label_of(&d)).as_deref(), Some("a2"));
    }

    #[test]
    fn capacity_is_never_exceeded() {
        let mut c = BufferCache::new(3);
        for i in 0..10 {
            c.insert(PathBuf::from(format!("/{i}")), decoded("x"));
        }
        assert_eq!(c.len(), 3);
    }
}
```

Register the module in `crates/app/src/lib.rs` (keep the list alphabetical — it goes first):

```rust
pub mod buffer_cache;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p app buffer_cache`
Expected: FAIL — `cannot find type BufferCache in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/app/src/buffer_cache.rs`, insert above the `#[cfg(test)] mod tests` block:

```rust
/// Least-recently-used cache of decoded photos, keyed by source path.
///
/// Values are `Arc<Decoded>`, so a cache hit shares the ~78 MB editing buffer with the live
/// session rather than copying it. `entries` is ordered least-recently-used first; at a
/// capacity of 3 a linear scan is cheaper than any index.
pub struct BufferCache {
    cap: usize,
    entries: Vec<(PathBuf, Arc<Decoded>)>,
}

impl BufferCache {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            entries: Vec::new(),
        }
    }

    /// Fetch a decoded photo, marking it most-recently-used.
    pub fn get(&mut self, path: &Path) -> Option<Arc<Decoded>> {
        let i = self.entries.iter().position(|(p, _)| p == path)?;
        let entry = self.entries.remove(i);
        let decoded = entry.1.clone();
        self.entries.push(entry);
        Some(decoded)
    }

    /// Insert (or replace) a photo, evicting the least recently used entry when full.
    pub fn insert(&mut self, path: PathBuf, decoded: Arc<Decoded>) {
        if let Some(i) = self.entries.iter().position(|(p, _)| *p == path) {
            self.entries.remove(i);
        }
        self.entries.push((path, decoded));
        while self.entries.len() > self.cap {
            self.entries.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p app buffer_cache`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/buffer_cache.rs crates/app/src/lib.rs
git commit -m "feat(app): LRU cache of decoded photos"
```

---

### Task 2: `decode_worker` — background decode with a concurrency cap

**Do this task first** if you are working strictly in order — Task 1's tests import `Decoded` from here.

**Files:**
- Create: `crates/app/src/decode_worker.rs`
- Modify: `crates/app/src/lib.rs`

**Reference for style:** `crates/app/src/render_engine.rs` (channels + worker threads) and `crates/app/src/thumb_loader.rs` (background producer drained per frame).

- [ ] **Step 1: Write the failing test**

Create `crates/app/src/decode_worker.rs` with the imports and test module only:

```rust
//! Background decode worker: decodes a file into the linear editing buffer off the UI
//! thread, one thread per request (capped), and renders the neutral baseline frame while
//! it already holds the buffer.
//!
//! Superseded requests are **abandoned, not aborted**: the doomed thread runs to completion
//! (libraw offers no practical mid-`unpack` cancellation) while the newly requested photo
//! decodes in parallel. Callers drop stale results by comparing sequence numbers.

use crate::render_engine::{render_once, RenderedFrame};
use core_image::ImageBuf;
use edit_model::Edits;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    fn tiny_decode(_path: &Path) -> Result<(ImageBuf, String), String> {
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        Ok((img, "FAKE".to_string()))
    }

    /// Poll until `n` results have arrived (or 5 s elapse).
    fn wait_for(worker: &mut DecodeWorker, n: usize) -> Vec<DecodeResult> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < n && Instant::now() < deadline {
            got.extend(worker.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        got
    }

    #[test]
    fn delivers_buffer_label_and_baseline() {
        let mut w = DecodeWorker::new(tiny_decode, 2, || {});
        let seq = w.request(PathBuf::from("/fake/a.raf"), 4);
        let got = wait_for(&mut w, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].seq, seq);
        let d = got[0].outcome.as_ref().expect("decode should succeed");
        assert_eq!(d.label, "FAKE");
        assert_eq!(d.buffer.width(), 8);
        // The baseline is rendered at max_dim, i.e. the 8x8 buffer downscaled to 4x4.
        assert_eq!((d.baseline.width, d.baseline.height), (4, 4));
    }

    #[test]
    fn propagates_errors_and_keeps_serving() {
        let mut w = DecodeWorker::new(
            |p: &Path| {
                if p.ends_with("bad.raf") {
                    Err("boom".to_string())
                } else {
                    tiny_decode(p)
                }
            },
            2,
            || {},
        );
        w.request(PathBuf::from("/fake/bad.raf"), 4);
        let got = wait_for(&mut w, 1);
        assert_eq!(
            got[0].outcome.as_ref().err().map(String::as_str),
            Some("boom")
        );

        w.request(PathBuf::from("/fake/good.raf"), 4);
        let got = wait_for(&mut w, 1);
        assert!(got[0].outcome.is_ok());
    }

    #[test]
    fn never_runs_more_than_max_concurrent_decodes() {
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (l, p) = (live.clone(), peak.clone());
        let mut w = DecodeWorker::new(
            move |path: &Path| {
                let n = l.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(80));
                l.fetch_sub(1, Ordering::SeqCst);
                tiny_decode(path)
            },
            2,
            || {},
        );
        for i in 0..4 {
            w.request(PathBuf::from(format!("/fake/{i}.raf")), 4);
        }
        let _ = wait_for(&mut w, 3);
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak concurrency {}",
            peak.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn queue_keeps_only_the_newest_request() {
        // Capacity 1: "a" occupies the worker while b, c, d queue — only the newest (d) runs.
        let mut w = DecodeWorker::new(
            |p: &Path| {
                std::thread::sleep(Duration::from_millis(60));
                tiny_decode(p)
            },
            1,
            || {},
        );
        w.request(PathBuf::from("/fake/a.raf"), 4);
        w.request(PathBuf::from("/fake/b.raf"), 4);
        w.request(PathBuf::from("/fake/c.raf"), 4);
        let seq_d = w.request(PathBuf::from("/fake/d.raf"), 4);

        let got = wait_for(&mut w, 2);
        let names: Vec<String> = got
            .iter()
            .map(|r| r.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["a.raf".to_string(), "d.raf".to_string()]);
        assert_eq!(got.last().unwrap().seq, seq_d);
    }
}
```

Register it in `crates/app/src/lib.rs` (alphabetical, after `crop_tool`):

```rust
pub mod decode_worker;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p app decode_worker`
Expected: FAIL — `cannot find type DecodeWorker in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/app/src/decode_worker.rs`, insert above the `#[cfg(test)] mod tests` block:

```rust
/// A decoded photo, ready to become an editing session.
pub struct Decoded {
    /// The linear working buffer (shared with the render engine and the cache).
    pub buffer: Arc<ImageBuf>,
    /// Display label, e.g. "Fujifilm X-T4" or "JPEG 6000×4000".
    pub label: String,
    /// Neutral-edits render, used for the before/after baseline texture.
    pub baseline: RenderedFrame,
}

/// The outcome of one decode request.
pub struct DecodeResult {
    /// Matches the value returned by [`DecodeWorker::request`]; anything older is stale.
    pub seq: u64,
    pub path: PathBuf,
    pub outcome: Result<Arc<Decoded>, String>,
}

type DecodeFn = Arc<dyn Fn(&Path) -> Result<(ImageBuf, String), String> + Send + Sync>;

struct Request {
    seq: u64,
    path: PathBuf,
    max_dim: usize,
}

/// Spawns a decode thread per request, up to `max_concurrent` at a time. Further requests
/// wait in a one-slot queue that always keeps the newest.
pub struct DecodeWorker {
    decode: DecodeFn,
    on_done: Arc<dyn Fn() + Send + Sync>,
    max_concurrent: usize,
    in_flight: usize,
    queued: Option<Request>,
    next_seq: u64,
    res_tx: Sender<DecodeResult>,
    res_rx: Receiver<DecodeResult>,
}

impl DecodeWorker {
    /// `decode` is injected so tests can drive the worker without libraw. `on_done` runs on
    /// the worker thread after each result (e.g. `ctx.request_repaint`).
    pub fn new(
        decode: impl Fn(&Path) -> Result<(ImageBuf, String), String> + Send + Sync + 'static,
        max_concurrent: usize,
        on_done: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (res_tx, res_rx) = channel();
        Self {
            decode: Arc::new(decode),
            on_done: Arc::new(on_done),
            max_concurrent: max_concurrent.max(1),
            in_flight: 0,
            queued: None,
            next_seq: 0,
            res_tx,
            res_rx,
        }
    }

    /// Queue a decode of `path`, rendering its baseline at `max_dim`. Returns the sequence
    /// number identifying this request.
    pub fn request(&mut self, path: PathBuf, max_dim: usize) -> u64 {
        self.next_seq += 1;
        let req = Request {
            seq: self.next_seq,
            path,
            max_dim,
        };
        if self.in_flight < self.max_concurrent {
            self.spawn(req);
        } else {
            // Newest wins: an older queued request is dropped, never run.
            self.queued = Some(req);
        }
        self.next_seq
    }

    /// Collect finished results and start the queued request if a slot freed up.
    /// Call once per frame.
    pub fn poll(&mut self) -> Vec<DecodeResult> {
        let mut out = Vec::new();
        while let Ok(res) = self.res_rx.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            out.push(res);
        }
        while self.in_flight < self.max_concurrent {
            match self.queued.take() {
                Some(req) => self.spawn(req),
                None => break,
            }
        }
        out
    }

    fn spawn(&mut self, req: Request) {
        let decode = self.decode.clone();
        let on_done = self.on_done.clone();
        let tx = self.res_tx.clone();
        self.in_flight += 1;
        std::thread::spawn(move || {
            let outcome = decode(&req.path).map(|(image, label)| {
                let buffer = Arc::new(image);
                let baseline = render_once(&buffer, &Edits::default(), req.max_dim);
                Arc::new(Decoded {
                    buffer,
                    label,
                    baseline,
                })
            });
            let _ = tx.send(DecodeResult {
                seq: req.seq,
                path: req.path,
                outcome,
            });
            on_done();
        });
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p app decode_worker`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/decode_worker.rs crates/app/src/lib.rs
git commit -m "feat(app): background decode worker with concurrency cap"
```

---

### Task 3: Non-blocking `open_path` in the app

**Files:**
- Modify: `crates/app/src/main.rs` (imports, constants, `RustroomApp`, `open_path`, `main`)

No unit tests — this is egui view/state wiring (see CLAUDE.md); verification is compile + clippy in this task and the manual GUI checks in Task 5.

- [ ] **Step 1: Imports and constants**

In `crates/app/src/main.rs`, replace the render_engine import line:

```rust
use rustroom_app::render_engine::{render_once, RenderEngine};
```

with (the baseline render now happens in the worker, so `render_once` is no longer used here):

```rust
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::decode_worker::{DecodeWorker, Decoded};
use rustroom_app::render_engine::RenderEngine;
```

and add below `use std::sync::Arc;`:

```rust
use std::time::{Duration, Instant};
```

Add next to `const PREVIEW_MAX_DIM: usize = 1600;`:

```rust
/// How long the selection must sit still before we commit to a decode. Holding an arrow key
/// therefore starts zero decodes until you settle.
const DECODE_DEBOUNCE: Duration = Duration::from_millis(180);
/// Superseded decodes are abandoned rather than aborted, so cap how many run at once.
const MAX_CONCURRENT_DECODES: usize = 2;
/// Recently viewed photos kept decoded in memory (~85 MB each).
const BUFFER_CACHE_CAP: usize = 3;
```

- [ ] **Step 2: Add the pending state and construct the app with a context**

Add this struct just above `struct RustroomApp {`:

```rust
/// A photo whose decode is scheduled or in flight. `Session` still means "fully decoded",
/// so nothing downstream needs to know about this half-open state.
struct PendingOpen {
    path: PathBuf,
    /// The filmstrip thumbnail shown while decoding, when the folder is loaded.
    placeholder: Option<egui::TextureHandle>,
    /// Debounce deadline — the decode starts once `Instant::now()` passes this.
    start_at: Instant,
    /// The worker sequence number, once the decode has actually been requested.
    seq: Option<u64>,
}
```

Add three fields to `struct RustroomApp`, after `session: Option<Session>,`:

```rust
    pending: Option<PendingOpen>,
    decoder: DecodeWorker,
    cache: BufferCache,
```

Delete the whole `impl Default for RustroomApp { … }` block and replace it with:

```rust
impl RustroomApp {
    fn new(ctx: &egui::Context) -> Self {
        let ctx2 = ctx.clone();
        Self {
            view: AppView::default(),
            library: None,
            session: None,
            pending: None,
            decoder: DecodeWorker::new(decode_for_edit, MAX_CONCURRENT_DECODES, move || {
                ctx2.request_repaint()
            }),
            cache: BufferCache::new(BUFFER_CACHE_CAP),
            status: String::new(),
            show_filmstrip: true,
        }
    }
}
```

In `fn main`, change the creation closure so it receives the context:

```rust
        Box::new(|cc| Ok(Box::new(RustroomApp::new(&cc.egui_ctx)))),
```

- [ ] **Step 3: Replace `open_path` with the non-blocking version**

Replace the entire existing `fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) { … }` (the one that calls `decode_for_edit` inline) with these four methods:

```rust
    /// Start opening `path`: instant from the cache, otherwise a debounced background decode
    /// with the filmstrip thumbnail as a placeholder. Never blocks the UI thread.
    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.persist_current_session();

        if let Some(decoded) = self.cache.get(&path) {
            self.pending = None;
            self.finalize_session(ctx, path, decoded);
            return;
        }

        let placeholder = self.thumbnail_for(&path);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.status = format!("Decoding {name}…");
        self.pending = Some(PendingOpen {
            path,
            placeholder,
            start_at: Instant::now() + DECODE_DEBOUNCE,
            seq: None,
        });
    }

    /// The library thumbnail for `path`, if the folder is loaded and that thumbnail has
    /// already arrived. `TextureHandle` is `Arc`-backed, so cloning it is cheap.
    fn thumbnail_for(&self, path: &std::path::Path) -> Option<egui::TextureHandle> {
        let lib = self.library.as_ref()?;
        let i = lib.entries.iter().position(|e| e.path == *path)?;
        lib.thumbs.get(i).cloned().flatten()
    }

    /// Build the editing `Session` from a decoded photo (cache hit or worker result).
    fn finalize_session(&mut self, ctx: &egui::Context, path: PathBuf, decoded: Arc<Decoded>) {
        let sidecar = sidecar_path(&path);
        let edits = if sidecar.exists() {
            edit_model::Edits::from_json_file(&sidecar).unwrap_or_default()
        } else {
            Edits::default()
        };

        let bf = &decoded.baseline;
        let baseline = ctx.load_texture(
            "baseline",
            egui::ColorImage::from_rgba_unmultiplied([bf.width, bf.height], &bf.rgba),
            egui::TextureOptions::LINEAR,
        );

        let ctx2 = ctx.clone();
        let engine = RenderEngine::new(decoded.buffer.clone(), move || ctx2.request_repaint());
        engine.request(edits.clone(), PREVIEW_MAX_DIM);

        self.session = Some(Session {
            path,
            engine,
            edits: edits.clone(),
            texture: None,
            baseline,
            hist: [[0; 256]; 3],
            make_model: decoded.label.clone(),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            history: History::new(edits),
            show_before: false,
            crop_mode: false,
            active_handle: None,
        });
        self.status.clear();
    }

    /// Per-frame pump: fire the debounce timer, then absorb finished decodes.
    fn pump_decodes(&mut self, ctx: &egui::Context) {
        if let Some(p) = &mut self.pending {
            if p.seq.is_none() {
                let now = Instant::now();
                if now >= p.start_at {
                    let path = p.path.clone();
                    // `p` borrows `self.pending`, the call borrows `self.decoder` — disjoint
                    // fields, so the borrow checker allows both at once.
                    p.seq = Some(self.decoder.request(path, PREVIEW_MAX_DIM));
                } else {
                    ctx.request_repaint_after(p.start_at - now);
                }
            }
        }

        for res in self.decoder.poll() {
            let is_current = self.pending.as_ref().and_then(|p| p.seq) == Some(res.seq);
            match res.outcome {
                Ok(decoded) => {
                    // Cache even superseded decodes — that work is already paid for.
                    self.cache.insert(res.path.clone(), decoded.clone());
                    if is_current {
                        self.pending = None;
                        self.finalize_session(ctx, res.path, decoded);
                    }
                }
                Err(e) => {
                    if is_current {
                        self.pending = None;
                        self.status = format!("Decode failed: {e}");
                    }
                }
            }
        }
    }
```

- [ ] **Step 4: Call the pump once per frame**

In `impl eframe::App for RustroomApp`, in `fn ui`, add this immediately after the macOS close-request block and before the "Pull any finished preview frame into a texture" comment:

```rust
        self.pump_decodes(&ctx);
```

- [ ] **Step 5: Verify it compiles clean**

Run:

```bash
cargo clippy -p app --all-targets -- -D warnings
```

Expected: no errors, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): non-blocking photo open — debounced background decode + cache"
```

---

### Task 4: Placeholder canvas and disabled controls while decoding

**Files:**
- Modify: `crates/app/src/main.rs` (central panel, right panel, toolbar)

- [ ] **Step 1: Draw the placeholder**

Add this free function next to `draw_crop_overlay` (near the bottom of `main.rs`):

```rust
/// Canvas while a decode is in flight: the target photo's thumbnail if we have one (scaled
/// up to fill, since thumbnails are ≤512 px), else the previous photo dimmed, else a spinner.
fn draw_pending_canvas(ui: &mut egui::Ui, pending: &PendingOpen, session: Option<&Session>) {
    let showing_placeholder = pending.placeholder.is_some();
    let fallback = session.and_then(|s| s.texture.as_ref());
    let Some(tex) = pending.placeholder.as_ref().or(fallback) else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
        });
        return;
    };

    let avail = ui.available_size();
    let tex_size = tex.size_vec2();
    // No `.min(1.0)` here: a 512 px thumbnail must scale *up* to fill the canvas.
    let fit = (avail.x / tex_size.x).min(avail.y / tex_size.y);
    let rect = egui::Rect::from_center_size(ui.max_rect().center(), tex_size * fit);
    egui::Image::new(tex).paint_at(ui, rect);

    if !showing_placeholder {
        // This is the *previous* photo — dim it so it reads as stale.
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_black_alpha(140),
        );
    }
}
```

- [ ] **Step 2: Use it in the central panel**

In `develop_body`, the central panel currently reads:

```rust
        egui::CentralPanel::default().show(ui, |ui| match &mut self.session {
```

Replace that line with:

```rust
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(p) = &self.pending {
                draw_pending_canvas(ui, p, self.session.as_ref());
                return;
            }
            match &mut self.session {
```

and close the extra block: the panel closure currently ends with

```rust
        });
    }
```

(right before `/// ←/→ move to the previous/next photo…`). Change it to:

```rust
            }
        });
    }
```

- [ ] **Step 3: Disable the adjustment panel while decoding**

In `develop_body`, inside the right panel closure, add this as the **first** statement (before `if let Some(s) = &mut self.session {`):

```rust
                if self.pending.is_some() {
                    ui.disable();
                }
```

(`Ui::set_enabled` does not exist in egui 0.35; `ui.disable()` greys out and blocks every widget added afterwards in this `Ui`.)

- [ ] **Step 4: Disable the Develop toolbar while decoding**

In `fn ui`, replace:

```rust
                if self.view == AppView::Develop && self.session.is_some() {
                    self.develop_toolbar(ui, &ctx);
                }
```

with:

```rust
                if self.view == AppView::Develop && self.session.is_some() {
                    let decoding = self.pending.is_some();
                    ui.add_enabled_ui(!decoding, |ui| self.develop_toolbar(ui, &ctx));
                }
```

Arrow-key navigation and filmstrip clicks deliberately stay live — `handle_develop_arrows` runs outside these scopes, so you can keep browsing while a decode is in flight.

- [ ] **Step 5: Verify it compiles clean**

Run:

```bash
cargo clippy -p app --all-targets -- -D warnings
```

Expected: no errors, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): thumbnail placeholder canvas + controls disabled while decoding"
```

---

### Task 5: Manual GUI verification

**Files:** none (verification only)

- [ ] **Step 1: Launch the release build**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo run --release -p app
```

- [ ] **Step 2: Work through the checklist**

Open a folder of RAWs, enter Develop, then confirm each:

1. **No freeze:** hold → across many photos — the filmstrip highlight and canvas keep up at frame rate; the window never stalls.
2. **Debounce:** while holding, the status bar keeps saying `Decoding <name>…` and no photo becomes editable; ~0.2 s after you release, one decode runs and the photo becomes editable in ~1 s.
3. **Placeholder:** during that wait the canvas shows the *target* photo's thumbnail (soft, filling the canvas), not the previous photo.
4. **Cache:** step back one photo — it appears instantly, no placeholder phase.
5. **Controls:** sliders, Export, Undo/Redo and Crop are greyed out while decoding, live again once the photo lands.
6. **Single file:** `Open file…` on a JPEG still works (no filmstrip, placeholder falls back to the dimmed previous photo or a spinner).
7. **Error path:** point it at a corrupt/unsupported file — the previous photo stays on screen and the status bar shows `Decode failed: …`.
8. **Edits survive switching:** adjust a slider, arrow away and back — the sidecar values are restored.

- [ ] **Step 3: Record the result**

If any check fails, stop and fix it before Task 6. If all pass, note it and continue.

---

### Task 6: Full verification and docs

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Run the whole workspace green**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Expected: all three succeed. (`cargo fmt --all` without `--check` fixes formatting.)

- [ ] **Step 2: Update CLAUDE.md**

1. Crate layout: extend the `app/` line so the lib list reads `lib=history/crop_tool/histogram/render_engine/thumb_loader/decode_worker/buffer_cache`.
2. **Current status:** add a "Snappy RAW switching (done)" paragraph at the top of the status body covering: `DecodeWorker` (thread per request, cap 2, seq-numbered results, baseline rendered off the UI thread), `BufferCache` (3-entry LRU of `Arc<Decoded>`, ~85 MB per entry), the 180 ms debounce, the thumbnail placeholder, controls disabled while decoding, and the measured phase split (`unpack` 840 ms of the ~1.03 s half-size decode).
3. Record the **abandon-vs-abort** decision and the follow-up: real cancellation via `libraw_set_progress_handler` is plumbable (`rsraw-sys` exposes it, `rsraw` gives `AsMut<libraw_data_t>`) but 85% of the time is inside one `unpack()` call whose callback granularity is unverified — revisit only with an experiment, behind a `CancelToken` on `decode_raw`.
4. Strike the M1.4a limitation "open decode is inline (UI freezes a few s during decode — threading it is a later refinement)" — it is now done.
5. Rewrite the **▶ RESUME HERE** block: snappy switching is done; the next candidates are HSL / vignette (color adjustments, using `stages/saturation.rs` as the template) and caching the downscaled preview so only the pipeline reruns per edit.
6. Add this plan and its spec to the `docs/superpowers/` file lists.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record snappy RAW switching"
```

- [ ] **Step 4: Final check**

```bash
git status --porcelain
```

Expected: empty. Do **not** push — the user pushes to GitHub themselves. Commit messages carry **no** `Co-Authored-By` trailer.

---

## Notes for the implementer

- **Task order:** `decode_worker.rs` (Task 2) must exist before `buffer_cache.rs` (Task 1) compiles, since the cache stores `Arc<Decoded>`. Either do Task 2 first, or create both files' skeletons together.
- **Don't** try to cancel an in-flight decode. The spec explains why abandoning is the design; a superseded thread finishing is expected behavior, not a leak.
- **Do** keep caching stale results — that is what makes skimming past a photo and coming back instant.
- The `Session` type and every consumer of it (render engine, crop, histogram, export, before/after) must stay untouched by this change. If you find yourself editing them, the `pending` state has leaked somewhere it shouldn't.
