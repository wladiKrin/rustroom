# W4 track D — M6.5d colour readout: Implementation Plan

> **For the track agent:** you are executing this in your **own git worktree**, dispatched as one of
> W4's three parallel tracks. Read `docs/PARALLEL-WORK.md` §2, §3 and §5, then
> `docs/superpowers/specs/2026-09-03-colour-readout-design.md`, before Task 1. Steps use checkbox
> (`- [ ]`) syntax. **Every decision is settled in the spec — re-open none of them.**

**Goal:** Hover the canvas and the status cluster shows the sRGB value under the pointer, so "is this
actually blown, or does it only look it?" has an answer without leaving the canvas.

**Architecture:** `pack` already quantizes to sRGB before the display transform and buckets the
histogram from those bytes; the readout is a second instrument of exactly that kind, so it reads the
same bytes. `RenderedFrame` carries them out on an `Arc`, `Session` keeps them, the canvas indexes
them by `img_rect` UV, and one pure lib function formats the result.

**Tech Stack:** Rust, egui 0.35 (bin modules only).

---

## Build environment — do this first, in every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing conventions, non-negotiable:**
- Commit messages carry **no `Co-Authored-By` trailer**. **Never push.**
- `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all` all
  green before reporting done.
- **Uniquely-named log files only** — `/tmp/<what>-d.log`, never `/tmp/t.log`. W4's three agents
  share `/tmp`; on 2026-09-01 two agents both wrote `/tmp/t.log` and one read the other's run, seeing
  901 passed where its own was 1501.
