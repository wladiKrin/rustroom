# M6.5d — a numeric colour readout under the cursor

*Design, 2026-09-03. W4 track D. Settled with the user in W4-0; no decision below is the agent's to
re-open.*

Hover the canvas and the status cluster shows the sRGB value under the pointer. The question it
answers is the one culling and grading both keep asking: *is this actually blown, or does it only
look it?*

---

## 1. Two corrections to the brief's premise, both load-bearing

`docs/PARALLEL-WORK.md` §4 says the rendered choice needs *"the texture's RGBA … on hand in
`canvas/mod.rs`"* and *"no inverse mapping at all"*. The second half is right. The first is not, and
the difference changes the design.

**(1) `Session` holds only an `egui::TextureHandle`, which is GPU-side and cannot be sampled.** That
is why `guide_overlay::draw_loupe` magnifies the texture by *drawing it with UVs* rather than reading
pixels — the only way to get at it. The rendered bytes live on `RenderedFrame.rgba` at
[shell.rs:73](../../../crates/app/src/shell.rs), where they are copied into a `ColorImage` and then
**dropped**.

**(2) `frame.rgba` is NOT sRGB — it is in the display's primaries.** `pack` converts to
`display_space`, which is one of `Srgb`, `DisplayP3`, `AdobeRgb` or `ProPhoto`, and `shell.rs:70`
says so outright: *"those bytes are in the display's primaries."* Reporting them as "sRGB 0–255"
would be a readout that changes when you change monitors.

⚠⚠ **And `pack` has already solved this exact problem one line above, for the histogram.** Its
comment is the whole design of this feature:

> *Quantized to sRGB **first**, and the histogram taken from that. The histogram describes the image;
> the texture describes what this monitor must be driven with. Deriving one from the other is the bug
> this ordering exists to prevent.*

A colour readout is a second instrument of exactly that kind. **It reads `srgb8`, the same bytes the
histogram is bucketed from** — not `rgba`, and not the texture.

⚠ Note the ordering consequence, which is free but must not be undone: `srgb8` is taken **before**
the Visualize Spots view mode replaces it. So the readout reports the *photograph*, not the edge
view — the rule `display_space` and the histogram both already follow. If Visualize Spots is on, the
numbers describe what the photo is, which is the only useful answer.

---

## 2. What is read: the rendered frame

**Every edit applied, as delivered.** Lightroom's choice, and here there is no feedback loop to
worry about — a readout writes nothing, so the trap that forces the white-balance eyedropper and Auto
Tone onto `Session.buffer` (*"analysing it would make the auto chase its own correction"*) simply
does not exist.

The decode buffer was the alternative and is worse for this job: it hands back scene-referred linear
ProPhoto running above 1.0, so showing any display-meaningful number would mean re-implementing the
output transform inside the readout. That is more work than the rendered path *and* it answers a
different question.

⚠ **Consequence: the `sampler_for` extraction the brief assigned to D is DROPPED.** It exists to
compose `Homography::source_of` → `Corrector::source` → a buffer read, which only the decode-buffer
choice needs. Extracting it anyway would touch the retouch tool for a helper with no second caller —
and `crates/app`'s bin modules reject a `pub` item with no production caller under
`clippy -D warnings`, the trap §5 records as having bitten ten times.

---

## 3. Carrying the bytes: one `Arc`, no extra copies

`RenderedFrame` gains the sRGB bytes, and `rgba` becomes an `Arc` so the common case costs nothing:

```rust
pub struct RenderedFrame {
    /// In the display's primaries — what this monitor must be driven with.
    pub rgba: Arc<Vec<u8>>,
    /// The same frame quantized to sRGB, before the display transform and before
    /// Visualize Spots. The histogram is bucketed from these bytes; so is the readout.
    pub srgb: Arc<Vec<u8>>,
    // … width, height, region, hist, max_dim unchanged
}
```

⚠ **In the `Srgb` branch both fields are clones of ONE `Arc`, so nothing is duplicated** — and
`display_space` is `Srgb` whenever the OS is colour-managing, which on macOS is the normal case
(`tag_as_srgb` returns true on this machine). In the wide-gamut branch they are two allocations, one
of which `pack` was already producing and throwing away.

⚠ `shell.rs:75` passes `&frame.rgba` and `ColorImage::from_rgba_unmultiplied` takes a **slice**, so
the texture path needs no ownership and the deref makes that call site compile unchanged.

`Session` keeps `Arc::clone(&frame.srgb)` alongside the frame's width, height and `region`. Retained
memory is ~7 MB at the fit rung and ~27 MB at a native one, for bytes the app already computes.

