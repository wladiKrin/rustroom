# A display profile for the preview — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the preview oversaturating on a wide-gamut display by converting sRGB output into the display's primaries on the way to the GPU.

**Architecture:** A **post-step, never a pipeline stage.** The develop pipeline keeps rendering sRGB, so the histogram stays image-referred and no golden can move. One conversion is applied in `render_engine::pack` (in float, before quantizing) and to thumbnails at texture upload.

**Tech Stack:** Rust, `color`'s existing fused-matrix style, egui/eframe.

**Spec:** `docs/superpowers/specs/2026-08-24-display-profile-design.md`

**Before anything:** every build command needs

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Branch first**, per this project's standing convention. Never push; the user pushes.

```bash
git checkout -b display-profile
```

---

## File Structure

| file | responsibility |
|---|---|
| `crates/color/src/spaces.rs` | *modify* — add `SRGB_TO_DISPLAY_P3` and `SRGB_TO_ADOBE_RGB` |
| `crates/color/src/output_space.rs` | *modify* — `from_srgb`, `DISPLAYS`, and an 8-bit helper |
| `crates/app/src/render_engine.rs` | *modify* — `pack`, `Request` and `render_once` take a space |
| `crates/app/src/settings.rs` | *modify* — `Settings.display_space` |
| `crates/app/src/main.rs` | *modify* — `Session.display_space`; `Session::request` supplies it; `apply_display_space` |
| `crates/app/src/modals/settings.rs` | *modify* — the dropdown |
| `crates/app/src/shell.rs` | *modify* — convert thumbnails at texture upload |

### Three facts established by reading the code — do not re-derive them

- **⚠ `Session::request` (`main.rs:382`) is the single choke point** for all seven `s.request(..)`
  call sites. Putting the space on `Session` means **none of those seven lines change**.
- **⚠ The `baseline` texture is never drawn.** `show_before` is a normal render now; the baseline
  survives only for its dimensions (`develop/panel/geometry.rs:52`). So its colour does not matter
  and `DecodeWorker` needs no plumbing at all.
- **⚠ `render_once` must still take the space** even though its only production caller ignores
  colour, because `every_cached_frame_matches_a_cold_render` compares it against `render_cached`.
  If the two used different spaces that test would fail for the wrong reason.

---

### Task 1: `color` learns sRGB → display

**Files:**
- Modify: `crates/color/src/spaces.rs`
- Modify: `crates/color/src/output_space.rs`

- [ ] **Step 1: Add the two matrices to `spaces.rs`**

Append after `PROPHOTO_TO_ADOBE_RGB`:

```rust
/// Linear sRGB → linear Display P3. Both are D65, so no chromatic adaptation is involved.
///
/// ⚠ Every entry is non-negative and no in-range input can produce a negative output, because
/// sRGB's gamut sits strictly *inside* P3 — the opposite of [`PROPHOTO_TO_SRGB`], which must
/// produce negatives. Do not copy a clamping decision between the two.
pub const SRGB_TO_DISPLAY_P3: [[f32; 3]; 3] = [
    [0.8224619687, 0.1775380313, 0.0000000000],
    [0.0331941989, 0.9668058011, 0.0000000000],
    [0.0170826307, 0.0723974407, 0.9105199286],
];

/// Linear sRGB → linear Adobe RGB (1998). Both are D65.
///
/// ⚠⚠ **The middle row really is `[0, 1, 0]`.** It looks like a copy-paste error and is not:
/// Adobe RGB shares sRGB's red and blue primaries and differs only in green, and the green
/// *channel value* comes through unchanged. Cross-checked independently — sRGB green expressed
/// here is `(0.2849, 1.0, 0.0412)`, whose luminance through Adobe's own Y coefficients
/// (0.2974 / 0.6273 / 0.0753) is 0.7151, matching sRGB green's known 0.7152.
pub const SRGB_TO_ADOBE_RGB: [[f32; 3]; 3] = [
    [0.7151256069, 0.2848743931, 0.0000000000],
    [0.0000000000, 1.0000000000, 0.0000000000],
    [0.0000000000, 0.0411619485, 0.9588380515],
];
```

- [ ] **Step 2: Write the failing tests**

Append these to the `tests` module in `crates/color/src/output_space.rs` (inside the existing
closing brace — put them before it):

