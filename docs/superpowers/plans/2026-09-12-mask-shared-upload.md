# M8.3l — the mask's shared upload: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **Conventions that are not negotiable:** commit messages carry **no `Co-Authored-By`
> trailer**; work happens on a branch (`m8.3l-shared-upload`, already created); **never push** —
> the user pushes to `github.com/wladiKrin/rustroom` himself.

**Goal:** stop the mask uploading the same 78 MB plane to the GPU twice per frame — once as the
auto-mask gate's base, once as the `GpuChain`'s input.

**Architecture:** `GpuBrushRaster` uploads the base into a `GpuImage` and **deposits** it in a slot
on `GpuContext`; the mask's first sub-stage chain **takes** it instead of uploading. Three guards:
the chain must be `stages[0]` (marked statically in `walk`), the slot is cleared at the top of
every `MaskStage::apply` (a new defaulted `CoverageRasterizer::begin_mask`), and a deposit is taken
only at matching dimensions on the depositing thread.

**Tech stack:** Rust, wgpu 29, the existing `pipeline` / `gpu-pipeline` seam.

**Spec:** `docs/superpowers/specs/2026-09-12-mask-shared-upload-design.md`

---

## Facts established before writing this plan — do not re-derive them

- `MaskStage::apply` runs `build_map` (which rasterises, and is where the base is uploaded) →
  the inert short-circuit → `let mut adjusted = image.clone()` → `for stage in &self.stages`. So
  **`stages[0]` receives `image` byte for byte**, and nothing else does.
- `MaskStage` hands the rasteriser `BaseRef { plane: image.data(), w, h, x0, y0 }` where `w`/`h`
  are the incoming image's dimensions. `GpuImage::new(ctx, w, h)` allocates exactly `w*h*3` f32
  with `STORAGE | COPY_DST | COPY_SRC` — the identical layout, and a superset of the usages the
  gate's base binding needs.
- `walk` pushes a chain in **two** places: inside the loop's `None` arm (a chain followed by a CPU
  stage) and at the final `flush`. A chain is at index 0 exactly when `out.is_empty()` at the push.
- `walk`'s `inner.is_some()` guard sets **both** `accelerate_inner` and `set_coverage_rasterizer`,
  so `coverage_gpu` is `Some` whenever a sub-stage chain can exist. `begin_mask` therefore always
  runs when it matters.
- ⚠ The base is uploaded **only when `plan.gating_rows` is `Some`**, i.e. only when a stroke is
  auto-masked. A plain brush deposits nothing and its chain still uploads. Test fixtures must set
  `auto_mask: true`, and this is a real scope limit to record.
- ⚠ `GpuBrushRaster::rasterize` declines below `MIN_GPU_BOX_PX = 512 * 1024`, so a test brush must
  cover a box of at least 512 Ki px or nothing runs on the GPU at all.
- `gpu-pipeline` is a **library**, so `pub` items are public API and do not trip `dead_code`.
  `pub(crate)` items whose only callers are `#[cfg(test)]` **do** trip it — that is why Task 2 is
  one commit rather than three.

---

### Task 1: the `begin_mask` seam

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (the `CoverageRasterizer` trait)
- Modify: `crates/pipeline/src/stages/mask.rs` (`apply`)
- Test: `crates/pipeline/src/stages/mask.rs` (its existing `#[cfg(test)]` module)

- [ ] **Step 1: add the defaulted trait method**

In `crates/pipeline/src/lib.rs`, inside `pub trait CoverageRasterizer`, **above** `fn rasterize`:

```rust
    /// Called once at the top of every mask's `apply`, before any coverage is built.
    ///
    /// ⚠⚠ **This exists so an implementation can drop per-apply state, and without it a shared
    /// GPU plane would be taken by the WRONG frame.** A frame whose coverage comes from
    /// `BrushCache`'s prebuilt map rasterises nothing and so deposits nothing — and a sub-stage
    /// chain that then found a *previous* apply's plane still resident would render last frame's
    /// pixels. Silently, and as a plausible photograph.
    ///
    /// ⚠ Defaulted to nothing, so an implementation holding no per-apply state ignores it. It is
    /// deliberately **not** part of `rasterize`: a mask calls this even when it goes on to
    /// rasterise nothing at all, which is exactly the case that makes it necessary.
    fn begin_mask(&self) {}
```

- [ ] **Step 2: call it at the top of `MaskStage::apply`**

In `crates/pipeline/src/stages/mask.rs`, `fn apply(&self, image: ImageBuf) -> ImageBuf`, insert as
the **first** statement, above `let (w, h) = ...`:

```rust
        // ⚠⚠ **Before the early returns, not after them.** An offered rasteriser may hold state
        // scoped to one apply — `gpu-pipeline` parks the uploaded base plane there — and a mask
        // that returns early still has to let go of the previous apply's. Clearing only on the
        // path that rasterises would leave a stale plane resident exactly when this mask built no
        // coverage, which is the one case a later consumer must not be handed one.
        if let Some(r) = &self.coverage_gpu {
            r.begin_mask();
        }
```