⚠ **The dimensions must travel with the bytes**, not be re-derived from `s.rendered_region` or the
texture. Two facts that can disagree is the shape `render_max_dim`, `fit_image_size` and
`Session::geometry_mode` were each fixed for; and `RenderedFrame.max_dim` carries a comment saying
exactly that about its own field.

---

## 4. The hover read, at the end of `canvas/mod.rs::show`

Cursor → `img_rect` UV → index into the retained bytes. **No inverse mapping** — that part of the
brief is correct, and it is what makes this the cheap choice.

⚠⚠ **`img_rect`, NOT `frame_rect` — and this is the one overlay where that is true.**
`draw_loupe`'s comment states the rule and the exception together:

> *The sample window is taken in `img_rect` coordinates, not `frame_rect`: `img_rect` is where the
> texture is actually painted, so this stays exact whether the frame was rendered whole or as a
> region. That is the one place in this overlay where `img_rect` is the right rect.*

The readout is the second such place, for the identical reason: it indexes the *rendered bytes*, and
those cover the rendered slice. Every other overlay in this canvas is frame-relative, so this needs
its own comment or the next reader will "fix" it.

⚠ Read only when `resp.hovered()` and the pointer is inside `img_rect`; otherwise the readout is
blank. A stale number left on screen after the pointer leaves is worse than none, because it gets
trusted.

⚠ The read happens **after** the per-tool overlay match, so it never competes for a gesture — it
senses nothing at all, which is what lets it coexist with B's dispatch branch a hundred lines above.

---

## 5. `crates/app/src/color_readout.rs` — the pure half

A new **lib** module (no egui), because formatting a value is a rule a test should reach without a
window.

```rust
/// How wide the sampled patch is, in rendered pixels, either side of the cursor.
pub const PATCH_R: usize = 2;

/// The mean sRGB value of the patch centred on `(x, y)`, or `None` outside the image.
pub fn mean_at(rgba: &[u8], w: usize, h: usize, x: usize, y: usize) -> Option<[u8; 3]>;

/// What the status cluster shows.
pub fn label(rgb: [u8; 3]) -> String;
```

**sRGB 0–255**, which is exactly what the bytes already are — zero conversion, nothing to get subtly
wrong. It also answers the clipping question precisely: **255 means blown and 254 does not**, a
distinction Lightroom's 0–100 % cannot make. Lab and Oklab are the right spaces for judging a
neutral or a skin tone and answer a question a culling feature is not asking; they are a later
addition over a readout that exists.

**A 5×5 mean** (`PATCH_R = 2`), which is the eyedropper's own patch and its recorded reasoning:
*"a single sample is sensor noise, and the whole point of clicking a grey card is to average over
it."* It also stops the numbers jittering as the pointer moves, which is what makes a readout usable
at a native rung.

⚠ **Stated so it is not later found as a bug: a mean reports the neighbourhood, not the extreme.** A
single blown pixel can hide inside a 5×5 average. The clipping question the readout answers is "is
this region blown", and the histogram remains the instrument for "is anything blown".

⚠ The patch **clamps at the image edges**, averaging over the pixels that exist rather than reading
out of bounds — the window shrinks, divided by its actual count. `spatial::box_blur` already
establishes that convention here (*"borders clamp the window, not the samples"*), and it is why
`mean_at` divides by a counted `n` rather than by 25.

---

## 6. Where it shows: the status cluster in `shell.rs`

Its **own label, immediately left of the status/progress label** in the `right_to_left` cluster at
`shell.rs:228`. Blank when the pointer is off the canvas.

⚠ In Develop that cluster currently holds either `export_progress` or `self.status` and nothing
else — the filename is deliberately absent (*"the Develop panel's footer already carries it"*) — so
there is room, and nothing has to yield.

⚠⚠ **The Develop toolbar must not grow a second right-aligned region.** The cluster's own comment
records why it exists: *"the develop toolbar right-aligned the filename itself and then the status
label was added after it — which meant the status was pushed into a row that had no space left."*
Two right-aligned regions in one row is a bug by construction.

The rejected alternatives, so they are not re-proposed: **replacing** the status while hovering saves
width and silently swallows the line where this app reports what it just did; a **floating readout
beside the cursor** occludes the photograph being judged, is not Lightroom's behaviour, and would
push D deeper into `canvas/mod.rs` — the surface it already contends with B on.

---

## 7. Ownership, and what D must not touch