```rust
    #[test]
    fn srgb_is_bit_exactly_the_identity_for_the_display_transform() {
        // ⚠ The default must cost nothing and change nothing. `assert_eq!`, not a tolerance:
        // routing sRGB through a general matrix path would leave it a ulp off, and a preview
        // that shifts by a ulp when nobody asked is a regression nobody would find.
        for c in [[0.18, 0.5, 0.9], [0.0, 1.0, 0.0], [1.0, 1.0, 1.0]] {
            assert_eq!(OutputSpace::Srgb.from_srgb(c), c);
        }
    }

    #[test]
    fn the_display_transform_maps_white_to_white() {
        for s in OutputSpace::DISPLAYS {
            let w = s.from_srgb([1.0, 1.0, 1.0]);
            for (i, v) in w.iter().enumerate() {
                assert!(
                    (v - 1.0).abs() < 1e-4,
                    "{}: white channel {i} became {v}",
                    s.label()
                );
            }
        }
    }

    #[test]
    fn srgb_fits_inside_every_display_space() {
        // ⚠ sRGB's gamut is strictly inside both destinations, so no in-range colour can leave
        // [0, 1]. If this ever fails a matrix is wrong — it is not a case to start clamping.
        for s in OutputSpace::DISPLAYS {
            for c in [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 1.0, 0.0],
                [0.2, 0.7, 0.4],
            ] {
                let out = s.from_srgb(c);
                for (i, v) in out.iter().enumerate() {
                    assert!(
                        (-1e-6..=1.0 + 1e-6).contains(v),
                        "{}: {c:?} channel {i} left the range: {v}",
                        s.label()
                    );
                }
            }
        }
    }

    #[test]
    fn a_wider_container_needs_smaller_coordinates() {
        // ⚠ The direction test, and the reason it is not merely "the value changed": an
        // *inverted* matrix would also change the value, and would oversaturate exactly as the
        // uncorrected preview does — i.e. it would look like no fix at all while passing.
        // Red is the channel where sRGB and P3 differ most.
        let red = OutputSpace::DisplayP3.from_srgb([1.0, 0.0, 0.0]);
        assert!(
            red[0] < 0.9,
            "sRGB red should need less than full red in P3, got {red:?}"
        );
        assert!(
            red[1] > 0.0 && red[2] > 0.0,
            "sRGB red is less saturated than P3 red, so it needs some green and blue: {red:?}"
        );
    }

    #[test]
    fn the_adobe_green_row_is_what_it_looks_like() {
        // ⚠ `SRGB_TO_ADOBE_RGB`'s middle row is [0, 1, 0], which reads as a copy-paste error.
        // It is correct, and this pins it by an independent route: the luminance of sRGB green
        // expressed in Adobe RGB, through Adobe's own Y coefficients, must come back to sRGB
        // green's known 0.7152.
        let g = OutputSpace::AdobeRgb.from_srgb([0.0, 1.0, 0.0]);
        let y = 0.2974 * g[0] + 0.6273 * g[1] + 0.0753 * g[2];
        assert!(
            (y - 0.7152).abs() < 1e-3,
            "sRGB green in Adobe RGB is {g:?}, luminance {y}, want 0.7152"
        );
    }

    #[test]
    fn prophoto_is_not_offered_as_a_display() {
        // ⚠ No such monitor exists. Offering it is a pure footgun, and building the list from
        // `ALL` is exactly how it would creep back in.
        assert!(!OutputSpace::DISPLAYS.contains(&OutputSpace::ProPhoto));
        for s in [
            OutputSpace::Srgb,
            OutputSpace::DisplayP3,
            OutputSpace::AdobeRgb,
        ] {
            assert!(
                OutputSpace::DISPLAYS.contains(&s),
                "{} should be offered as a display",
                s.label()
            );
        }
    }

    #[test]
    fn the_eight_bit_helper_agrees_with_the_float_one() {
        // ⚠ Two implementations of one transform, so they must be pinned to each other. The
        // 8-bit path exists only for thumbnails, which have no float behind them; anywhere a
        // float is available the float path is the correct one.
        for s in OutputSpace::DISPLAYS {
            for v in [0u8, 1, 64, 128, 200, 255] {
                let src = [v, 255 - v, 128];
                let eight = s.convert_srgb8(src);
                let float = {
                    let lin = src.map(|c| crate::srgb_to_linear(c as f32 / 255.0));
                    let out = s.from_srgb(lin);
                    out.map(|c| (s.encode(c) * 255.0).round() as u8)
                };
                for i in 0..3 {
                    assert!(
                        eight[i].abs_diff(float[i]) <= 1,
                        "{}: {src:?} channel {i}: 8-bit {} vs float {}",
                        s.label(),
                        eight[i],
                        float[i]
                    );
                }
            }
        }
    }
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p color output_space`
Expected: FAIL — `no method named from_srgb`, `no associated item named DISPLAYS`.