- [ ] **Step 3: write the failing test**

In `mask.rs`'s test module, add (adapting the stub to whatever the existing
`#[cfg(test)]` rasteriser stub in this file is named — reuse it if one exists rather than adding a
second):

```rust
    /// ⚠⚠ **The carrier, and nothing else in the workspace can see this.** `begin_mask` changes
    /// no pixel by construction — it exists so an implementation can drop state — so deleting the
    /// call leaves every render identical and every other test green. That is the W5 failure mode,
    /// and a counting stub is the only guard against it.
    ///
    /// ⚠ It is called even for a mask that rasterises nothing, which is the case the hook exists
    /// for: a prebuilt coverage map deposits no plane, so the previous apply's must be let go of
    /// here or not at all.
    #[test]
    fn every_mask_apply_begins_by_telling_its_rasterizer() {
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl crate::CoverageRasterizer for Counting {
            fn begin_mask(&self) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            fn rasterize(
                &self,
                _plan: &crate::brush::DabPlan,
                _invert: bool,
                _base: Option<crate::brush::BaseRef<'_>>,
            ) -> Option<crate::brush::CoverageMap> {
                None
            }
        }

        let mut e = edit_model::Edits::default();
        e.masks = vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                exposure: 0.5,
                ..Default::default()
            },
        }];
        let mut tail = crate::build_pipeline_split(&e, RenderScale::FULL).1;
        let r: std::sync::Arc<dyn crate::CoverageRasterizer> =
            std::sync::Arc::new(Counting(seen.clone()));
        let mut offered = 0usize;
        for s in tail.stages_mut() {
            if s.name() == "mask" {
                s.set_coverage_rasterizer(&r);
                offered += 1;
            }
        }
        assert_eq!(offered, 1, "fixture check: exactly one mask must be offered one");

        let out = tail.run(ImageBuf::new(8, 8));
        assert_eq!(out.width(), 8);
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the mask must tell its rasteriser that an apply has begun"
        );
    }
```

⚠ If `Pipeline` has no `stages_mut()`, do **not** add one — instead build the `MaskStage` the way
the existing tests in this file do and offer the rasteriser directly. Check first:
`grep -n "stages_mut\|fn set_coverage_rasterizer" crates/pipeline/src/lib.rs crates/pipeline/src/stages/mask.rs`

- [ ] **Step 4: run it and watch it pass, then sabotage**

```bash
cargo test -p pipeline every_mask_apply_begins_by_telling_its_rasterizer
```
Expected: PASS.

Sabotage: comment out the `r.begin_mask()` call in `apply`. Re-run.
Expected: FAIL with `left: 0, right: 1`. **Confirm a `test result:` line is printed** — a sabotage
that does not compile reads exactly like one that passes. Restore.

- [ ] **Step 5: gate and commit**

```bash
cargo fmt --all
```
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```
```bash
git add -A && git commit -F - <<'EOF'
feat(pipeline): a mask tells its coverage rasteriser when an apply begins

`CoverageRasterizer::begin_mask` is a defaulted no-op that `MaskStage::apply`
calls once, above every early return. It exists so an implementation can drop
state scoped to one apply — gpu-pipeline is about to park an uploaded base
plane there — and it must run even for a mask that rasterises nothing, because
a prebuilt coverage map deposits no plane and the previous apply's must be let
go of here or not at all.

Guarded by a counting stub: the hook changes no pixel, so deleting the call
leaves every render identical and every other test green.
EOF
```

---

### Task 2: the shared plane, deposited and taken

⚠⚠ **This is one commit and cannot be split.** `SharedPlane::take` has no production caller until
`GpuChain` uses it, and a `pub(crate)` item whose only callers are `#[cfg(test)]` fails
`-D warnings` in a library — the trap this project has now recorded five times. The counters are
`pub` for the same reason `GpuChain::transfer_counts` is: a `pub` item in a lib is public API.

**Files:**
- Create: `crates/gpu-pipeline/src/shared_plane.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs` (declare and re-export the module)
- Modify: `crates/gpu-pipeline/src/context.rs` (the field and its accessor)
- Modify: `crates/gpu-pipeline/src/brush_raster.rs` (deposit, and `begin_mask`)
- Modify: `crates/gpu-pipeline/src/chain.rs` (take)
- Modify: `crates/gpu-pipeline/src/accelerate.rs` (mark index 0 of an inner walk)

- [ ] **Step 1: write `shared_plane.rs`**

