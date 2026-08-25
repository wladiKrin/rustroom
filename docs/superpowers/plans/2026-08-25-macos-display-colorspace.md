# macOS display colorspace — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On macOS, tag the window's `CAMetalLayer` as sRGB so Core Animation colour-matches the preview using the display's calibrated profile — including following the window between monitors — and stand the manual setting down while it does.

**Architecture:** One thin macOS-only module sets the layer's colorspace once at startup and reports whether it worked. That boolean feeds a single pure function which decides, for every render and every thumbnail, whether we convert or the OS does — so exactly one mechanism is ever live and a failure degrades to the manual setting rather than to nothing.

**Tech Stack:** Rust, `objc2 0.6` + `objc2-quartz-core` / `objc2-core-graphics` / `objc2-app-kit` 0.3 (macOS-only), eframe 0.35.

**Spec:** `docs/superpowers/specs/2026-08-25-macos-display-colorspace-design.md`

**Before anything:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
git checkout -b macos-display-colorspace
```

Branch per feature; **never push**.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/app/src/settings.rs` | *modify* — `Settings::effective_display_space(os_managed)` and its tests |
| `crates/app/src/display_colorspace.rs` | **create** — macOS-only; find the `CAMetalLayer`, set sRGB, report success |
| `crates/app/Cargo.toml` | *modify* — three macOS-only `objc2-*` crates + `raw-window-handle` |
| `crates/app/src/main.rs` | *modify* — call it in `new`, hold `os_manages_colour`, route the three read sites |
| `crates/app/src/modals/settings.rs` | *modify* — note instead of dropdown when the OS is managing |

⚠ `display_colorspace.rs` is the **only** `#[cfg]`-gated module. Everything else branches on a
**runtime bool**, so both paths compile on every platform — the lesson from the menu bar, where
`#[cfg]`-ing a whole UI branch meant nobody ever compiled half of it.

---

### Task 1: one function decides which mechanism is live

**Files:**
- Modify: `crates/app/src/settings.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/settings.rs`'s test module:

```rust
#[test]
fn the_os_managing_colour_overrides_the_manual_setting() {
    // ⚠⚠ The property that makes double conversion unrepresentable. If both were live we would
    // convert sRGB→P3 ourselves and Core Animation would then treat *that* as sRGB and convert
    // again — visibly undersaturated, and varying per monitor.
    let mut s = Settings::default();
    for space in color::OutputSpace::DISPLAYS {
        s.display_space = space;
        assert_eq!(
            s.effective_display_space(true),
            color::OutputSpace::Srgb,
            "{} should be ignored while the OS is managing colour",
            space.label()
        );
    }
}

#[test]
fn without_the_os_managing_the_manual_setting_is_used() {
    // ⚠ The fallback, and it is not a formality: if tagging the layer ever fails, this is what
    // stops macOS being left *uncorrected* — which would be worse than what piece D shipped.
    let mut s = Settings::default();
    for space in color::OutputSpace::DISPLAYS {
        s.display_space = space;
        assert_eq!(s.effective_display_space(false), space);
    }
}

#[test]
fn the_stored_setting_survives_the_os_taking_over() {
    // ⚠ `effective_display_space` must not *write*. A settings file shared between a Mac and a
    // Linux machine has to keep a sensible value for the latter.
    let mut s = Settings::default();
    s.display_space = color::OutputSpace::AdobeRgb;
    let _ = s.effective_display_space(true);
    assert_eq!(s.display_space, color::OutputSpace::AdobeRgb);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app --lib settings`
Expected: FAIL — `no method named effective_display_space`.

- [ ] **Step 3: Implement it**

Add to `impl Settings` in `crates/app/src/settings.rs`:

```rust
    /// The space the *renderer* should convert into.
    ///
    /// ⚠⚠ Every read of `display_space` in the render path must go through here. When the
    /// operating system is colour-matching the window for us — macOS, once the `CAMetalLayer`
    /// carries a colorspace — converting as well would be a **double conversion**: we would
    /// produce P3 and Core Animation would then treat it as sRGB and convert again, leaving the
    /// preview undersaturated and varying per monitor. Returning `Srgb` here makes that
    /// unrepresentable rather than something to remember.
    ///
    /// ⚠ The stored value is deliberately left alone, so a settings file shared with a Linux
    /// machine keeps a sensible choice for it.
    pub fn effective_display_space(&self, os_managed: bool) -> color::OutputSpace {
        if os_managed {
            color::OutputSpace::Srgb
        } else {
            self.display_space
        }
    }
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p app --lib settings`
Expected: PASS.