- [ ] **Step 4: Implement**

In `crates/color/src/output_space.rs`, extend the `use` at the top:

```rust
use crate::spaces::{
    apply, PROPHOTO_TO_ADOBE_RGB, PROPHOTO_TO_DISPLAY_P3, PROPHOTO_TO_SRGB, SRGB_TO_ADOBE_RGB,
    SRGB_TO_DISPLAY_P3,
};
```

Add inside `impl OutputSpace`, after `ALL`:

```rust
    /// The spaces a *display* can plausibly be.
    ///
    /// ⚠ Deliberately not `ALL`: no ProPhoto monitor exists, so offering it is a pure footgun.
    /// Building a display list from `ALL` is exactly how it would creep back in.
    pub const DISPLAYS: [OutputSpace; 3] = [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
    ];

    /// Linear **sRGB** → this space's linear primaries.
    ///
    /// ⚠ The sibling of [`Self::from_working`], and a different starting point: that one leaves
    /// the ProPhoto working space, this one leaves already-rendered sRGB output. The display
    /// transform uses this because the preview pipeline still renders sRGB — see the spec for
    /// why that is deliberate and not an oversight.
    ///
    /// ⚠ sRGB is the identity, returned without touching a matrix.
    #[inline]
    pub fn from_srgb(self, c: [f32; 3]) -> [f32; 3] {
        match self {
            OutputSpace::Srgb => c,
            OutputSpace::DisplayP3 => apply(&SRGB_TO_DISPLAY_P3, c),
            OutputSpace::AdobeRgb => apply(&SRGB_TO_ADOBE_RGB, c),
            // Not reachable through `DISPLAYS`; defined so the match is total and a future
            // caller cannot get silence. ProPhoto's primaries contain sRGB's, so the honest
            // answer is the ordinary conversion.
            OutputSpace::ProPhoto => {
                apply(&crate::spaces::SRGB_TO_PROPHOTO, c)
            }
        }
    }

    /// Convert one **sRGB-encoded 8-bit** pixel into this space, encoded 8-bit.
    ///
    /// ⚠ For thumbnails only, which are 8-bit camera JPEGs with no float behind them. Anywhere
    /// a float is available, use [`Self::from_srgb`] before quantizing instead: an 8-bit → 8-bit
    /// conversion compresses the range and costs roughly 0.8 bits, which shows as banding on a
    /// smooth gradient.
    #[inline]
    pub fn convert_srgb8(self, c: [u8; 3]) -> [u8; 3] {
        if self == OutputSpace::Srgb {
            return c;
        }
        let lin = c.map(|v| crate::srgb_to_linear(v as f32 / 255.0));
        let out = self.from_srgb(lin);
        [
            (self.encode(out[0]) * 255.0).round() as u8,
            (self.encode(out[1]) * 255.0).round() as u8,
            (self.encode(out[2]) * 255.0).round() as u8,
        ]
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p color`
Expected: PASS — 26 existing plus 7 new.

- [ ] **Step 6: Sabotage each new test and watch it fail**

⚠ Confirm each sabotage **compiles** before believing a pass — an edit that fails to compile looks
exactly like one that passes when you grep for a test result. Revert after each.

| sabotage | must fail |
|---|---|
| `OutputSpace::Srgb => apply(&SRGB_TO_DISPLAY_P3, c)` in `from_srgb` | `srgb_is_bit_exactly_the_identity_for_the_display_transform` |
| swap `SRGB_TO_DISPLAY_P3`'s first two rows | `the_display_transform_maps_white_to_white` |
| `SRGB_TO_ADOBE_RGB` middle row → `[0.0, 0.9, 0.0]` | `the_adobe_green_row_is_what_it_looks_like` **and** `the_display_transform_maps_white_to_white` |
| `DISPLAYS` → `[Srgb, DisplayP3, ProPhoto]` | `prophoto_is_not_offered_as_a_display` |
| `convert_srgb8` skips the `srgb_to_linear` decode (use `v as f32 / 255.0` directly) | `the_eight_bit_helper_agrees_with_the_float_one` |