```rust
//! A one-shot hand-off slot for a GPU buffer two collaborators both need.
//!
//! ⚠⚠ **Why this exists: a masked frame uploads the same plane twice.** `MaskStage::apply` hands
//! the auto-mask gate `image.data()`, then hands its first sub-stage a clone of that same
//! `image` — byte for byte the same pixels, in the same packed `w*h*3` f32 layout a `GpuImage`
//! holds. Measured on the catalogue's masked photograph at rung 3200: ~2.1 ms of host memcpy plus
//! ~6-10 ms of device copy, paid twice.
//!
//! ⚠ Generic over the payload **so its rules are testable with no GPU at all**. What this type
//! enforces is bookkeeping — dimensions, ownership, one use — and none of that is about wgpu.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// What a deposit carries besides the payload.
struct Deposit<T> {
    payload: T,
    w: usize,
    h: usize,
    /// ⚠⚠ **`GpuContext::shared()` is process-wide, so without this a preview render and an
    /// export render on two threads could hand each other a plane.** A legitimate hand-off is
    /// always same-thread: `MaskStage::apply` builds its coverage and runs its sub-stages on one
    /// call stack. Stamping the thread makes the cross-thread case unrepresentable rather than
    /// merely unlikely.
    by: std::thread::ThreadId,
}

/// A slot holding at most one deposited payload.
#[derive(Default)]
pub struct SharedPlane<T> {
    slot: Mutex<Option<Deposit<T>>>,
    deposits: AtomicUsize,
    takes: AtomicUsize,
}

impl<T> SharedPlane<T> {
    /// Empty the slot, dropping whatever is in it.
    ///
    /// ⚠⚠ **This is what stops a stale plane being taken, and it is the guard most likely to be
    /// "simplified" away.** A frame whose coverage comes from a cache rasterises nothing and so
    /// deposits nothing; without a clear, its sub-stage chain would find the *previous* frame's
    /// plane and render those pixels. The failure is a plausible photograph, not an error.
    pub fn clear(&self) {
        if let Ok(mut g) = self.slot.lock() {
            *g = None;
        }
    }

    /// Park `payload`, replacing anything already there.
    pub fn deposit(&self, w: usize, h: usize, payload: T) {
        if let Ok(mut g) = self.slot.lock() {
            *g = Some(Deposit {
                payload,
                w,
                h,
                by: std::thread::current().id(),
            });
            self.deposits.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Take the deposit if it is this thread's and describes a `w`x`h` plane.
    ///
    /// ⚠ **Take and clear — one use.** Leaving it would let a second consumer in the same apply
    /// (a mask whose sub-stages form two chains around a CPU stage) take a plane the first has
    /// already mutated in place.
    pub fn take(&self, w: usize, h: usize) -> Option<T> {
        let mut g = self.slot.lock().ok()?;
        let d = g.take()?;
        if d.w == w && d.h == h && d.by == std::thread::current().id() {
            self.takes.fetch_add(1, Ordering::Relaxed);
            Some(d.payload)
        } else {
            // ⚠ Already removed by the `take()` above, deliberately: a deposit that does not match
            // is stale by definition, and leaving it for a later consumer is the staleness this
            // whole type exists to prevent.
            None
        }
    }

    /// How many deposits this slot has accepted.
    pub fn deposits(&self) -> usize {
        self.deposits.load(Ordering::Relaxed)
    }

    /// How many deposits have been taken.
    ///
    /// ⚠⚠ **The observable, and it has to exist for `GpuChain::transfer_counts`' reason.** A
    /// shared upload changes no pixel any test can read — the same bytes reach the same buffer by
    /// a different route — so without a counter the whole mechanism could be deleted and every
    /// equivalence test in the workspace would stay green.
    ///
    /// ⚠ Read it only from a **privately constructed** `GpuContext`. On the process-wide
    /// `shared()` one it is contaminated by whatever else is rendering, which is the bug
    /// `GpuChain`'s per-chain counters were introduced to fix.
    pub fn takes(&self) -> usize {
        self.takes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deposit_is_taken_once_and_then_gone() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        assert_eq!(s.take(4, 5), Some(7));
        assert_eq!(s.take(4, 5), None, "a deposit is good for exactly one use");
        assert_eq!((s.deposits(), s.takes()), (1, 1));
    }

    #[test]
    fn an_empty_slot_yields_nothing() {
        let s: SharedPlane<u32> = Default::default();
        assert_eq!(s.take(4, 5), None);
        assert_eq!(s.takes(), 0);
    }

    /// ⚠ The dimensions are what say the deposit describes the plane being asked for. A mask
    /// rendering a region and a chain rendering the frame would otherwise trade buffers.
    #[test]
    fn a_deposit_of_another_size_is_refused() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        assert_eq!(s.take(5, 4), None, "transposed dimensions are not a match");
        assert_eq!(s.takes(), 0);
    }

    /// ⚠⚠ A refused deposit must also be **gone**, not left for the next asker — it is stale by
    /// definition, and leaving it is exactly the staleness this type exists to prevent.
    #[test]
    fn a_refused_deposit_is_dropped_rather_than_left_behind() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        assert_eq!(s.take(9, 9), None);
        assert_eq!(s.take(4, 5), None, "the stale deposit must not survive");
    }

    #[test]
    fn clearing_discards_the_deposit() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        s.clear();
        assert_eq!(s.take(4, 5), None);
    }

    /// ⚠⚠ The cross-thread case, which `GpuContext::shared()` being process-wide makes real: a
    /// preview render and an export render must never hand each other a plane.
    #[test]
    fn another_threads_deposit_is_refused() {
        let s: std::sync::Arc<SharedPlane<u32>> = Default::default();
        let d = s.clone();
        std::thread::spawn(move || d.deposit(4, 5, 7)).join().unwrap();
        assert_eq!(
            s.take(4, 5),
            None,
            "a plane deposited on another thread must not be taken here"
        );
        assert_eq!(s.takes(), 0);
    }
}
```