- [ ] **Step 5: Sabotage**

| sabotage | must fail |
|---|---|
| body → `self.display_space` unconditionally | `the_os_managing_colour_overrides_the_manual_setting` |
| body → `color::OutputSpace::Srgb` unconditionally | `without_the_os_managing_the_manual_setting_is_used` |

Confirm each compiles before believing the result, then revert.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
git add crates/app/src/settings.rs
git commit -m "feat(app): one place decides whether we or the OS correct the display"
```

⚠ Check clippy's **exit code**, not grep output — a `clippy … | grep` pipeline reports grep's
status, which let a failing clippy into a commit earlier in this colour-management work.

---

### Task 2: tag the layer on macOS

**Files:**
- Create: `crates/app/src/display_colorspace.rs`
- Modify: `crates/app/Cargo.toml`
- Modify: `crates/app/src/main.rs`

⚠ This task lands as **one commit**. `display_colorspace` is a module of a *binary* crate, so a
`pub fn` with no caller is dead code that fails `clippy -D warnings` — the trap this project has
hit six times. It ships with its call site.

- [ ] **Step 1: Add the dependencies**

In `crates/app/Cargo.toml`, under the existing
`[target.'cfg(target_os = "macos")'.dependencies]` block (the one holding `muda` and `objc2`):

```toml
objc2-app-kit = { version = "0.3", default-features = false, features = ["std", "NSView", "NSResponder", "objc2-quartz-core"] }
objc2-quartz-core = { version = "0.3", default-features = false, features = ["std", "CALayer", "CAMetalLayer", "objc2-core-graphics"] }
objc2-core-graphics = { version = "0.3", default-features = false, features = ["std", "CGColorSpace"] }
```

And in the **normal** `[dependencies]` block, so the handle type is nameable on every platform:

```toml
raw-window-handle = "0.6"
```

⚠ The feature names above are the real ones from those crates' manifests (`CALayer`,
`CAMetalLayer`, `CGColorSpace`, `NSView`). If the compiler asks for another, add it — feature
resolution is compile-checked, so a missing one is a build error, never a silent gap.

- [ ] **Step 2: Write the module**

Create `crates/app/src/display_colorspace.rs`:

```rust
//! Hand the preview's colour management to macOS.
//!
//! ⚠ **wgpu never sets `CAMetalLayer.colorspace`** — it touches only
//! `wantsExtendedDynamicRangeContent` — and Apple's SDK header says a nil colorspace means *"no
//! colormatching occurs"*. So our sRGB pixels were being driven into the panel's native primaries
//! unconverted. Setting the property tells Core Animation what they are, and it matches them to
//! the display's own **calibrated** profile — following the window between monitors, which the
//! manual setting structurally cannot do.
//!
//! ⚠ Set **once**: `wgpu_hal`'s `configure()` sets device, pixel format, framebuffer-only, EDR,
//! drawable count and drawable size, and never touches `colorspace`. The value survives every
//! resize and surface rebuild.
//!
//! ⚠ Deliberately thin, because none of it can be unit tested — it needs a live window and a real
//! Metal layer. The decision it feeds is a pure function; see `Settings::effective_display_space`.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_core_graphics::{kCGColorSpaceSRGB, CGColorSpace};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use raw_window_handle::RawWindowHandle;

/// Tag the window's Metal layer as sRGB. Returns whether it worked.
///
/// ⚠ The return value is load-bearing: a `false` puts the manual setting back in charge rather
/// than leaving the preview uncorrected. Never change this to return `()`.
pub fn tag_as_srgb(handle: RawWindowHandle) -> bool {
    let RawWindowHandle::AppKit(handle) = handle else {
        return false;
    };
    // SAFETY: eframe hands us a live `NSView` for the window we are rendering into, and this runs
    // on the main thread during app construction.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(layer) = view.layer() else {
        return false;
    };
    let Some(metal) = find_metal_layer(&layer) else {
        return false;
    };
    let Some(srgb) = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })) else {
        return false;
    };
    metal.setColorspace(Some(&srgb));
    // Read it back rather than trusting the setter: this is the whole feature, and a silent
    // no-op here would look exactly like the OS not colour-managing.
    metal.colorspace().is_some()
}