- **⚠ Read exit codes from a file, never through a pipe.** `${PIPESTATUS[0]}` is bash and this shell
  is **zsh**, so it expands to nothing and a check printing `EXIT: ` reads as a pass:
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-d.log 2>&1; echo "EXIT=$?"
  ```

---

## ⚠⚠ Two corrections to the brief you must internalise before Task 1

§4 of `docs/PARALLEL-WORK.md` says the rendered choice needs *"the texture's RGBA … on hand in
`canvas/mod.rs`"* and *"no inverse mapping at all"*. **The second half is right; the first is not.**

**(1) `Session` holds only an `egui::TextureHandle`, which is GPU-side and cannot be sampled
CPU-side.** That is precisely why `guide_overlay::draw_loupe` magnifies the texture by *drawing it
with UVs* rather than reading pixels. The rendered bytes live on `RenderedFrame.rgba` at
`crates/app/src/shell.rs:73`, where they are copied into a `ColorImage` and then **dropped**.

**(2) `frame.rgba` is NOT sRGB — it is in the display's primaries.** `pack` converts to
`display_space`, one of `Srgb`, `DisplayP3`, `AdobeRgb`, `ProPhoto`, and `shell.rs:70` says so
outright: *"those bytes are in the display's primaries."* Reporting them as "sRGB 0–255" would be a
readout that changes when you change monitors.

**⚠⚠ And `pack` has already solved this exact problem one line above, for the histogram.** Its
comment is the whole design of your feature:

> *Quantized to sRGB **first**, and the histogram taken from that. The histogram describes the image;
> the texture describes what this monitor must be driven with. Deriving one from the other is the bug
> this ordering exists to prevent.*

**So you read `srgb8`** — the same bytes the histogram is bucketed from. Not `rgba`, not the texture.

⚠ Note the free consequence, which must not be undone: `srgb8` is taken **before** the Visualize
Spots view mode replaces it, so the readout reports the *photograph* rather than the grey edge view.
That is the rule `display_space` and the histogram both already follow.

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Create** `crates/app/src/color_readout.rs` | Pure: bytes + a pixel position → the mean and the string shown. No egui. Lib module, so `pub` is public API and `dead_code` cannot fire. |
| **Modify** `crates/app/src/lib.rs` | `pub mod color_readout;` |
| **Modify** `crates/app/src/render_engine.rs` | `RenderedFrame.rgba` becomes `Arc<Vec<u8>>`; a new `srgb: Arc<Vec<u8>>` beside it; `pack` fills both. |
| **Modify** `crates/app/src/main.rs` | One field group at the **end** of `struct Session`, after `visualize` (line ~795). |
| **Modify** `crates/app/src/session_flow.rs`, `crates/app/src/test_app.rs` | The two `Session { .. }` literals. |
| **Modify** `crates/app/src/shell.rs` | Retain the bytes at the frame-landing site (~line 69); the readout label in the right-aligned cluster (~line 228). |
| **Modify** `crates/app/src/develop/canvas/mod.rs` | The hover read at the **end** of `show`. |
| **Modify** `crates/app/src/develop/canvas/test_support.rs` | A `hover_through` helper — there is none. |

⚠ **`canvas/mod.rs` (992 lines), `shell.rs` (930) and `main.rs` are all already over ~600 lines.** §5
requires you to **say so in your report** rather than adding to them silently. You are not asked to
split them.

### ⚠⚠ Surfaces you must NOT touch

`crop_overlay.rs` · `guide_overlay.rs` · `panel/**` · `edit-model` · `pipeline` · the **53 goldens**,
byte-identical · **B's dispatch branch in `canvas/mod.rs`'s `Tool::Crop` arm** ·
**`spot_overlay::sampler_for` — the extraction is DROPPED** (see below).

### ⚠ The `sampler_for` extraction is cancelled

§4 assigned you *"extracting `spot_overlay::sampler_for` into a shared bin helper"*. **Do not.** It
composes `Homography::source_of` → `Corrector::source` → a buffer read, which only the
**decode-buffer** choice needed, and you read the rendered frame. Extracting it anyway would touch
the retouch tool to produce a helper with no second caller — and in a **binary** crate a `pub` item
with no production caller is `dead_code` that `clippy -D warnings` rejects. CLAUDE.md records that
trap as having bitten ten times.

### ⚠ Contention with B

B also appends a field to `struct Session` in `main.rs` and edits the two `Session` literals. **You
merge second (B → D → C), so expect one trivial conflict in `main.rs` / `session_flow.rs` /
`test_app.rs` and keep both additions.** §2 rule 3 is why you append at the **end** and never insert
mid-struct: "git merges appends cleanly and conflicts on mid-struct insertions almost every time."

⚠ `develop/canvas/test_support.rs` is a shared `#[cfg(test)]` module. B does not add a helper to it,
but **check for a conflict there when you rebase.**

---

## Task 1: `color_readout.rs` — the mean and the label, pure

**Files:**
- Create: `crates/app/src/color_readout.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `crates/app/src/lib.rs`, in alphabetical position among the existing `pub mod` lines:

```rust
pub mod color_readout;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/color_readout.rs` with the tests and a stub:

```rust
//! The numeric colour readout — what the status cluster shows for the pixel under the cursor.
//!
//! ⚠ Lives in the lib because this is a unit-testable rule: the `crop_tool` / `mask_tool` /
//! `panel_order` pattern. The hover that produces the position is in the canvas bin module.
//!
//! ⚠⚠ **These bytes are sRGB, and that is load-bearing.** They come from `pack`'s `srgb8`, the
//! same quantization the histogram is bucketed from — **not** from `RenderedFrame.rgba`, which is
//! in the *display's* primaries. `pack`'s own comment states the rule: "the histogram describes
//! the image; the texture describes what this monitor must be driven with."

/// How far either side of the cursor the patch reaches, in rendered pixels.
///
/// ⚠ 2, i.e. a 5×5 patch — the white-balance eyedropper's own radius and its recorded reasoning:
/// "a single sample is sensor noise, and the whole point of clicking a grey card is to average
/// over it." It also stops the numbers jittering as the pointer moves, which is what makes a
/// readout usable at a native rung.
///
/// ⚠ Stated so it is not later found as a bug: **a mean reports the neighbourhood, not the
/// extreme.** A single blown pixel can hide inside a 5×5 average; the histogram remains the
/// instrument for "is *anything* blown".
pub const PATCH_R: usize = 2;

/// The mean sRGB value of the patch centred on `(x, y)` in an RGBA8 buffer of `w` × `h`.
///
/// `None` when the position is outside the image or the buffer does not match its dimensions.
pub fn mean_at(_rgba: &[u8], _w: usize, _h: usize, _x: usize, _y: usize) -> Option<[u8; 3]> {
    unimplemented!()
}

/// What the status cluster shows for a sampled value.
pub fn label(_rgb: [u8; 3]) -> String {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An RGBA8 buffer of one flat colour.
    fn flat(w: usize, h: usize, c: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            v.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        v
    }

    #[test]
    fn a_flat_patch_reads_its_own_value() {
        let b = flat(32, 32, [10, 128, 250]);
        assert_eq!(mean_at(&b, 32, 32, 16, 16), Some([10, 128, 250]));
    }

    /// ⚠⚠ **This is what discriminates a mean from a single-pixel read**, and a flat-field test
    /// cannot: the two agree exactly on a uniform image. The `spot_source` lesson — "each placed
    /// its fixture where a wrong implementation had nothing to reach for."
    ///
    /// A vertical split down the middle of the patch: half 0, half 100, so the mean is 50.
    #[test]
    fn a_patch_straddling_two_values_reads_the_mean() {
        let (w, h) = (32, 32);
        let mut b = flat(w, h, [0, 0, 0]);
        for y in 0..h {
            for x in 16..w {
                let i = (y * w + x) * 4;
                b[i] = 100;
                b[i + 1] = 100;
                b[i + 2] = 100;
            }
        }
        // Centred at x = 16: columns 14..=18, so 2 dark and 3 light — mean 60.
        let m = mean_at(&b, w, h, 16, 16).expect("in bounds");
        assert_eq!(m, [60, 60, 60], "got {m:?}");
    }

    /// ⚠⚠ **The edge clamp divides by the pixels that exist, not by 25.** At a corner only a 3×3
    /// is in bounds; a version dividing by the full patch area gives a plausible, *darker* number
    /// that no flat-field test can see. `spatial::box_blur` establishes the convention here:
    /// "borders clamp the window, not the samples."
    #[test]
    fn the_patch_clamps_at_the_edges_and_divides_by_what_it_read() {
        let b = flat(32, 32, [200, 200, 200]);
        // A corner: 9 of 25 samples are in bounds, all 200. The mean must still be 200.
        assert_eq!(mean_at(&b, 32, 32, 0, 0), Some([200, 200, 200]));
    }

    #[test]
    fn a_position_outside_the_image_reads_nothing() {
        let b = flat(8, 8, [1, 2, 3]);
        assert_eq!(mean_at(&b, 8, 8, 8, 4), None);
        assert_eq!(mean_at(&b, 8, 8, 4, 8), None);
    }

    /// A buffer that does not match its stated dimensions must not index out of bounds.
    #[test]
    fn a_mismatched_buffer_reads_nothing() {
        assert_eq!(mean_at(&[0, 0, 0, 255], 8, 8, 0, 0), None);
        assert_eq!(mean_at(&[], 0, 0, 0, 0), None);
    }

    /// ⚠ The label is not a rescale. 255 must read as 255 — "255 means blown and 254 does not" is
    /// the distinction that makes 0–255 the right units and Lightroom's 0–100 % the wrong ones.
    #[test]
    fn the_label_shows_the_bytes_not_a_percentage() {
        let s = label([255, 0, 128]);
        assert!(s.contains("255"), "{s}");
        assert!(s.contains("128"), "{s}");
        assert!(!s.contains('%'), "the label rescaled to a percentage: {s}");
    }

    /// ⚠ The channels stay in R G B order and are distinguishable in the string, or the readout
    /// cannot answer "which channel is blown".
    #[test]
    fn the_label_keeps_the_channels_in_order() {
        let s = label([11, 22, 33]);
        let r = s.find("11").expect("no red: {s}");
        let g = s.find("22").expect("no green");
        let b = s.find("33").expect("no blue");
        assert!(r < g && g < b, "channels out of order in {s}");
    }
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
cargo test -p app --lib color_readout > /tmp/cr-d.log 2>&1; echo "EXIT=$?"; tail -20 /tmp/cr-d.log
```

Expected: **FAIL**, `not implemented`. ⚠ Confirm a `test result:` line appears — a file that failed
to compile prints none, and this project has recorded that reading exactly like a pass.

- [ ] **Step 4: Implement**

```rust
/// The mean sRGB value of the patch centred on `(x, y)` in an RGBA8 buffer of `w` × `h`.
///
/// `None` when the position is outside the image or the buffer does not match its dimensions.
///
/// ⚠ The dimension check is not defensive noise: the bytes and the dimensions travel together
/// from `RenderedFrame`, and a mismatch would index past the end of the buffer.
pub fn mean_at(rgba: &[u8], w: usize, h: usize, x: usize, y: usize) -> Option<[u8; 3]> {
    if w == 0 || h == 0 || rgba.len() != w * h * 4 || x >= w || y >= h {
        return None;
    }
    let (mut acc, mut n) = ([0u32; 3], 0u32);
    let y0 = y.saturating_sub(PATCH_R);
    let y1 = (y + PATCH_R).min(h - 1);
    let x0 = x.saturating_sub(PATCH_R);
    let x1 = (x + PATCH_R).min(w - 1);
    for py in y0..=y1 {
        for px in x0..=x1 {
            let i = (py * w + px) * 4;
            acc[0] += rgba[i] as u32;
            acc[1] += rgba[i + 1] as u32;
            acc[2] += rgba[i + 2] as u32;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    // ⚠ Divided by what was actually read, never by the full patch area: the window clamps at
    // the borders, so at a corner only 9 of 25 samples exist. `spatial::box_blur`'s convention.
    Some([
        (acc[0] / n) as u8,
        (acc[1] / n) as u8,
        (acc[2] / n) as u8,
    ])
}

/// What the status cluster shows for a sampled value.
///
/// ⚠ sRGB 0–255, which is exactly what the bytes already are — zero conversion, nothing to get
/// subtly wrong. It also answers the clipping question precisely: 255 means blown and 254 does
/// not, a distinction Lightroom's 0–100 % cannot make.
pub fn label(rgb: [u8; 3]) -> String {
    format!("R {}  G {}  B {}", rgb[0], rgb[1], rgb[2])
}
```

- [ ] **Step 5: Verify they pass**

```bash
cargo test -p app --lib color_readout > /tmp/cr-d.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/cr-d.log
```

Expected: **PASS**, 7 passed.

- [ ] **Step 6: Sabotage each and watch it fail**

⚠ **Not a formality** — sixteen recorded instances of a test that could not detect the bug it named.

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Read one pixel: return `[rgba[i], rgba[i+1], rgba[i+2]]` at `(x, y)` | `a_patch_straddling_two_values_reads_the_mean` |
| 2 | Divide by `(2*PATCH_R+1).pow(2)` instead of `n` | `the_patch_clamps_at_the_edges_and_divides_by_what_it_read` |
| 3 | Drop the `rgba.len() != w * h * 4` check | `a_mismatched_buffer_reads_nothing` (or a panic — either is a catch) |
| 4 | Drop `x >= w \|\| y >= h` | `a_position_outside_the_image_reads_nothing` |
| 5 | `format!("{}%", rgb[0] as f32 / 2.55)` in `label` | `the_label_shows_the_bytes_not_a_percentage` |
| 6 | Swap R and B in `label` | `the_label_keeps_the_channels_in_order` |

Confirm a `test result:` line for each (so you know it compiled), confirm the **named** test failed,
revert. ⚠ A sabotage that does not fail is a finding about the test; a sabotage that does not compile
says nothing at all.

- [ ] **Step 7: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-d.log 2>&1; echo "EXIT=$?"
git add crates/app/src/color_readout.rs crates/app/src/lib.rs
git commit -m "feat(app): the numeric colour readout's mean and label"
```

---

## Task 2: carry the sRGB bytes out of `pack`

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Change `RenderedFrame`**

In `crates/app/src/render_engine.rs`, `pub struct RenderedFrame` (line ~12), change `rgba` and add
`srgb`:

```rust
    /// The frame in the **display's** primaries — what this monitor must be driven with.
    ///
    /// ⚠ An `Arc` so the sRGB case below costs nothing: when the display space *is* sRGB, `rgba`
    /// and `srgb` are two handles on one allocation. `ColorImage::from_rgba_unmultiplied` takes a
    /// slice, so the texture path never needs ownership.
    pub rgba: std::sync::Arc<Vec<u8>>,
    /// The same frame quantized to **sRGB**, before the display transform and before the
    /// Visualize Spots view mode.
    ///
    /// ⚠⚠ **This is the instrument channel, and `rgba` is not.** `hist` is already bucketed from
    /// these bytes for the reason `pack` records: "the histogram describes the image; the texture
    /// describes what this monitor must be driven with. Deriving one from the other is the bug
    /// this ordering exists to prevent." A colour readout is a second instrument of exactly that
    /// kind — reading `rgba` would give numbers that change when you change monitors.
    ///
    /// ⚠ Taken **before** Visualize Spots replaces the image, so the readout reports the
    /// photograph rather than the grey edge view.
    pub srgb: std::sync::Arc<Vec<u8>>,
```

- [ ] **Step 2: Fill both in `pack`**

In `pack` (line ~276), replace the tail — from `let rgba = if display_space == …` to the
`RenderedFrame { .. }` literal — with:

```rust
    // ⚠ Wrapped before the branch so the sRGB case shares one allocation with `rgba` rather than
    // cloning ~7 MB (fit) or ~27 MB (a native rung) on every rendered frame.
    let srgb = std::sync::Arc::new(srgb8);

    let rgba = if display_space == color::OutputSpace::Srgb {
        std::sync::Arc::clone(&srgb)
    } else {
        // ⚠ Converted from the **float** image, not from `srgb8`. An 8-bit → 8-bit conversion
        // compresses the range and costs roughly 0.8 bits, which shows as banding on a smooth
        // sky. The extra quantization pass is paid only when a display space is actually set.
        std::sync::Arc::new(
            out.map_pixels(move |c| {
                let lin = c.map(color::srgb_to_linear);
                let conv = display_space.from_srgb(lin);
                [
                    display_space.encode(conv[0]),
                    display_space.encode(conv[1]),
                    display_space.encode(conv[2]),
                ]
            })
            .to_rgba8(),
        )
    };

    RenderedFrame {
        width,
        height,
        rgba,
        srgb,
        hist,
        region,
        max_dim,
    }
}
```

⚠ **`srgb8` is moved into the `Arc` before the Visualize Spots block rebinds it — check the order.**
In the current `pack`, `srgb8` is rebound by the `match visualize` block. Wrap the **pre-visualize**
value: hoist `let srgb = Arc::new(srgb8.clone());` to immediately after
`let hist = crate::histogram::histogram_rgba(&srgb8);`, **or** restructure so the visualize branch
rebinds a different name. Read the function and pick the shape that keeps *one* clone at most in the
Visualize case and *zero* in the ordinary one. **Say in your report which you chose.**

- [ ] **Step 3: Let the compiler find every reader**

```bash
cargo build -p app > /tmp/build-d.log 2>&1; echo "EXIT=$?"; grep -B2 -A6 "^error" /tmp/build-d.log | head -60
```

Fix each site. Known ones: `shell.rs:75` passes `&frame.rgba`, which still coerces through the
`Arc`'s deref and should need no change; `render_engine.rs`'s own tests at ~872, ~1133 and ~1142
compare `.rgba` and may need a `*` or `.as_slice()`. ⚠ **Take the list from the compiler, not from
this plan** — that is what keeps it correct if the file has moved.

- [ ] **Step 4: Add the test that pins the invariant**

In `render_engine.rs`'s test module:

```rust
    /// ⚠⚠ **`srgb` must be sRGB whatever the display space, and `rgba` must not.** Nothing else
    /// can see this: a readout built on `rgba` looks entirely plausible and is simply wrong on a
    /// wide-gamut monitor. This is the sibling of
    /// `the_histogram_is_unaffected_by_the_display_space`, one layer over.
    #[test]
    fn the_srgb_channel_is_unaffected_by_the_display_space() {
        // Render the same edits twice, once for each display space.
        let a = render_for_test(color::OutputSpace::Srgb);
        let b = render_for_test(color::OutputSpace::DisplayP3);
        assert_eq!(*a.srgb, *b.srgb, "the sRGB channel moved with the display space");
        assert_ne!(
            *a.rgba, *b.rgba,
            "fixture check: the display transform did nothing, so this test proves nothing"
        );
        // And the histogram already had this property; assert both together so the two cannot
        // drift apart.
        assert_eq!(a.hist, b.hist);
    }

    /// ⚠ In the sRGB case the two must be one allocation, or every rendered frame pays a
    /// multi-megabyte clone. Asserted structurally rather than by measuring.
    #[test]
    fn the_srgb_case_shares_one_allocation() {
        let f = render_for_test(color::OutputSpace::Srgb);
        assert!(
            std::sync::Arc::ptr_eq(&f.rgba, &f.srgb),
            "sRGB rendered two buffers where one would do"
        );
    }
```

⚠ `render_for_test(space)` is a helper you write, modelled on whatever
`the_histogram_is_unaffected_by_the_display_space` already does — **read that test and reuse its
fixture rather than inventing a second one.** If it does not exist under that exact name, find the
test that covers the display-space/histogram relationship and follow it.

- [ ] **Step 5: Run, sabotage, commit**

```bash
cargo test -p app > /tmp/test-d.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/test-d.log
```

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | `srgb: Arc::clone(&rgba)` in the `RenderedFrame` literal | `the_srgb_channel_is_unaffected_by_the_display_space` |
| 2 | Take `srgb` **after** the Visualize Spots block | add an assertion for it, or state that no test covers it and cover it in the GUI run |
| 3 | Clone unconditionally in the sRGB branch | `the_srgb_case_shares_one_allocation` |

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-d.log 2>&1; echo "EXIT=$?"
git add crates/app/src/render_engine.rs
git commit -m "feat(app): carry the frame's sRGB bytes beside its display-space ones"
```

---

## Task 3: retain the bytes, read them on hover, show them

⚠ **One commit.** The `Session` field, the code that fills it and the code that reads it must land
together: in a **binary** crate a field written but never read is `dead_code` and
`clippy -D warnings` rejects it. CLAUDE.md records that trap as having bitten ten times.

**Files:**
- Modify: `crates/app/src/main.rs`, `session_flow.rs`, `test_app.rs`
- Modify: `crates/app/src/shell.rs`
- Modify: `crates/app/src/develop/canvas/mod.rs`

- [ ] **Step 1: Add the fields to `Session`**

In `crates/app/src/main.rs`, **after** `visualize: Option<f32>,` — currently the last field, around
line 795:

```rust
    /// The rendered frame's sRGB bytes, kept for the colour readout, with the dimensions and
    /// region they describe.
    ///
    /// ⚠⚠ **Not the texture.** `texture` is an `egui::TextureHandle`, i.e. GPU-side, and cannot be
    /// sampled — which is exactly why `guide_overlay::draw_loupe` magnifies it by *drawing* it
    /// with UVs. These bytes are `pack`'s `srgb8`, the same ones `hist` is bucketed from, and
    /// deliberately **not** `RenderedFrame.rgba`, which is in the display's primaries and would
    /// give a readout that changes with the monitor.
    ///
    /// ⚠ The dimensions travel **with** the bytes rather than being re-derived from
    /// `rendered_region` or the texture. Two facts that can disagree is the shape
    /// `render_max_dim`, `fit_image_size` and `geometry_mode` were each fixed for.
    ///
    /// ⚠ An `Arc`, so retaining it costs nothing beyond the memory the frame already held: ~7 MB
    /// at the fit rung, ~27 MB at a native one.
    readout_bytes: Option<(std::sync::Arc<Vec<u8>>, usize, usize)>,
    /// What the status cluster shows for the pixel under the cursor, or empty.
    ///
    /// ⚠ Cleared whenever the pointer is not over the image. A stale number left on screen after
    /// the pointer leaves is worse than none, because it gets trusted.
    readout: String,
```

⚠ **Append at the end, never insert mid-struct** — §2 rule 3, and **B is appending here too.**

- [ ] **Step 2: Fix the two `Session` literals the compiler names**

```bash
cargo build -p app > /tmp/build-d.log 2>&1; echo "EXIT=$?"; grep -A4 "missing field" /tmp/build-d.log
```

Add `readout_bytes: None,` and `readout: String::new(),` to each. There are two:
`session_flow.rs:356` and `test_app.rs:189`. ⚠ Take the list from the compiler.

- [ ] **Step 3: Retain the bytes at the frame-landing site**

In `crates/app/src/shell.rs`, inside `if let Some(frame) = s.engine.poll_latest() {` — immediately
after the `s.hist = frame.hist;` line (~70):

```rust
                // ⚠⚠ Kept rather than dropped, and it is the **sRGB** channel, not `rgba`. See
                // `RenderedFrame::srgb`: `rgba` is in the display's primaries, so a readout built
                // on it would report different numbers on a different monitor. The dimensions
                // travel with the bytes so nothing has to re-derive them.
                s.readout_bytes = Some((
                    std::sync::Arc::clone(&frame.srgb),
                    frame.width,
                    frame.height,
                ));
```

- [ ] **Step 4: Read on hover, at the end of `canvas::show`**

In `crates/app/src/develop/canvas/mod.rs`, inside the `if let Some(tex) = &s.texture {` block, at the
**very end** — after the zoom/pan block and after the composition-guides block, i.e. as the last
statement before the closing brace of that `if let`:

```rust
                    // ⚠⚠ **`img_rect`, NOT `frame_rect` — and this is the one place in this
                    // canvas where that is true.** `draw_loupe` records the rule and the exception
                    // together: "`img_rect` is where the texture is actually painted, so this
                    // stays exact whether the frame was rendered whole or as a region." The
                    // readout indexes the *rendered bytes*, and those cover the rendered slice.
                    // Every other overlay here is frame-relative, so this needs its own comment or
                    // the next reader will "fix" it.
                    //
                    // ⚠ No inverse mapping anywhere: cursor → UV → index.
                    //
                    // ⚠ Placed after the per-tool match and senses nothing at all, so it never
                    // competes for a gesture.
                    s.readout.clear();
                    if let (Some(p), Some((bytes, bw, bh))) =
                        (ui.input(|i| i.pointer.hover_pos()), s.readout_bytes.as_ref())
                    {
                        if img_rect.contains(p) && img_rect.width() > 0.0 && img_rect.height() > 0.0
                        {
                            let u = (p.x - img_rect.left()) / img_rect.width();
                            let v = (p.y - img_rect.top()) / img_rect.height();
                            let x = ((u * *bw as f32) as usize).min(bw.saturating_sub(1));
                            let y = ((v * *bh as f32) as usize).min(bh.saturating_sub(1));
                            if let Some(m) =
                                rustroom_app::color_readout::mean_at(bytes, *bw, *bh, x, y)
                            {
                                s.readout = rustroom_app::color_readout::label(m);
                            }
                        }
                    }
```

⚠ If the borrow checker objects to reading `s.readout_bytes` while writing `s.readout`, split it:
compute into a local `let text = …;` first, then `s.readout = text;`. **Do not** reach for a clone of
the bytes.

- [ ] **Step 5: Show it in the status cluster**

In `crates/app/src/shell.rs`, inside the `ui.with_layout(egui::Layout::right_to_left(…), |ui| { … })`
block (~line 228), **after** the export-progress / status `if` chain — which puts it to the **left**
of them in a right-to-left layout:

```rust
                    // ⚠ Its own label, immediately left of the status/progress label, so neither
                    // is ever hidden. ⚠⚠ It must NOT become a second right-aligned region: this
                    // cluster's own comment records that "the develop toolbar right-aligned the
                    // filename itself and then the status label was added after it — which meant
                    // the status was pushed into a row that had no space left." One cluster, laid
                    // out rightmost first.
                    //
                    // ⚠ Blank when the pointer is off the image, which `canvas::show` clears each
                    // frame — a stale value gets trusted.
                    if let Some(s) = &self.session {
                        if self.view == AppView::Develop && !s.readout.is_empty() {
                            ui.label(&s.readout);
                        }
                    }
```

⚠ `self.session` may already be mutably borrowed in that closure — if so, copy the string into a
local **before** the `with_layout` call and use that. Read the surrounding code first.

- [ ] **Step 6: Add a `hover_through` helper**

`test_support.rs` has `drag_through`, `drag_through_with` and `click_through` — **there is no hover
helper**. Add one, following their five-pass shape:

```rust
/// Drive `show` with the pointer moved to `at` and no button pressed — an ordinary hover.
///
/// ⚠⚠ **The pointer must be at `at` on the pass that is read.** `Response` and pointer state are
/// per-frame, and the headless-canvas work records three separate harness faults of this family —
/// one of which made the regression guard for a whole change "pass vacuously because
/// `drag_delta()` is per-frame and the harness moved the pointer on an earlier pass than the one
/// it read." Two passes: move, then read on the next one with the pointer still there.
pub(super) fn hover_through(app: &mut RustroomApp, ctx: &egui::Context, at: egui::Pos2) {
    for _ in 0..2 {
        let c = ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen()),
                events: vec![egui::Event::PointerMoved(at)],
                ..Default::default()
            },
            |ui| {
                super::show(app, ui, &c);
            },
        );
    }
}

/// Drive `show` with the pointer moved off the image entirely.
pub(super) fn hover_away(app: &mut RustroomApp, ctx: &egui::Context) {
    hover_through(app, ctx, egui::pos2(-100.0, -100.0));
}
```

- [ ] **Step 7: Write the wiring tests**

⚠⚠ **The wiring half is what this project has repeatedly shipped broken.**
`an_armed_eyedropper_sets_the_white_balance_from_a_click` exists because "the solve has its own unit
tests, but nothing else can see that a click on the canvas ever reaches it."

In `crates/app/src/develop/canvas/mod.rs`'s `mod tests`:

```rust
    /// ⚠⚠ The wiring test: a hover over the image must reach `color_readout` and land in the
    /// string the status cluster shows.
    #[test]
    fn hovering_the_canvas_produces_a_readout() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        // ⚠ The bytes only exist once a frame has landed, which happens in `shell.rs` — not in
        // this harness. Seed them the way the frame-landing site does, so the test covers the
        // canvas read rather than the render.
        {
            let s = app.session.as_mut().expect("a session");
            let (w, h) = (8usize, 8usize);
            let mut bytes = Vec::new();
            for _ in 0..w * h {
                bytes.extend_from_slice(&[10, 128, 250, 255]);
            }
            s.readout_bytes = Some((std::sync::Arc::new(bytes), w, h));
        }
        super::test_support::hover_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        assert!(
            s.readout.contains("128"),
            "the hover produced no readout: {:?}",
            s.readout
        );
    }

    /// ⚠ The guard on the guard. "A number appeared after hovering" means nothing unless it goes
    /// away — a stale value left on screen is worse than none, because it gets trusted.
    #[test]
    fn hovering_off_the_image_clears_the_readout() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().expect("a session");
            let (w, h) = (8usize, 8usize);
            let mut bytes = Vec::new();
            for _ in 0..w * h {
                bytes.extend_from_slice(&[10, 128, 250, 255]);
            }
            s.readout_bytes = Some((std::sync::Arc::new(bytes), w, h));
        }
        super::test_support::hover_through(&mut app, &ctx, centre);
        assert!(
            !app.session.as_ref().expect("a session").readout.is_empty(),
            "fixture check: nothing was read, so this test proves nothing"
        );
        super::test_support::hover_away(&mut app, &ctx);
        assert!(
            app.session.as_ref().expect("a session").readout.is_empty(),
            "the readout survived the pointer leaving: {:?}",
            app.session.as_ref().expect("a session").readout
        );
    }

    /// ⚠ A hover reads and **writes nothing else**: no render request, no history entry, no edit.
    /// A readout that dirtied the photo would be found much later, by use.
    #[test]
    fn hovering_asks_for_no_render_and_pushes_no_history() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        let before = {
            let s = app.session.as_ref().expect("a session");
            (s.engine.requests_issued(), s.history.can_undo(), s.edits.clone())
        };
        super::test_support::hover_through(&mut app, &ctx, centre);
        let s = app.session.as_ref().expect("a session");
        assert_eq!(s.engine.requests_issued(), before.0, "the hover requested a render");
        assert_eq!(s.history.can_undo(), before.1, "the hover pushed history");
        assert_eq!(s.edits, before.2, "the hover changed the edits");
    }
```

- [ ] **Step 8: Run and sabotage**

```bash
cargo test -p app > /tmp/test-d.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/test-d.log
```

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Delete the hover-read block from `show` | `hovering_the_canvas_produces_a_readout` |
| 2 | Remove `s.readout.clear();` | `hovering_off_the_image_clears_the_readout` |
| 3 | Use `frame_rect` instead of `img_rect` for the UV | ⚠ **may be inert** in the harness, where no region render is active and the two rects coincide. **If it does not fail, say so** and verify it in the GUI at 1:1 zoom instead. An inert sabotage is not evidence. |
| 4 | Drop the `img_rect.contains(p)` guard | `hovering_off_the_image_clears_the_readout` |
| 5 | Read `frame.rgba` instead of `frame.srgb` in `shell.rs` | ⚠ **no canvas test can see this** — it is covered by `the_srgb_channel_is_unaffected_by_the_display_space` in Task 2 plus the GUI check in Task 4. Say so. |
| 6 | Delete the `ui.label(&s.readout)` in the cluster | ⚠ **likely uncovered** — the canvas tests assert the *string*, not that it is drawn. Use `test_ui`'s label-finding if it can see a plain `ui.label`; CLAUDE.md records that **plain `ui.label` text produces no AccessKit node**, so it probably cannot. **If so, state the gap and cover it in the GUI run.** Do not invent a test that appears to cover it. |

- [ ] **Step 9: Full suite, goldens, commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-d.log 2>&1; echo "EXIT=$?"
cargo test --all > /tmp/test-d.log 2>&1; echo "EXIT=$?"; grep "test result" /tmp/test-d.log | tail -5
git status --short -- crates/pipeline/tests
git add -A
git commit -m "feat(develop): a numeric colour readout under the cursor"
```

Expected: the test count is **1815 plus your new ones**. ⚠ Assert the count, not "still green" —
W0's rule: a change that alters the test count moved behaviour.

---

## Task 4: verify, run the GUI, report

- [ ] **Step 1: The three gates, exit codes read from files**

```bash
cargo fmt --check > /tmp/fmt-d.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy-d.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/test-d.log 2>&1; echo "TEST=$?"; grep "test result" /tmp/test-d.log
```

All three must print `=0`. ⚠ `cargo test` being green is **not** evidence about clippy — seven
recorded instances.

- [ ] **Step 2: Goldens byte-identical, and `sampler_for` untouched**

```bash
git diff --stat $(git merge-base main HEAD) -- crates/pipeline/tests
git diff --stat $(git merge-base main HEAD) -- crates/app/src/develop/canvas/spot_overlay.rs
```

Both expected **empty**. ⚠ Use `git merge-base main HEAD`, not `git diff main` — a plain diff shows
B's merged changes and reads as a violation. §3 records this trap, and **B merges before you**, so it
will bite.

- [ ] **Step 3: Measure the memory you retained**

State it rather than estimating: the bytes are `width * height * 4`. Print them once from the
frame-landing site behind an env gate, the way `RUSTROOM_DEBUG_DISPLAY` already works in `pack`:

```bash
RUSTROOM_DEBUG_DISPLAY=1 cargo run --release -p app 2>&1 | head -20
```

Report the fit-rung and native-rung figures you actually saw.

- [ ] **Step 4: ⚠⚠ RUN IT IN THE GUI**

```bash
cargo run --release -p app
```

Every defect in this project's canvas record was found by use and none by a test before it was
reported. Do all of this and write down what you saw:

1. Open a photo. In **Develop**, hover the canvas. The readout appears in the top-right cluster,
   left of the status line.
2. Move the pointer over a **blown highlight** — a sky, a specular. It should read **255**, or very
   near it, and 254 vs 255 should be distinguishable as you move.
3. Move the pointer over deep shadow. Low numbers, no jitter as you move slowly.
4. Move the pointer **off the image** onto the grey surround. The readout must **disappear**.
5. **Zoom to 1:1** and pan. ⚠ **This is the `img_rect`-vs-`frame_rect` check that no test could
   make**: the numbers must describe the pixel actually under the cursor, not one offset by the pan.
   Hover a distinctive feature, pan, hover it again — the numbers must follow the feature.
6. Trigger an **export** so `export_progress` fills, and hover. Both labels must be visible; neither
   is pushed off the row.
7. Switch to **Library** and back. No readout in Library.
8. ⚠ **Settings → display space → Display P3**, then hover the same pixel as in step 2. **The numbers
   must be unchanged from sRGB.** This is the check for the feature's central decision, and it is
   what a readout built on `frame.rgba` would fail. Set it back afterwards.
9. Turn **Visualize Spots** on in the Retouch panel and hover. The readout must still describe the
   **photograph**, not the grey edge view.
10. Hover while dragging a slider. Nothing should stutter — the readout does no work per frame beyond
    a 25-pixel mean.

- [ ] **Step 5: Report**

Your report must say:

- **What you ran in the GUI** and what you saw for each of the ten checks — not only what passed in
  the suite. ⚠ **Checks 5, 8, 9 and the cluster layout are the four that no test covers**; they are
  the point of this step.
- The **test count before and after** (1815 → your number), not "still green".
- **That the 53 goldens are byte-identical** and `spot_overlay.rs` is untouched.
- **The retained memory** you measured at the fit and native rungs.
- **Which shape you chose in Task 2 Step 2** for taking `srgb` before the Visualize Spots rebind,
  and how many clones it costs in each case.
- **The files you touched that are already over ~600 lines** — `canvas/mod.rs` (992), `shell.rs`
  (930), `main.rs` — per §5.
- **Any sabotage that did not fail, and any that was inert.** Sabotages 3, 5 and 6 in Task 3 are all
  expected to be uncoverable by a canvas test; **name the gaps** rather than inventing tests that
  appear to close them. CLAUDE.md's precedent: *"no test can distinguish the two, and inventing one
  that appeared to would be worse than saying so."*
- **Anything this plan did not settle.** ⚠ Report it, do not silently pick — §6 exists because "an
  agent given an open design choice does not stall; it picks one silently and the choice ships."
- ⚠ If the instructions genuinely conflict — if `cargo test --all` cannot be green without touching a
  barred file — **choose the green gate and say so.** W3 has two recorded precedents of exactly that
  being right, both flagged rather than hidden.

**Do not push.** The orchestrator merges **B → D → C**; you are second, so expect a trivial conflict
with B in `main.rs`, `session_flow.rs` and `test_app.rs` — both of you append a `Session` field, and
**both additions are kept**.