- [ ] **Step 2: declare and export it**

In `crates/gpu-pipeline/src/lib.rs`, add `mod shared_plane;` beside the other module declarations
and `pub use shared_plane::SharedPlane;` beside the other re-exports. Match the file's existing
style exactly — check with `sed -n '1,60p' crates/gpu-pipeline/src/lib.rs` first.

- [ ] **Step 3: put the slot on `GpuContext`**

In `crates/gpu-pipeline/src/context.rs`, add to the struct after `cache`:

```rust
    /// The base plane a mask's gate uploaded, offered to that mask's first sub-stage chain.
    ///
    /// ⚠⚠ **Here because it is the only place both participants can reach.** `GpuBrushRaster`
    /// and `GpuChain` are built separately by `accelerate` and never meet; both hold this
    /// `Arc<GpuContext>`. It is **not** part of `cache` deliberately: the cache's rule is "outlive
    /// the render", and this one's is the exact opposite — `MaskStage::apply` clears it at the top
    /// of every apply, so a plane never survives the mask that uploaded it.
    shared_plane: crate::SharedPlane<crate::GpuImage>,
```

and in `GpuContext::new`'s `Some(Self { .. })`, `shared_plane: Default::default(),`.

Then an accessor beside the other `pub fn`s:

```rust
    /// The slot a mask's gate parks its uploaded base plane in.
    pub fn shared_plane(&self) -> &crate::SharedPlane<crate::GpuImage> {
        &self.shared_plane
    }
```

⚠ If `GpuContext` derives `Debug`, `SharedPlane` must too — add `#[derive(Debug)]` needs `T: Debug`
and `GpuImage` is `Debug`? Check: `grep -n "derive" crates/gpu-pipeline/src/image.rs`. If it is
not, implement `Debug` for `SharedPlane<T>` by hand printing only the counters, or drop `Debug`
from `GpuContext` — **prefer the hand-written impl**, since `GpuContext: Debug` is existing API.

- [ ] **Step 4: deposit from `GpuBrushRaster`**

In `crates/gpu-pipeline/src/brush_raster.rs`, in `run()`, replace the base-buffer creation arm.
The current shape is:

```rust
            (Some(b), Some(_)) => {
                let plane = b.plane;
                let bytes = std::mem::size_of_val(plane) as u64;
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("brush-base"),
                    size: bytes,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
```

Replace those lines (keeping everything from `let t_stage = ...` onward) with:

```rust
            (Some(b), Some(_)) => {
                let plane = b.plane;
                let bytes = std::mem::size_of_val(plane) as u64;
                // ⚠⚠ **A `GpuImage`, not a bare buffer, and that is the whole mechanism.** Its
                // layout is `w*h*3` f32 — bit for bit what `ImageBuf::data()` holds and what a
                // chain uploads — and its usages are a superset of the gate's, so the very
                // allocation the gate reads from can be handed on to this mask's first sub-stage
                // instead of a second copy of the same bytes.
                //
                // ⚠ Declining on a length mismatch rather than assuming: a `BaseRef` whose plane
                // does not match its own dimensions is a programming error, and every path here
                // fails toward the CPU.
                if plane.len() != b.w.checked_mul(b.h)?.checked_mul(3)? {
                    return None;
                }
                let img = crate::GpuImage::new(ctx, b.w, b.h)?;
```

and then, wherever the old code used `buf`, use `img.buffer.clone()`-free access — specifically:

- the tuple built at the end of that arm becomes
  `(img, b.w as u32, b.h as u32, b.x0 as u32, b.y0 as u32, 1u32)` and the `match`'s other arm's
  first element must become a `GpuImage` too. ⚠ That is awkward: the `_ => (storage("brush-base",
  &[]), 1, 1, 0, 0, 0u32)` arm has no image. **Instead keep the match producing a plain
  `wgpu::Buffer` for the binding and carry the `GpuImage` separately**:

```rust
        let mut staging = None;
        let mut d_stage = std::time::Duration::ZERO;
        // ⚠ Carried beside the binding rather than inside the match, because the no-gate arm has
        // no image to produce and a `GpuImage` of a dummy size would be a lie.
        let mut resident: Option<crate::GpuImage> = None;
        let (base_buf, img_w, img_h, bx0, by0, has_base) = match (base, plan.gating_rows) { ... };
```

In the gating arm, after the staging copy is recorded, set
`resident = Some(img);` and produce `(resident.as_ref().unwrap().buffer.clone(), ...)`.