/// The view's own layer, or the first `CAMetalLayer` among its descendants.
///
/// ⚠ Both shapes are handled because wgpu may nest it: `wgpu_hal`'s own occlusion workaround
/// walks `superlayer()` *upward* from the metal layer to find the view, which only makes sense if
/// the metal layer is sometimes a sublayer. Assuming either shape would work until it didn't.
fn find_metal_layer(layer: &CALayer) -> Option<Retained<CAMetalLayer>> {
    if let Some(metal) = layer.downcast_ref::<CAMetalLayer>() {
        return Some(metal.retain());
    }
    // SAFETY: reading the sublayer list of a live layer on the main thread.
    let subs = unsafe { layer.sublayers() }?;
    subs.iter().find_map(|sub| find_metal_layer(&sub))
}
```

⚠ If `downcast_ref` or `retain()` does not resolve, read the vendored `objc2 0.6` source rather
than guessing — `AnyObject::downcast_ref::<T>()` is the safe checked cast and `CAMetalLayer`
implements `DowncastTarget`. Two API facts were wrong from memory during the menu-bar work and
cost a compile each.

- [ ] **Step 3: Declare it and call it**

In `crates/app/src/main.rs`, beside the existing `#[cfg(target_os = "macos")] mod native_menu;`:

```rust
#[cfg(target_os = "macos")]
mod display_colorspace;
```

Add the field to `RustroomApp`:

```rust
    /// Whether the OS is colour-matching the window for us, so we must not also convert.
    ///
    /// ⚠ Runtime, not `#[cfg]`: a macOS build where tagging failed has to take the manual path,
    /// and keeping it a value means both branches compile on every platform.
    os_manages_colour: bool,
```

In `RustroomApp::new(cc)`, before the struct is built:

```rust
        #[cfg(target_os = "macos")]
        let os_manages_colour = cc
            .window_handle()
            .ok()
            .map(|h| display_colorspace::tag_as_srgb(h.as_raw()))
            .unwrap_or(false);
        #[cfg(not(target_os = "macos"))]
        let os_manages_colour = false;
