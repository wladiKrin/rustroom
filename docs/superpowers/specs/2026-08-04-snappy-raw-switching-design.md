# Snappy RAW switching — design

Date: 2026-08-04
Status: approved

## Goal

Switching photos must never block the UI. Today `RustroomApp::open_path` decodes inline on
the UI thread, so every photo switch freezes the window for ~1 s. After this change:

- The canvas shows the target photo **immediately** (its filmstrip thumbnail), and the app
  keeps repainting — you can arrow through a folder at frame rate.
- The full, editable image arrives ~1 s after you *stop* moving, not ~1 s per photo passed.
- Stepping back to a recently viewed photo is instant (small buffer cache).

## Measurements that drive the design

Release build, Fuji X-T4 `.RAF` (6246×4170), `DecodeOptions::preview()` (half size):

| Phase | Time |
|---|---|
| file read (27 MB) | 12 ms |
| `RawImage::open` | 0.8 ms |
| **`unpack()` (RAF decompression)** | **840 ms** |
| `process()` (half-size demosaic + WB + gamma) | 111 ms |
| u16 → f32 conversion | 26 ms |
| **total `decode_for_edit`** | **~1.03 s** |
| baseline render (`downscale_to(1600)` + pipeline) | 42 ms |
| `extract_thumbnail` (embedded JPEG → 512 px) | 99 ms |

Two conclusions:

1. The cost is one opaque `unpack()` call, not many small steps.
2. The baseline render (42 ms) is small but still worth moving off the UI thread, since the
   worker already holds the buffer.

## Why "abandon", not "abort"

True mid-decode cancellation is *plumbable*: `libraw_set_progress_handler` is present in the
`rsraw-sys` bindings, `rsraw` exposes `AsMut<libraw_data_t>`, and a nonzero return from the
callback aborts with `LIBRAW_CANCELLED_BY_CALLBACK`. But 85% of the time is inside a single
`unpack()` of a compressed RAF, and LibRaw's Fuji compressed loader is not a dcraw-style
per-row loop that fires the progress callback frequently — so a cancel request arriving
mid-unpack would most likely take effect only once unpack had finished anyway. That is unsafe
FFI in `raw-decode` for an unverified payoff.

Instead we **abandon** superseded work: the doomed decode runs to completion on its own thread
(≤1 s of one core) while the newly selected photo starts decoding *immediately* in parallel.
Perceptually this is what cancellation would buy. Its result is dropped by sequence number —
except that it is still **inserted into the cache**, so a photo you skimmed past and return to
is already decoded.

**Documented follow-up:** if this still feels slow, revisit real cancellation as an isolated
change inside `raw-decode` (a `CancelToken` parameter on `decode_raw`), after an experiment
confirming the callback actually fires during `unpack` for the formats we care about.

## Architecture

Two new lib modules in `crates/app`, both unit-testable without egui, mirroring the existing
`render_engine.rs` / `thumb_loader.rs` pattern.

### `decode_worker.rs`

```rust
pub struct Decoded {
    pub buffer: Arc<ImageBuf>,
    pub label: String,
    pub baseline: RenderedFrame,   // neutral-edits render, done off the UI thread
}

pub struct DecodeResult {
    pub seq: u64,
    pub path: PathBuf,
    pub outcome: Result<Arc<Decoded>, String>,
}

pub struct DecodeWorker { /* ... */ }

impl DecodeWorker {
    /// `decode` is injected so tests can drive the worker without libraw.
    pub fn new(
        decode: impl Fn(&Path) -> Result<(ImageBuf, String), String> + Send + Sync + 'static,
        max_concurrent: usize,
        on_done: impl Fn() + Send + Sync + 'static,
    ) -> Self;

    /// Queue a decode; returns the sequence number identifying this request.
    pub fn request(&mut self, path: PathBuf, max_dim: usize) -> u64;

    /// Collect every finished result (call once per frame).
    pub fn poll(&mut self) -> Vec<DecodeResult>;
}
```

Behavior:

- **Thread per request**, capped at `max_concurrent` (2). Requests beyond the cap are held in
  a one-slot queue that keeps only the **newest**; when a slot frees (detected in `poll`), the
  queued request starts.
- Each spawned thread runs `decode(path)`, then `render_once(&buffer, &Edits::default(), max_dim)`
  for the baseline, then sends a `DecodeResult` and calls `on_done` (→ `ctx.request_repaint`).
- Sequence numbers are monotonic; the app compares against the request it is waiting on and
  ignores stale results (but still caches them).

### `buffer_cache.rs`

```rust
pub struct BufferCache { /* cap + Vec<(PathBuf, Arc<Decoded>)> */ }

impl BufferCache {
    pub fn new(cap: usize) -> Self;
    pub fn get(&mut self, path: &Path) -> Option<Arc<Decoded>>;  // refreshes recency
    pub fn insert(&mut self, path: PathBuf, decoded: Arc<Decoded>); // evicts LRU
    pub fn len(&self) -> usize;
}
```

An LRU over a `Vec` with linear scan — at capacity 3 anything cleverer is noise. Values are
`Arc<Decoded>`, so a cache hit and the live `Session` share one buffer instead of copying it.