⚠ For `a_wider_container_needs_smaller_coordinates`, the discriminating sabotage is **inverting the
matrix** — replace `SRGB_TO_DISPLAY_P3` with `PROPHOTO_TO_DISPLAY_P3`, whose first entry is 1.63.
That is the failure mode the test exists for: a preview that is *more* saturated, i.e. exactly the
bug, while a "the value changed" test would pass.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add crates/color
git commit -m "feat(color): sRGB to display-space conversion"
```

---

### Task 2: the render path carries a display space

⚠ Everything here passes `Srgb`, so **behaviour is unchanged and every golden stays byte-identical**.
Task 3 makes it a real choice.

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/app/src/render_engine.rs`:

```rust
    #[test]
    fn the_display_space_changes_the_packed_pixels_and_srgb_does_not() {
        // ⚠ Two claims. The sRGB half is the regression guard — it is the default, and if it
        // moved every preview would shift for people who never touched the setting. The P3 half
        // proves the parameter is actually reaching `pack`; without it the setting would be a
        // dropdown that does nothing, which is the failure this whole task risks.
        let mut buf = ImageBuf::new(2, 1);
        buf.set_pixel(0, 0, color::srgb_to_prophoto([0.9, 0.1, 0.1]));
        buf.set_pixel(1, 0, color::srgb_to_prophoto([0.2, 0.6, 0.3]));
        let buf = std::sync::Arc::new(buf);

        let srgb = render_once(&buf, &Edits::default(), 2, 1.0, color::OutputSpace::Srgb);
        let p3 = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::OutputSpace::DisplayP3,
        );

        assert_ne!(srgb.rgba, p3.rgba, "the display space did not reach `pack`");
        // The corrected image must be *less* saturated in red, not more — an inverted transform
        // would also differ, and would look like no fix at all.
        assert!(
            p3.rgba[0] < srgb.rgba[0],
            "P3 red {} should be below sRGB red {}",
            p3.rgba[0],
            srgb.rgba[0]
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p app --lib render_engine`
Expected: FAIL — `render_once` takes 4 arguments, not 5.

- [ ] **Step 3: Thread the space through**

In `crates/app/src/render_engine.rs`:

Add the field to `Request` (line ~51):

```rust
struct Request {
    edits: Edits,
    max_dim: usize,
    region: Option<RegionRect>,
    /// The display's primaries. ⚠ Applied in `pack`, *after* the pipeline — never as a
    /// pipeline stage, or the histogram becomes display-referred. See the spec.
    display_space: color::OutputSpace,
}
```

Change `RenderEngine::request`:

```rust
    pub fn request(
        &self,
        edits: Edits,
        max_dim: usize,
        region: Option<RegionRect>,
        display_space: color::OutputSpace,
    ) {
        let _ = self.req_tx.send(Request {
            edits,
            max_dim,
            region,
            display_space,
        });
    }
```

Change `pack` (line ~158):

```rust
/// Quantize a rendered frame for the GPU, converting into the display's primaries on the way.
///
/// ⚠ The conversion happens **here, in float, before quantizing** — not on the packed bytes.
/// An 8-bit → 8-bit conversion compresses the range and costs roughly 0.8 bits, which shows as
/// banding on a smooth sky. It also happens *after* the pipeline rather than inside it, so the
/// histogram stays image-referred; see the spec for why that is the whole design.
fn pack(
    out: ImageBuf,
    region: Option<Region>,
    max_dim: usize,
    display_space: color::OutputSpace,
) -> RenderedFrame {
    let out = if display_space == color::OutputSpace::Srgb {
        out
    } else {
        // The image is sRGB-*encoded* here, so decode, change primaries, re-encode.
        out.map_pixels(move |c| {
            let lin = c.map(color::srgb_to_linear);
            let conv = display_space.from_srgb(lin);
            [
                display_space.encode(conv[0]),
                display_space.encode(conv[1]),
                display_space.encode(conv[2]),
            ]
        })
    };
    RenderedFrame {
        width: out.width(),
        height: out.height(),
        rgba: out.to_rgba8(),
        region,
        max_dim,
    }
}
```