⚠ `wgpu::Buffer` is `Clone` (it is a refcounted handle) — **verify** with
`grep -rn "impl Clone for Buffer\|pub struct Buffer" $(find ~/.cargo/registry -name "buffer.rs" -path "*wgpu-29*" 2>/dev/null | head -1)`
or simply let the compiler say so. If it is not `Clone`, bind the binding resource from
`resident.as_ref().unwrap().buffer` at the bind-group site instead and keep `base_buf` an
`Option<wgpu::Buffer>` for the no-base arm.

Then, immediately after the existing `ctx.cache.put_staging(st)` (i.e. after `poll(Wait)` has
confirmed the submission finished):

```rust
        // ⚠⚠ **Deposited only after `poll(Wait)`, the same rule that governs when the staging
        // buffer goes back to the pool.** The GPU reads this buffer during the submit; handing it
        // on any earlier would let the sub-stage chain mutate a plane still being read.
        if let Some(img) = resident {
            ctx.shared_plane().deposit(img.width(), img.height(), img);
        }
```

- [ ] **Step 5: implement `begin_mask` on `GpuBrushRaster`**

In the `impl pipeline::CoverageRasterizer for GpuBrushRaster` block:

```rust
    /// ⚠⚠ **This is guard 2 of the shared upload, and without it a cached-coverage frame renders
    /// the PREVIOUS frame's pixels.** Such a frame rasterises nothing, so it deposits nothing —
    /// and a sub-stage chain finding the last apply's plane still resident would take it. Clearing
    /// at the top of every apply bounds a deposit's life to the mask that made it.
    fn begin_mask(&self) {
        self.ctx.shared_plane().clear();
    }
```

⚠ Check the field name: `grep -n "struct GpuBrushRaster" -A 10 crates/gpu-pipeline/src/brush_raster.rs`.

- [ ] **Step 6: take it in `GpuChain`**

In `crates/gpu-pipeline/src/chain.rs`, add to `pub struct GpuChain`:

```rust
    /// Whether this chain may take a base plane deposited by a mask's gate.
    ///
    /// ⚠⚠ **False unless `walk` marked it, and only the FIRST sub-stage of a mask is ever
    /// marked.** `MaskStage::apply` clones the image and pipes it through its sub-stages in order,
    /// so only `stages[0]` receives the plane the gate was given; a chain at index 1 gets whatever
    /// index 0 produced. Making this opt-in means the rule is decided where the position is known
    /// and nowhere else.
    take_shared: bool,
```

`new` sets `take_shared: false`. Add:

```rust
    /// Allow this chain to take a deposited base plane instead of uploading its input.
    ///
    /// ⚠ Called by `accelerate::walk` and by nothing else — see the field.
    pub fn allow_shared_input(&mut self) {
        self.take_shared = true;
    }
```

In `run_with`, replace

```rust
        let img = GpuImage::new(ctx, image.width(), image.height())?;
        let bytes = img.bytes();
```
…through the `enc.copy_buffer_to_buffer(&staging, 0, &img.buffer, 0, bytes);` line with:

```rust
        // ⚠⚠ **The shared upload.** When this chain is a mask's first sub-stage, the plane it is
        // about to upload is the one that mask's auto-mask gate uploaded moments ago — the same
        // bytes, in the same layout, already resident. Taking it deletes a host memcpy and a
        // device copy of the whole frame: measured at ~2.1 ms and ~6-10 ms respectively at rung
        // 3200. `take` refuses anything that is not this thread's deposit at these dimensions.
        let shared = if self.take_shared {
            ctx.shared_plane().take(image.width(), image.height())
        } else {
            None
        };
        let img = match shared {
            Some(img) => img,
            None => GpuImage::new(ctx, image.width(), image.height())?,
        };
        let bytes = img.bytes();

        let mut enc = device.create_command_encoder(&Default::default());
        let staging = if shared_taken {
            None
        } else {
            let staging = ctx.cache.take_staging(device, bytes)?;
            staging
                .slice(..bytes)
                .get_mapped_range_mut()
                .copy_from_slice(bytemuck::cast_slice(image.data()));
            staging.unmap();
            enc.copy_buffer_to_buffer(&staging, 0, &img.buffer, 0, bytes);
            self.uploads.fetch_add(1, Ordering::Relaxed);
            Some(staging)
        };
```

⚠ `shared` is moved into `img`, so record the flag first:
`let shared = ...; let shared_taken = shared.is_some();`

Then at the end, `ctx.cache.put_staging(staging)` becomes:

```rust
        // ⚠ Only when this render actually took one. A chain that shared its input never asked the
        // pool for a staging buffer and must not hand it one it does not own.
        if let Some(st) = staging {
            ctx.cache.put_staging(st);
        }
```

⚠ Keep the existing doc comment on that line — it records *why* it happens after `poll(Wait)`.
⚠ The `uploads` counter must move **inside** the upload branch. It is the per-chain observable and
a chain that shared its input has genuinely uploaded nothing.

- [ ] **Step 7: mark index 0 of an inner walk**

In `crates/gpu-pipeline/src/accelerate.rs`:

1. `walk` gains a final parameter `share_input: bool`.
2. `accelerate`'s outer call passes `false`; the `inner` closure's call passes `true`.
3. Both push sites gain the marking. Replace the loop's `None` arm body:

```rust
                if let Some(mut c) = flush(&mut run, &mut out, ctx) {
                    mark_shared(&mut c, share_input, out.is_empty());
                    out.push(Box::new(c));
                }
                out.push(s);
```

and the final `flush`'s non-trailing arm:

```rust
        Some(mut c) => {
            mark_shared(&mut c, share_input, out.is_empty());
            out.push(Box::new(c));
            (Pipeline::new(out), None)
        }
```

4. Add beside `flush`:

```rust
/// Let a chain take a mask's deposited base plane, when it is that mask's FIRST sub-stage.
///
/// ⚠⚠ **`out.is_empty()` is what "first" means here, and it is checked at the push rather than
/// inferred afterwards.** A finished `Vec<Box<dyn PipelineStage>>` cannot be asked which of its
/// entries is a chain without downcasting, so the position has to be recorded where it is known.
///
/// ⚠ `share_input` is true only for the recursive walk — a mask's sub-stages — because the
/// property it relies on is `MaskStage::apply`'s: it clones the image and pipes it through its
/// sub-stages in order, so only the first receives the plane the gate was handed. Nothing in the
/// outer tail has that relationship with a coverage rasteriser.
fn mark_shared(c: &mut GpuChain, share_input: bool, is_first: bool) {
    if share_input && is_first {
        c.allow_shared_input();
    }
}
```

- [ ] **Step 8: build, gate, commit**

```bash
cargo fmt --all
```
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```
```bash
cargo test -p gpu-pipeline shared_plane; echo "TEST=$?"
```
Expected: 6 passed.

```bash
git add -A && git commit -F - <<'EOF'
feat(gpu): a mask's sub-stage chain takes the plane its gate already uploaded

MaskStage::apply hands the auto-mask gate `image.data()` and then hands its
first sub-stage a clone of that same image — the same bytes, in the same packed
w*h*3 f32 layout a GpuImage holds. Both were uploaded. Now GpuBrushRaster
uploads the base into a GpuImage and deposits it in a slot on GpuContext, and
the chain at index 0 of a mask's sub-stage list takes it instead of uploading.

Three guards. The chain must be stages[0], marked in `walk` where the position
is known — a chain at index 1 gets whatever index 0 produced. The slot is
cleared at the top of every mask apply, so a frame whose coverage came from the
cache (and therefore deposited nothing) cannot take the previous frame's plane.
And a deposit is taken only at matching dimensions on the depositing thread,
because GpuContext::shared() is process-wide.

SharedPlane is generic over its payload so its bookkeeping rules — one use,
dimensions, ownership, clearing — are testable with no GPU at all.
EOF
```

---

### Task 3: the end-to-end guards

⚠⚠ **Every test here builds its own `GpuContext` rather than using `shared()`.** The slot's
counters are per context, and the shared one is process-wide — reading them there is the
contamination bug `GpuChain`'s per-chain counters were introduced to fix. A second
`GpuContext::new()` in a process that already has one is **259 µs** (measured in M8.3a), so a
private context costs nothing.

**Files:**
- Test: `crates/gpu-pipeline/src/accelerate.rs` (its `#[cfg(test)]` module)

- [ ] **Step 1: add the fixtures**

```rust
    /// A private context, so the shared plane's counters describe this test and nothing else.
    ///
    /// ⚠⚠ **Never `shared()` here.** That one is process-wide and `cargo test` runs in parallel,
    /// so its deposit and take counts would be whatever the rest of the suite happened to do —
    /// exactly the contamination that made `GpuChain`'s transfer counters per chain. A second
    /// `GpuContext::new()` in a process that already has one is 259 us.
    fn private_ctx() -> Option<Arc<GpuContext>> {
        match GpuContext::new() {
            Some(c) => Some(Arc::new(c)),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A brush big enough for the GPU and gating hard enough to upload a base.
    ///
    /// ⚠⚠ **Both properties are load-bearing and neither is obvious.** `GpuBrushRaster` declines
    /// a box below `MIN_GPU_BOX_PX` (512 Ki px), so a small stroke never reaches the GPU at all;
    /// and the base plane is uploaded **only when a stroke is auto-masked**, since a positional
    /// brush needs no pixels to rasterise. A fixture missing either one makes every assertion
    /// below vacuous.
    fn gating_brush_mask(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
        let mut m = mask_carrying(a);
        m.mask = edit_model::Mask::Brush(edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                points: (0..12).map(|i| [0.1 + i as f32 * 0.07, 0.5]).collect(),
                radius: 0.45,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: true,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        });
        m
    }

    /// A scene big enough that the brush's box clears `MIN_GPU_BOX_PX`.
    fn big_scene() -> ImageBuf {
        let (w, h) = (1024, 1024);
        ImageBuf::from_vec(
            w,
            h,
            (0..w * h * 3).map(|i| (i % 977) as f32 / 977.0).collect(),
        )
    }
```