### App state

```rust
struct PendingOpen {
    path: PathBuf,
    placeholder: Option<egui::TextureHandle>, // filmstrip thumbnail, when we have one
    start_at: std::time::Instant,             // debounce deadline
    seq: Option<u64>,                         // None until the decode is actually started
}

struct RustroomApp {
    pending: Option<PendingOpen>,
    decoder: DecodeWorker,
    cache: BufferCache,
    // ... existing fields; `session: Option<Session>` keeps its current meaning
}
```

`Session` is unchanged and still means "a fully decoded photo", so the render engine, crop,
histogram, before/after and export code need no knowledge of the half-open state.

Constants (in `main.rs`, next to `PREVIEW_MAX_DIM`):
`DECODE_DEBOUNCE: Duration = 180 ms`, `MAX_CONCURRENT_DECODES = 2`, `BUFFER_CACHE_CAP = 3`.

## Flow

**`open_path(ctx, path)`** (called by `open_dialog`, `open_entry`, arrow nav) becomes
non-blocking:

1. `persist_current_session()` — unchanged; writes the outgoing photo's sidecar.
2. **Cache hit** → build the `Session` from the cached `Arc<Decoded>` right now (same
   finalize path as below: load the sidecar, upload textures, start the render engine), clear
   `pending`, done. No thread, no placeholder, no debounce.
3. **Miss** → set `pending = PendingOpen { path, placeholder, start_at: now + DEBOUNCE, seq: None }`.
   The placeholder is `library.thumbs[i].clone()` when the path is in the library, else `None`.
   Status: `Decoding <file name>…`.

**Each frame, before drawing:**

- If `pending.seq.is_none()` and `now >= pending.start_at` → `pending.seq = Some(decoder.request(path, PREVIEW_MAX_DIM))`.
  Otherwise call `ctx.request_repaint_after(remaining)` so the deadline actually fires.
- `decoder.poll()`; for each result:
  - `Ok` → `cache.insert(path, decoded.clone())` **always** (even if stale — the work is done).
    If `Some(seq) == pending.seq`, finalize: load the sidecar, upload textures, construct the
    `Session`, clear `pending`, clear status.
  - `Err(e)` → if it matches `pending.seq`, set status `Decode failed: {e}` and clear `pending`,
    leaving the previous session on screen. Stale errors are ignored.

**Debounce effect:** holding an arrow key replaces `pending` (fresh `start_at`) each press
without ever starting a decode. Zero wasted work while scrolling; the decode begins 180 ms
after you settle.

## UI during a pending open

- **Canvas:** the placeholder thumbnail, scaled to fit the panel (ignoring zoom/pan — the new
  `Session` resets both anyway). With no thumbnail: the previous photo painted dimmed
  (`Color32::from_black_alpha`), or the existing empty-state label on first open.
- **Controls:** the right adjustment panel, Export, Undo/Redo and crop are wrapped in
  `ui.add_enabled_ui(self.pending.is_none(), …)`. Nothing can render against a buffer that
  isn't there, and it sidesteps any question of in-flight edits racing the incoming sidecar.
- **Arrow keys and filmstrip clicks stay live**, so you can keep browsing; each one supersedes
  the pending open. `library.selected` still updates immediately, so the strip highlight
  tracks your position with no lag.

## Memory

A half-size X-T4 buffer is 3123×2085×3×f32 ≈ 78 MB; its baseline frame is 1600×1068×4 ≈ 7 MB.
A 3-entry cache is therefore ~255 MB resident, shared (not duplicated) with the live session.
Two concurrent decodes add libraw's transient working set on top, so expect peaks around
500 MB while browsing hard. Acceptable for a desktop editor; the cache cap is one constant if
it ever isn't.

## Testing

**`decode_worker` unit tests** (fake decode fn — no libraw, no fixture):

1. A request produces a result whose `seq` matches, with the expected label and a baseline
   sized by `max_dim`.
2. A failing decode produces `Err` with the message, and the worker keeps serving later
   requests.
3. With `max_concurrent = 2` and a fake that sleeps, four rapid requests never run more than
   two decodes at once (the fake tracks a peak counter), and the newest request is still
   delivered.
4. When the cap is saturated, the one-slot queue keeps only the newest request — an older
   queued request is dropped rather than run.

**`buffer_cache` unit tests:** miss then hit; capacity is never exceeded; the least recently
*accessed* entry is evicted (not merely the oldest inserted); re-inserting an existing path
replaces without growing.

**Manual GUI check** (egui view code isn't unit-testable here): open a folder, hold → across
many photos — the strip and canvas keep up with no freeze; on release the photo becomes
editable in ~1 s; arrow back one photo — instant (cache); sliders/Export/undo are disabled
while decoding; a single-file open of a JPEG still works; a corrupt/unreadable file leaves the
previous photo up with an error in the status bar.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The manual GUI checks above all pass, in `--release`.
- `CLAUDE.md` updated: new modules in the crate layout, the M1.4a "inline decode" limitation
  struck, the abandon-vs-abort rationale and the cancellation follow-up recorded.