Update the two `pack` call sites — line ~551 becomes
`(pack(tail.run(input), region, max_dim, display_space), ran)` and line ~573 becomes
`pack(build_pipeline(edits, scale).run(preview), None, max_dim, display_space)`. Thread
`display_space` into whatever functions enclose them (`render_cached` and `render_once`), and pass
`req.display_space` where the worker loop calls `render_cached`.

`render_once`'s new signature:

```rust
pub fn render_once(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    display_space: color::OutputSpace,
) -> RenderedFrame {
```

- [ ] **Step 4: Fix the callers**

`crates/app/src/decode_worker.rs:124` — pass `Srgb` with a comment saying why it does not matter:

```rust
                // ⚠ `Srgb` regardless of the display setting: this baseline texture is never
                // drawn. `show_before` is an ordinary render now, and the baseline survives only
                // for its dimensions (`develop/panel/geometry.rs`).
                let baseline = render_once(
                    &buffer,
                    &Edits::default(),
                    req.max_dim,
                    source_scale,
                    color::OutputSpace::Srgb,
                );
```

`crates/app/src/main.rs:383` — `Session::request` gains the space in Task 3; for now pass
`color::OutputSpace::Srgb` so this task compiles on its own:

```rust
        self.engine.request(edits, max_dim, region, color::OutputSpace::Srgb);
```

`crates/app/src/main.rs:1150` — `engine.request(edits.clone(), fit_cap, None, color::OutputSpace::Srgb);`

The three `engine.request(..)` calls in `render_engine.rs`'s own tests (lines ~1105, ~1143, ~1155)
each gain a trailing `color::OutputSpace::Srgb`.

⚠ Every existing assertion in those tests stays **verbatim**. Unchanged assertions still passing is
the evidence this task changed no behaviour.

- [ ] **Step 5: Run the whole suite**

Run: `cargo test --all`
Expected: PASS, 734 + 1 new. ⚠ **All 39 goldens byte-identical** — this task touches no pipeline code.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add -A
git commit -m "feat(app): the render path carries a display space"
```

---

### Task 3: the setting, and the canvas is corrected

**Files:**
- Modify: `crates/app/src/settings.rs`
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/modals/settings.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/app/src/settings.rs`:

```rust
    #[test]
    fn a_settings_file_from_before_the_display_space_still_loads() {
        // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently
        // reverts *every* setting — worse than an error. This is the guard, and the same one
        // `view_demosaic` and `export` each needed.
        //
        // ⚠ What it discriminates is the **value**: the default must be sRGB. A wide space
        // arriving by default would change every existing user's preview with no action from
        // them, which is the opposite of what a colour-correctness feature should do.
        let json = r#"{"confirm_preset_delete":true}"#;
        let s: Settings = serde_json::from_str(json).expect("an older settings file should load");
        assert_eq!(s.display_space, color::OutputSpace::Srgb);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p app --lib settings`
Expected: FAIL — no field `display_space` on `Settings`.

- [ ] **Step 3: Add the field**

In `crates/app/src/settings.rs`, add to `Settings` after `view_demosaic`:

```rust
    /// The primaries of the monitor, so the preview can be converted into them.
    ///
    /// ⚠ Applied **after** the pipeline, in `render_engine::pack`, never as a pipeline stage —
    /// putting it in the pipeline would silently make the histogram display-referred.
    ///
    /// ⚠ Defaults to `Srgb`, which is the identity, so nobody who ignores this sees any change.
    pub display_space: color::OutputSpace,
```

and to its `Default` impl:

```rust
            display_space: color::OutputSpace::default(),
```

⚠ Check whether `Settings` carries a container-level `#[serde(default)]`. If it does, **do not**
add a field-level one — it would be redundant and imply a guarantee it is not the source of. If it
does not, add `#[serde(default)]` to the field.

- [ ] **Step 4: Give `Session` the space and supply it from one place**

In `crates/app/src/main.rs`, add to `struct Session` beside `hist`:

```rust
    /// The display's primaries, copied from `Settings` when the session opens.
    ///
    /// ⚠ Lives here rather than being passed per call so that `Session::request` — the single
    /// choke point for all seven request sites — can supply it without any of them changing.
    display_space: color::OutputSpace,
```