⚠ Check what `ImageBuf` constructor and `Arc`/`GpuContext` imports this test module already has —
`grep -n "^    use \|fn ctx()" crates/gpu-pipeline/src/accelerate.rs | head -20` — and reuse them.

- [ ] **Step 2: the carrier**

```rust
    /// ⚠⚠ **THE test of this milestone.** A shared upload changes no pixel — the same bytes reach
    /// the same buffer by a different route — so every equivalence test in this file would pass
    /// with the whole mechanism deleted. The deposit counter is the only thing that can see it.
    #[test]
    fn a_masked_chain_at_index_zero_takes_the_plane_its_gate_uploaded() {
        let Some(ctx) = private_ctx() else { return };
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        assert_eq!(
            mask_sub_stages(&p),
            vec!["gpu_chain"],
            "fixture check: the mask's hsl must have become a chain at index 0"
        );
        let _ = p.run(big_scene());
        assert_eq!(
            ctx.shared_plane().deposits(),
            1,
            "fixture check: the gate must have uploaded a base plane, or there is nothing to share"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            1,
            "the mask's first sub-stage chain must take the plane instead of uploading it"
        );
    }
```

- [ ] **Step 3: the position guard**

```rust
    /// ⚠⚠ The negative half, and it is the one that matters: a chain behind a CPU stage receives
    /// **that stage's output**, not the plane the gate was handed. Taking it there would render a
    /// picture that is plausible and wrong.
    ///
    /// ⚠ `local_contrast` is the wedge because it is a mask sub-stage that is **not** in
    /// `REPLACEABLE`, so it stays on the CPU and pushes the chain to index 1.
    #[test]
    fn a_chain_behind_a_cpu_stage_does_not_take_the_plane() {
        let Some(ctx) = private_ctx() else { return };
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                local_contrast: edit_model::LocalContrast {
                    clarity: 40.0,
                    ..Default::default()
                },
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let p = accel(
            tail_of(&e),
            &e,
            SCALE,
            ProfileRendering::default(),
            RenderSpec::default(),
            &ctx,
        );
        let subs = mask_sub_stages(&p);
        assert_eq!(
            subs.first(),
            Some(&"local_contrast"),
            "fixture check: a CPU stage must come first, got {subs:?}"
        );
        assert!(
            subs.contains(&"gpu_chain"),
            "fixture check: there must still be a chain behind it, got {subs:?}"
        );
        let _ = p.run(big_scene());
        assert_eq!(
            ctx.shared_plane().deposits(),
            1,
            "fixture check: the gate must still have uploaded a base plane"
        );
        assert_eq!(
            ctx.shared_plane().takes(),
            0,
            "a chain that is not the mask's first sub-stage must upload its own input"
        );
    }
```

⚠ Check the real field name and shape of `LocalAdjustments::local_contrast` before writing this —
`grep -n "local_contrast" crates/edit-model/src/lib.rs | head`. If `local_contrast` turns out to be
replaceable, use whichever mask sub-stage is **not** in `REPLACEABLE`; the table is at the top of
`accelerate.rs`. Print `mask_sub_stages` first if unsure.

- [ ] **Step 4: the staleness guard, and it must fail without `begin_mask`**

```rust
    /// ⚠⚠ **This is the test that fails when `begin_mask` is deleted, and nothing else does.**
    /// Render twice through one pipeline; the second render's coverage comes from the same brush
    /// but its gate deposits a fresh plane every time, so the take count must track the renders
    /// exactly. A slot that were never cleared would still hand out a plane — the previous
    /// render's — and the count would look identical. So the assertion is on the **pixels**: the
    /// second render must equal a cold one.
    #[test]
    fn a_second_render_does_not_take_the_first_renders_plane() {
        let Some(ctx) = private_ctx() else { return };
        let e = Edits {
            masks: vec![gating_brush_mask(edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            })],
            ..Edits::default()
        };
        let build = || {
            accel(
                tail_of(&e),
                &e,
                SCALE,
                ProfileRendering::default(),
                RenderSpec::default(),
                &ctx,
            )
        };
        let p = build();
        let first = p.run(big_scene());
        let second = p.run(big_scene());
        assert!(
            crate::compare(&first, &second).is_exact(),
            "two renders of one scene must agree: {:?}",
            crate::compare(&first, &second)
        );
        assert_eq!(ctx.shared_plane().takes(), 2, "each render shares its own plane");
    }
```

⚠ `crate::compare` returns whatever `equivalence.rs` defines — check its API
(`grep -n "pub fn compare" -A 20 crates/gpu-pipeline/src/equivalence.rs`) and use its real
accessor. If there is no `is_exact`, assert on its worst-level field being 0.

- [ ] **Step 5: run them**

```bash
cargo test --release -p gpu-pipeline shared 2>&1 | tail -20; echo "TEST=$?"
```
```bash
cargo test --release -p gpu-pipeline plane 2>&1 | tail -20
```
Expected: all pass. ⚠ **`--release`** — a debug adapter acquisition is 44–75 s.

- [ ] **Step 6: sabotage every guard, one at a time**