| D owns | D must not touch |
| --- | --- |
| new lib `app/src/color_readout.rs` | `crop_overlay.rs` |
| the `srgb` field and the `Arc` change in `render_engine.rs`'s `RenderedFrame` + `pack` | `guide_overlay.rs` |
| retaining the bytes at `shell.rs`'s frame-landing site | `panel/**` |
| the **hover read at the end of `canvas/mod.rs::show`** | `Session` fields beyond what the hover needs |
| the readout label in `shell.rs`'s right-aligned cluster | `edit-model` / `pipeline` |
| one field group on `Session` in `main.rs` | goldens — **53, byte-identical** |
| | B's dispatch branch in the `Tool::Crop` arm |
| | `spot_overlay::sampler_for` — **the extraction is dropped** (§2) |

⚠ **D merges second: B → D → C.** Revised from §4's D → B → C, because B's armed-toggle decision put
B in `panel/geometry.rs` *and* `Session`, making B the most contended track — and §3's rule is that
the owner of the most contended surface merges first. D then rebases: its hover block and B's
dispatch branch sit in different parts of `show`, and both tracks append a field to `Session`, so
expect one trivial conflict in `main.rs` and keep both additions.

⚠ **Files already over ~600 lines that D will edit** — `canvas/mod.rs` (992), `shell.rs` (930),
`main.rs` — must be **named in D's report**, per §5.

⚠ `render_engine.rs`'s own tests compare `.rgba`; the `Arc` change touches them. That is D's to fix
and no other track reads that file.

---

## 8. Testing

Headless in `color_readout.rs`, with no window and no image fixture:

- **A flat patch reads its own value exactly**, and **a patch straddling two values reads the mean**
  — the second is what discriminates a mean from a single-pixel read. ⚠ A test on a flat field
  cannot tell the two apart: the `spot_source` lesson is that a fixture must contain what a wrong
  implementation would be attracted to.
- **⚠ The edge clamp divides by the pixels that exist.** Sample at `(0, 0)` on a non-uniform image
  and assert the mean of the 3×3 that is actually in bounds — a version dividing by 25 gives a
  plausible, darker number that no flat-field test can see.
- **Out of bounds is `None`**, not a clamped read of the nearest pixel.
- **255 formats as 255**, i.e. the label is not a rescale.

Wiring, in `canvas/mod.rs`'s test module through `test_support`:

- **⚠⚠ The wiring test is the half this project has repeatedly shipped broken.** Hover the canvas
  centre on a photo of known colour and assert the readout **string** on `RustroomApp` is what
  `color_readout::label` gives for that colour. `an_armed_eyedropper_sets_the_white_balance_from_a_click`
  exists for precisely this reason: *"the solve has its own unit tests, but nothing else can see that
  a click on the canvas ever reaches it."*
- **⚠ The harness must produce the hover on the frame that is read.** The headless-canvas work
  records three separate harness faults of this family, one of which made the guard for a whole
  change pass vacuously: *"`drag_delta()` is per-frame and the harness moved the pointer on an
  earlier pass than the one it read."* Move the pointer, then read on the pass where it is there.
- **The readout is blank when the pointer is outside `img_rect`** — the guard on the guard. "A number
  appeared after hovering" means nothing unless it goes away.
- **⚠ It reads `srgb`, not `rgba`.** With `display_space` set to `DisplayP3`, the readout must be
  unchanged from the `Srgb` case on the same photo. Nothing else can see this: a wide-gamut readout
  looks entirely plausible, and it is the same bug `the_histogram_is_unaffected_by_the_display_space`
  was written for one layer over — **copy that test's shape rather than re-deriving it.**
- **The bytes are retained rather than dropped**: sabotage by not storing them and watch the wiring
  test fail.

**The sabotage rule applies to every one of these**, with `ran = passed + failed >= 1` confirmed so a
sabotage that failed to compile cannot read as a pass. ⚠ A sabotage that changes no behaviour is
**not evidence** — record it as inert rather than counting it.

---

## 9. Accepted caveats

1. **A 5×5 mean can hide one blown pixel** (§5). The histogram stays the instrument for that.
2. **The numbers describe the render at its current rung**, so a fit-resolution frame has already
   averaged neighbours that a native-rung frame has not — the same inherent property that makes
   sharpening and noise reduction judgeable only at 1:1.
3. **~7 MB retained at fit, ~27 MB at a native rung**, for bytes the app already computes.
4. **sRGB only** — no Lab, Oklab or percentage readout, and no per-channel clipping indicator.
5. **Develop only.** The Library grid and the Compare and Survey views show no readout; Compare in
   particular renders through `multi::pane`, which is a different path.
6. **Blank until the first frame lands** after opening a photo, since there are no bytes to read yet.

---

## 10. Done when

Hovering the canvas shows the sRGB triple under the pointer in the status cluster; it is blank when
the pointer leaves; it is identical under a wide-gamut display setting; and the report says **what
was run in the GUI**, not only what passed.