Change `Session::request` (line ~382):

```rust
    fn request(&self, edits: Edits, max_dim: usize, region: Option<RegionRect>) {
        self.engine.request(edits, max_dim, region, self.display_space);
    }
```

Set it where the `Session` is constructed (`main.rs:~1158`, the `Session { .. }` literal) with
`display_space: self.settings.display_space,`, and change the nearby
`engine.request(edits.clone(), fit_cap, None, color::OutputSpace::Srgb);` to pass
`self.settings.display_space`.

Add the apply function beside `apply_view_demosaic`:

```rust
    /// Adopt a new display space: re-render the open photo, and drop the thumbnail textures so
    /// they are rebuilt through the new conversion.
    ///
    /// ⚠ The caller must compare against the current value first — the Settings dialog body runs
    /// every frame, so an unguarded call would re-render sixty times a second. Same rule
    /// `apply_view_demosaic` follows.
    fn apply_display_space(&mut self, space: color::OutputSpace, ctx: &egui::Context) {
        self.settings.display_space = space;
        self.settings_store.save(&self.settings);
        if let Some(s) = &mut self.session {
            s.display_space = space;
            let tool = self.tool;
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        }
        // Thumbnails are converted at upload, so the textures have the old space baked in.
        // Reloading is the only way back; it streams in parallel, so the visible cells refill
        // quickly rather than the window freezing.
        if let Some(lib) = &mut self.library {
            for t in lib.thumbs.iter_mut() {
                *t = None;
            }
            lib.loader = rustroom_app::thumb_loader::ThumbLoader::new(
                lib.entries.iter().map(|e| e.path.clone()).collect(),
                {
                    let ctx = ctx.clone();
                    move || ctx.request_repaint()
                },
            );
        }
    }
```

⚠ Match the surrounding code for `settings_store`'s real name and for how `Library`'s loader was
originally constructed — read the folder-open path in `main.rs` and mirror it exactly rather than
inventing a second construction.

- [ ] **Step 5: Add the dropdown**

In `crates/app/src/modals/settings.rs`, mirror the `view_demosaic` pattern. Near the top, beside
`let mut tier = app.settings.view_demosaic;`:

```rust
    let mut display_space = app.settings.display_space;
```

In the body, after the `view_demosaic` row:

```rust
        ui.horizontal(|ui| {
            ui.label("Display");
            egui::ComboBox::from_id_salt("display_space_pick")
                .selected_text(display_space.label())
                .show_ui(ui, |ui| {
                    // ⚠ `DISPLAYS`, not `ALL`: no ProPhoto monitor exists.
                    for s in color::OutputSpace::DISPLAYS {
                        ui.selectable_value(&mut display_space, s, s.label());
                    }
                });
        });
        theme::hint(
            ui,
            "The primaries of your monitor. Most are sRGB; Apple laptops and recent phones \
             are Display P3. Setting this wrong makes the preview disagree with what you \
             export — it does not change the exported file.",
        );
```

and beside the existing `if tier != app.settings.view_demosaic { .. }`:

```rust
    if display_space != app.settings.display_space {
        app.apply_display_space(display_space, ctx);
    }
```

- [ ] **Step 6: Run the suite**