```

and set the field from it.

- [ ] **Step 4: Route the three read sites through the decision**

In `crates/app/src/main.rs`, replace every `self.settings.display_space` **in the render path**
with `self.settings.effective_display_space(self.os_manages_colour)`:

1. the `FrameSpec` built when a photo opens (`engine.request(...)` in `open_path`);
2. the `Session { display_space: ... }` initializer;
3. the `let space = ...` feeding `ThumbLoader::new` in `open_folder`.

And in `apply_display_space`, take the effective value rather than the raw one:

```rust
    fn apply_display_space(&mut self, space: color::OutputSpace, ctx: &egui::Context) {
        self.settings.display_space = space;
        self.save_settings();
        let space = self.settings.effective_display_space(self.os_manages_colour);
```

leaving the rest of that function unchanged.

⚠ `grep -n "settings.display_space" crates/app/src/main.rs` afterwards: the **only** remaining
raw reads should be the two assignments in `apply_display_space` and the modal's own copy. Any
other is a site that would still convert while the OS also does.

- [ ] **Step 5: Build and run the suite**

Run: `cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: PASS. ⚠ **All 39 goldens byte-identical** — nothing here touches the pipeline.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(app): tag the window as sRGB so macOS colour-matches the preview"
```

---

### Task 3: the Settings dialog says who is in charge

**Files:**
- Modify: `crates/app/src/modals/settings.rs`

- [ ] **Step 1: Replace the dropdown when the OS is managing**

Find the `Display` `ui.horizontal` block added by piece D and wrap it:

```rust
        if app.os_manages_colour {
            // ⚠ A note, not a disabled dropdown. There is nothing to choose: the OS is using the
            // display's calibrated profile and following the window between monitors, which no
            // value here could improve on.
            ui.horizontal(|ui| {
                ui.label("Display");
                ui.label("handled by macOS");
            });
            theme::hint(
                ui,
                "Your display's own colour profile is applied by the system, including any \
                 calibration, and it follows the window if you move it to another monitor.",
            );
        } else {
            ui.horizontal(|ui| {
                ui.label("Display");
                egui::ComboBox::from_id_salt("display_space_pick")
                    .selected_text(display_space.label())
                    .show_ui(ui, |ui| {
                        // ⚠ `DISPLAYS`, not `ALL`: no ProPhoto monitor exists, so offering it is
                        // a pure footgun.
                        for s in color::OutputSpace::DISPLAYS {
                            ui.selectable_value(&mut display_space, s, s.label());
                        }
                    });
            });
            theme::hint(
                ui,
                "The primaries of your monitor. Most are sRGB; recent Apple displays and many \
                 laptops are Display P3. Affects only what you see — never an export, and never \
                 the histogram.",
            );
        }
        ui.separator();
```

⚠ A **runtime** branch, so both halves compile on macOS and on Linux. The menu-bar work shipped a
`#[cfg]`-gated UI branch that the developer never compiled and CI never exercised; that is the
mistake this avoids.

⚠ The change guard at the bottom of `show` stays exactly as it is. When the OS is managing,
`display_space` is never mutated, so the guard never fires and no thumbnail reload happens.

- [ ] **Step 2: Build and check**

Run: `cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(app): Settings says when macOS is handling the display profile"
```

---

### Task 4: verify in the app, then update the docs

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: PASS, 39 goldens byte-identical.

- [ ] **Step 2: Confirm the layer was actually tagged**

The feature is invisible when it silently fails, which is exactly how piece D's setting shipped
inert. Add a temporary print in `new` after the tagging call:

```rust
        eprintln!("os_manages_colour = {os_manages_colour}");
```

```bash
cargo run --release -p app 2>&1 | grep os_manages_colour
```

Expected: `os_manages_colour = true` on macOS.
⚠ If it prints `false`, `find_metal_layer` did not find the layer — instrument it to print the
class name of the view's layer and its sublayers before changing anything else. **Remove the print
before committing.**

- [ ] **Step 3: Check it by eye, against a reference**

1. Settings shows **"Display — handled by macOS"** and no dropdown.
2. Export a photo as sRGB JPEG, open it in **Preview**, and compare side by side with the canvas.
   They should **match**. Before this change the canvas was more saturated.
3. ⚠ **The multi-monitor drag — the case that motivated the whole change, and the one that has
   never been tested.** Move the window to an external display. The image should stay correct on
   both, with no re-render and no visible jump beyond the displays' genuine difference.
4. The egui chrome should now match other macOS apps' greys, where before it was slightly
   oversaturated.
5. The histogram must not move at any point.

- [ ] **Step 4: Update `CLAUDE.md`**

A section in the house style: what landed, the ⚠ findings (the nil-colorspace fact and the SDK
header; `configure()` never resetting it, hence set-once; the return value keeping exactly one
mechanism live and degrading to the manual setting; the runtime bool rather than `#[cfg]` for the
UI branch), the test count, and the caveats from the spec — macOS only, Linux/Windows keep the
manual setting **and its multi-monitor gap**, still no soft-proofing. Update the **▶ NEXT ACTION**
block: this is done; **C** (camera input profiles) is next, then **M3.2** and **M3.4**.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record handing the preview's colour to macOS"
```

⚠ **Do not push.**

---

## Self-Review

**Spec coverage:** set `CAMetalLayer.colorspace` to sRGB once at startup (Task 2) ✓; reach it via
the `NSView`, handling both layer shapes (Task 2) ✓; return success and let it decide which
mechanism is live (Tasks 1, 2) ✓; fallback to the manual setting on failure (Task 1 test, Task 2
call) ✓; thumbnails routed through the same decision (Task 2 step 4) ✓; Settings note instead of
dropdown (Task 3) ✓; stored value untouched (Task 1 test) ✓; multi-monitor and Preview checks
(Task 4) ✓; caveats recorded (Task 4) ✓.

**Type consistency:** `Settings::effective_display_space(bool) -> color::OutputSpace` is defined in
Task 1 and used with that signature in Tasks 2 and 3. `tag_as_srgb(RawWindowHandle) -> bool` is
defined in Task 2 and called there. `RustroomApp::os_manages_colour` is declared in Task 2 and read
in Tasks 2 and 3.