For each, apply the sabotage, run the named test, **confirm a `test result:` line appears** (a
sabotage that fails to compile reads exactly like a pass), then restore from a `cp` backup —
**never `git checkout`**, which this project has twice used to destroy uncommitted work.

```bash
cp crates/gpu-pipeline/src/accelerate.rs /tmp/sab.bak   # etc, per file
```

| sabotage | must fail |
|---|---|
| `mark_shared` ignores `is_first` | `a_chain_behind_a_cpu_stage_does_not_take_the_plane` |
| `mark_shared` ignores `share_input` | *predicted inert* — the outer tail's chain has no mask deposit to take. **Record it as inert rather than counting it.** |
| delete `begin_mask`'s body in `brush_raster.rs` | `a_second_render_does_not_take_the_first_renders_plane` |
| `SharedPlane::take` ignores the dimensions | the `shared_plane` unit test |
| `run_with` takes even when `take_shared` is false | `a_chain_behind_a_cpu_stage_does_not_take_the_plane` |
| delete the `deposit` call | `a_masked_chain_at_index_zero_takes_the_plane...` (on the `deposits` fixture check) |

⚠ If a sabotage listed as caught turns out **inert**, that is a finding about the test — write it
down rather than waving it through, and add the discriminating fixture.

- [ ] **Step 7: gate and commit**

```bash
cargo fmt --all
```
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```
```bash
git add -A && git commit -F - <<'EOF'
test(gpu): guard the shared upload with the deposit counters

A shared upload changes no pixel — the same bytes reach the same buffer by a
different route — so every equivalence test in the workspace passes with the
whole mechanism deleted. The counters on SharedPlane are the only observable,
and each test builds its own GpuContext so they describe that test alone: the
process-wide shared() context would give whatever the rest of the suite did,
which is the contamination that made GpuChain's transfer counters per chain.

Three guards: the chain at a mask's index 0 takes the plane, a chain behind a
CPU stage does not, and a second render does not take the first's. The fixture
brush must be both large enough to clear MIN_GPU_BOX_PX and auto-masked, or the
gate uploads no base and every assertion is vacuous.
EOF
```

---

### Task 4: measure, gate, record

- [ ] **Step 1: measure the real photograph**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
RUSTROOM_DEBUG_MASK=1 cargo run --release -p app --example bench_gpu_tail ~/Pictures/fuji/DSCF6274.RAF 2>&1 | grep -E "MASK |BRUSHGPU|SIDECAR" | tail -24
```

Expect the accelerated render's `sub=` phase to fall from ~24–27 ms toward ~12–15, and the drag
row's GPU column to improve from its 126.4–131.5 ms baseline. ⚠ **Quote a BAND over three runs**,
never one figure — M8.3h had to retract a headline for quoting the top of a band as the value, and
M8.3k's headline was retracted yesterday for a different reason.

Run it three times and record the range.

- [ ] **Step 2: the full gate, each exit code read from its own command**

```bash
cargo fmt --all --check; echo "FMT=$?"
```
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```
```bash
cargo test --workspace 2>&1 | tee /tmp/m83l.log | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print "PASSED="p" FAILED="f}'
```
```bash
grep -c "^test result: FAILED" /tmp/m83l.log
```
```bash
grep -c "golden_" /tmp/m83l.log
```

⚠ Assert the count is **unchanged at 2346 plus exactly the tests this milestone adds** — not
"still green". Confirm **63 goldens ran and none moved**, and **0 GPU skips**.

- [ ] **Step 3: record it in `CLAUDE.md`**

Add an **M8.3l** section above M8.3k's, following the established shape: the headline with its
measured band, then the findings as `⚠`/`⚠⚠` bullets, then **Accepted caveats**. The caveats that
must be stated, because each is a real limit rather than a nicety:

1. **Only an auto-masked brush benefits.** The base is uploaded only when `plan.gating_rows` is
   `Some`; a plain brush deposits nothing and its chain uploads as before.
2. **Only the mask's first sub-stage.** A mask whose sub-stages start with a CPU stage — local
   contrast, saturation, white balance, contrast, the tone curve — pays both uploads.
3. **Nothing for the outer tail**, whose chain has no deposit to take.
4. The gate's own per-pixel Oklab work (~5–7 ms) is untouched and is real work.
5. One machine, one adapter (Apple M2, Metal), one rung, one photographer's single masked brush.
6. `gpu_tail` is still off by default.

Update the **▶ NEXT ACTION** block and the **State at handoff** line with the new test count and
the branch state.

- [ ] **Step 4: launch the GUI for confirmation by use**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Say what to look at: **Settings → Rendering → "Use the GPU"** on, open `DSCF6274.RAF`, drag a
global slider. ⚠⚠ **The picture must not change** — a shared plane that were taken wrongly would
render the *previous* frame's pixels, which looks like a plausible photograph rather than an
error, so the thing to watch for is the mask lagging a frame behind the slider.

- [ ] **Step 5: commit the record**

```bash
git add -A && git commit -F - <<'EOF'
docs: record M8.3l, the mask's shared upload

<the measured band, and the six accepted caveats>
EOF
```