Run: `cargo test --all`
Expected: PASS. All 39 goldens byte-identical.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add -A
git commit -m "feat(app): a display-space setting, applied to the canvas"
```

---

### Task 4: thumbnails are converted too

**Files:**
- Modify: `crates/app/src/shell.rs`

⚠ **Not optional polish.** The filmstrip sits directly beside the canvas showing the same photo, so
correcting only the canvas leaves a visibly more saturated strip next to a corrected image — a
mismatch more noticeable than the original error.

- [ ] **Step 1: Convert at the upload site**

In `crates/app/src/shell.rs`, replace the thumbnail upload block (around line 100):

```rust
        // Pull finished thumbnails into textures.
        if let Some(lib) = &mut self.library {
            let space = self.settings.display_space;
            for res in lib.loader.drain() {
                if let (Some(t), true) = (res.thumb, res.index < lib.thumbs.len()) {
                    // ⚠ Converted here rather than in `ThumbLoader`, for two reasons: its
                    // loader is a bare `fn` pointer that cannot capture the space, and its six
                    // tests stay untouched this way.
                    //
                    // ⚠ 8-bit in, 8-bit out, unlike the canvas — a thumbnail is a camera JPEG
                    // with no float behind it, so there is no precision to preserve.
                    let mut rgba = t.rgba;
                    if space != color::OutputSpace::Srgb {
                        for px in rgba.chunks_exact_mut(4) {
                            let out = space.convert_srgb8([px[0], px[1], px[2]]);
                            px[0] = out[0];
                            px[1] = out[1];
                            px[2] = out[2];
                        }
                    }
                    let img =
                        egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &rgba);
                    lib.thumbs[res.index] = Some(ctx.load_texture(
                        format!("thumb{}", res.index),
                        img,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }
```

⚠ If the borrow checker objects to reading `self.settings` while `&mut self.library` is held, hoist
`let space = self.settings.display_space;` **above** the `if let Some(lib) = &mut self.library`.
`OutputSpace` is `Copy`, so the hoist is free — the same pattern the tool hoist uses elsewhere in
this crate.

- [ ] **Step 2: Verify it builds and the suite is green**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(app): thumbnails follow the display space too"
```

---

### Task 5: verify in the app, then update the docs

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: PASS. ⚠ **All 39 goldens byte-identical** — this feature touches no pipeline code at all,
so a moved golden means something went in the wrong place.

- [ ] **Step 2: Launch the GUI and check by hand**

```bash
cargo run --release -p app
```

Check, in order:

1. Settings shows a **Display** dropdown with exactly three entries — no ProPhoto.
2. Open a photo with a saturated subject. Switch Display to **Display P3**: the canvas should
   become **less** saturated. ⚠ If it becomes *more* saturated the matrix is inverted — that is
   the failure `a_wider_container_needs_smaller_coordinates` guards, and it looks like "the
   setting does nothing helpful" rather than like a bug.
3. The filmstrip thumbnails should shift the same way, not stay saturated beside a corrected
   canvas.
4. **The histogram must not move at all** when the setting changes. This is the property the whole
   design exists to protect; if it moves, the transform ended up inside the pipeline.
5. Set Display P3 and compare the canvas against the same photo exported to sRGB and opened in
   Preview — they should now match. That is the actual acceptance test for this feature.
6. Restart the app: the setting persists.

- [ ] **Step 3: Update `CLAUDE.md`**

Add a section under "Current status" in the house style of those around it: what landed, the ⚠
findings (wgpu leaving `CAMetalLayer.colorspace` nil and Apple's header saying nil means no colour
matching; the measured display primaries and the 0.897-vs-1.0 figure; why the transform is a
post-step and not a pipeline stage, i.e. the histogram; the `[0, 1, 0]` Adobe row; ProPhoto
excluded from `DISPLAYS`), the test count, and the caveats from the spec's "What this does not do".
Update the **▶ NEXT ACTION** block: D is done; remaining are the **macOS `CAMetalLayer.colorspace`
follow-up**, **C** camera input profiles, **M3.2** lens corrections, **M3.4** virtual copies.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the display profile for the preview"
```

⚠ **Do not push.** The user pushes.

---

## Self-Review

**Spec coverage:** the two matrices and `from_srgb` (Task 1) ✓; `DISPLAYS` excluding ProPhoto
(Task 1) ✓; conversion in float inside `pack` (Task 2) ✓; pipeline and histogram untouched
(Tasks 2, 5 step 2 check 4) ✓; `Settings.display_space` with the serde-default guard (Task 3) ✓;
the dropdown and the change guard (Task 3) ✓; thumbnails converted at upload with a reload on
change (Tasks 3, 4) ✓; egui chrome deliberately unconverted (no task — it is the absence of one,
stated in the spec) ✓; every test the spec names (Task 1 steps 2 and 6, Task 2 step 1) ✓; caveats
recorded (Task 5) ✓.

**Type consistency:** `OutputSpace::{from_srgb, convert_srgb8, DISPLAYS, encode, label}` are used
under those exact names in Tasks 2, 3 and 4. `pack(out, region, max_dim, display_space)` and
`render_once(buffer, edits, max_dim, source_scale, display_space)` match every call site listed.
`Session.display_space` is set in Task 3 and read by `Session::request` in the same task.
`apply_display_space(space, ctx)` is defined in Task 3 step 4 and called in step 5.
