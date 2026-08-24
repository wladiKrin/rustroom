# 1:1 View Quality Setting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The demosaic tier used for the full-resolution 1:1 editing buffer becomes an explicit, persisted setting with a warning about what it costs, instead of a hardcoded `Demosaic::Fast`.

**Architecture:** A new `Settings.view_demosaic` field; the tier is captured in the closure that `DecodeWorker` was built with, so changing it rebuilds that worker; changing it also invalidates the open RAW session's full-resolution buffer so the existing eager-decode path re-fires immediately.

**Tech Stack:** Rust, `egui` 0.35, `serde`. No new dependencies — `app` already depends on `raw-decode` for `Demosaic` (see `batch_export.rs:54`).

**Spec:** `docs/superpowers/specs/2026-08-13-view-demosaic-setting-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell before starting:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/app/src/settings.rs` | Gains `view_demosaic: Demosaic` on `Settings`, its default, and the back-compat load guard. | 1 |
| `crates/app/src/main.rs` | `view_quality_hint` (pure, tested); `full_res_worker` constructor; the `decode_full_res` signature; the Settings dialog control and warning; the invalidate-and-re-decode on change; the export dialog's stale hint. | 2, 3 |

Task 1 is a self-contained data change. Task 2 adds the pure hint function and the worker plumbing. Task 3 is the UI and the behaviour on change. Every task leaves the tree green.

---

## Task 1: `Settings.view_demosaic`

**Files:**
- Modify: `crates/app/src/settings.rs`

- [ ] **Step 1: Write the failing test**

In `crates/app/src/settings.rs`, inside `mod tests`, add directly after the existing
`a_settings_file_from_before_export_options_still_loads`:

```rust
    #[test]
    fn a_settings_file_from_before_view_demosaic_still_loads() {
        // ⚠ Same property as the test above, for the same reason: `load()` is infallible, so
        // a field that fails to parse does not surface an error — it silently reverts *every*
        // setting to its default. A new field must therefore be fillable by `#[serde(default)]`.
        let f = TempFile::new("view_demosaic_old");
        std::fs::write(
            &f.0,
            r#"{"confirm_preset_delete": false, "confirm_export_preset_delete": false}"#,
        )
        .unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old fields must survive");
        assert!(!loaded.confirm_export_preset_delete);
        assert_eq!(
            loaded.view_demosaic,
            raw_decode::Demosaic::Fast,
            "and the new one must default to what linear_scene() already hardcoded"
        );
    }

    #[test]
    fn the_view_tier_round_trips_through_a_file() {
        let f = TempFile::new("view_demosaic_rt");
        let store = SettingsStore::new(f.0.clone());
        // ⚠ Struct-update syntax, not `let mut s = Settings::default();` followed by a field
        // assignment — clippy's `field_reassign_with_default` is warn-by-default and CI runs
        // with `-D warnings`.
        let s = Settings {
            view_demosaic: raw_decode::Demosaic::Best,
            ..Default::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().view_demosaic, raw_decode::Demosaic::Best);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib settings
```

Expected: **compile error**, `no field view_demosaic on type Settings`.

- [ ] **Step 3: Add the field**

In `crates/app/src/settings.rs`, add to the `Settings` struct after `confirm_export_preset_delete`:

```rust
    /// Demosaic tier for the **full-resolution editing buffer** — what the 1:1 view shows.
    ///
    /// ⚠ Independent of `export.demosaic`, and deliberately not mirrored from it. "The export
    /// preset's demosaic" is not one value — there are N named presets plus the last-used —
    /// and the same photo legitimately goes out as web-JPEG-fast and archive-TIFF-best.
    /// Inferring one would make the view's decode cost depend on distant state.
    ///
    /// Defaults to `Fast`, which is what `DecodeOptions::linear_scene()` already hardcodes, so
    /// nobody who ignores this setting sees any change.
    pub view_demosaic: Demosaic,
```

Add the import at the top of the file, beside the existing `use crate::batch_export::ExportOptions;`:

```rust
use raw_decode::Demosaic;
```

And add the field to `impl Default for Settings`, after `confirm_export_preset_delete: true,`:

```rust
            view_demosaic: Demosaic::default(),
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app --lib settings
```

Expected: all pass, including the two new ones.

- [ ] **Step 5: Prove the back-compat test can fail**

Temporarily remove `#[serde(default)]` from above `pub struct Settings` and run:

```bash
cargo test -p app --lib settings
```

Expected: `a_settings_file_from_before_view_demosaic_still_loads` **fails** — the file lacking
`view_demosaic` no longer parses, so `load()` returns `Settings::default()` and
`confirm_preset_delete` comes back `true`. Restore the attribute and re-run to confirm green.

- [ ] **Step 6: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/settings.rs
git commit -m "feat(app): a persisted demosaic tier for the 1:1 view"
```

---

## Task 2: The hint text and the parameterised worker

**Files:**
- Modify: `crates/app/src/main.rs` — `decode_full_res`, a new `full_res_worker`, a new `view_quality_hint`, and `RustroomApp::new`

- [ ] **Step 1: Write the failing test**

`main.rs` already has a `#[cfg(test)] mod tests` (added during the filmstrip work). Add there:

```rust
    #[test]
    fn every_demosaic_tier_has_a_cost_hint() {
        // ⚠ A table test rather than four assertions: a fifth tier added to `Demosaic::ALL`
        // must not ship with an empty hint in the Settings dialog, and nothing else would
        // catch that.
        for d in raw_decode::Demosaic::ALL {
            let hint = view_quality_hint(d);
            assert!(!hint.is_empty(), "{d:?} has no cost hint");
        }
    }

    #[test]
    fn the_default_tier_is_described_as_the_default() {
        // The hint is the only place the dialog says which one costs nothing extra.
        assert!(view_quality_hint(raw_decode::Demosaic::default()).contains("default"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p app --bin Rustroom view_quality_hint
```

Expected: **compile error**, `cannot find function view_quality_hint in this scope`.

- [ ] **Step 3: Add the hint function**

In `crates/app/src/main.rs`, directly above `fn decode_full_res`, add:

```rust
/// What choosing `tier` for the 1:1 view costs, relative to the default.
///
/// ⚠ A **multiplier, not seconds.** The export dialog says "roughly 1.2 s to 11 s per photo",
/// which are X-T4 numbers written into prose and wrong on any other body. Ratios are the
/// stable part, and they matter more here because this cost is *recurring*: the
/// full-resolution decode is eager, so a slower tier is paid on every photo you stop on,
/// not once per export.
///
/// Derived from the measured end-to-end decodes — 1.25 / 2.89 / 6.9 / 11.42 s — recorded in
/// `docs/superpowers/specs/2026-08-11-demosaic-backend-design.md`.
///
/// Lives here rather than on `raw_decode::Demosaic` because a cost measured on one camera is
/// presentation, not a property of the algorithm; `raw-decode` exposes the neutral facts.
fn view_quality_hint(tier: raw_decode::Demosaic) -> &'static str {
    match tier {
        raw_decode::Demosaic::Fastest => "about 0.4× the decode time of Fast",
        raw_decode::Demosaic::Fast => "the default",
        raw_decode::Demosaic::Better => "about 2.4× the decode time of Fast",
        raw_decode::Demosaic::Best => "about 4× the decode time of Fast",
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p app --bin Rustroom view_quality_hint
```

Expected: `test result: ok. 2 passed`.

⚠ Expect a `dead_code` warning for `view_quality_hint` until task 3 calls it. Do not run
`clippy -D warnings` before then; `cargo test` is unaffected by warnings.

- [ ] **Step 5: Give `decode_full_res` the tier**

Replace `fn decode_full_res` — signature, doc comment and body — with:

```rust
/// Decode a RAW at **full** resolution, for the true-1:1 view, at `tier`.
///
/// The tier comes from `Settings::view_demosaic`, not from any export preset — see that
/// field's doc comment for why mirroring an export preset was rejected.
fn decode_full_res(
    path: &std::path::Path,
    tier: raw_decode::Demosaic,
) -> Result<(core_image::ImageBuf, String, f32), String> {
    let opts = raw_decode::DecodeOptions {
        demosaic: tier,
        ..raw_decode::DecodeOptions::linear_scene()
    };
    let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
    let label = format!("{} {}", d.metadata.make, d.metadata.model);
    Ok((d.image, label, opts.source_scale()))
}
```

- [ ] **Step 6: Add the worker constructor**

Directly below `decode_full_res`, add:

```rust
/// Build the full-resolution decode worker for `tier`.
///
/// ⚠ The tier is **captured** here rather than passed per request. `DecodeWorker` is shared
/// with the preview decode, where demosaic is inert — libraw skips it entirely for a
/// half-size decode — so putting a tier on `DecodeWorker::request` would impose a parameter
/// on a caller that cannot act on it. Changing the setting therefore rebuilds this worker;
/// dropping the old one disconnects its channel, so a decode in flight finishes and discards
/// its own result, the same abandon-don't-abort trade the full-resolution handover makes.
fn full_res_worker(tier: raw_decode::Demosaic, ctx: &egui::Context) -> DecodeWorker {
    let ctx = ctx.clone();
    DecodeWorker::new(
        move |p| decode_full_res(p, tier),
        MAX_CONCURRENT_FULL_DECODES,
        move || ctx.request_repaint(),
    )
}
```

- [ ] **Step 7: Use it in `RustroomApp::new`**

In `RustroomApp::new`, replace the `full_decoder` field initialiser:

```rust
            full_decoder: full_res_worker(settings.view_demosaic, ctx),
```

⚠ `settings` is already read earlier in `new` (`let settings = settings_store.load();`), and
`ctx` is the function's own parameter. `Demosaic` is `Copy`, so reading `settings.view_demosaic`
here does not move `settings`, which the struct literal binds further down.

`full_res_worker` makes its own clone, so the one the old initialiser used is now unused.
Delete both lines at `main.rs:213-214`:

```rust
        // A second clone: the one above is moved into `decoder`'s closure.
        let ctx_for_full = ctx.clone();
```

Leave `let ctx2 = ctx.clone();` on the line above — the preview `decoder` still uses it.

- [ ] **Step 8: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E "^(error|warning)" ; cargo test --all
```

Expected: exactly one warning, `function view_quality_hint is never used` — it is called in
task 3. Every test passes. Any *other* warning means a step above was missed.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): the full-res decode takes its demosaic tier as a parameter"
```

---

## Task 3: The Settings control, and re-decoding on change

**Files:**
- Modify: `crates/app/src/main.rs` — `settings_modal`, a new `apply_view_demosaic`, and the export dialog's hint

- [ ] **Step 1: Add the change handler**

In `crates/app/src/main.rs`, directly above `fn settings_modal`, add:

```rust
    /// Adopt a new 1:1 view demosaic tier: persist it, rebuild the worker, and re-decode the
    /// open photo so the change is visible rather than silently deferred to the next photo.
    ///
    /// ⚠ Clearing `full_pending` is load-bearing: that sequence number belongs to the worker
    /// being dropped, and leaving it would make `pump_full_res` compare against a number no
    /// live worker will ever produce — blocking `maybe_request_full_res`, which refuses while
    /// a decode is in flight.
    fn apply_view_demosaic(&mut self, tier: raw_decode::Demosaic, ctx: &egui::Context) {
        self.settings.view_demosaic = tier;
        self.save_settings();
        self.full_decoder = full_res_worker(tier, ctx);
        self.full_pending = None;
        // RAW only. For JPEG/PNG/TIFF the buffer really is the whole file, so clearing this
        // would be a lie — and `maybe_request_full_res` refuses on its own `is_raw` check
        // anyway. The old buffer stays on screen until the new one lands, so nothing flickers.
        if let Some(s) = self.session.as_mut() {
            if catalog::is_raw(&s.path) {
                s.buffer_is_full_res = false;
            }
        }
    }
```

- [ ] **Step 2: Put the control in the dialog**

Replace the whole of `fn settings_modal` with:

```rust
    fn settings_modal(&mut self, ctx: &egui::Context) {
        let mut confirm = self.settings.confirm_preset_delete;
        let mut tier = self.settings.view_demosaic;
        let mut close = false;

        let dismissed = modal_dialog(ctx, "settings", "Settings", |ui| {
            ui.checkbox(&mut confirm, "Confirm before deleting a preset");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("1:1 view quality");
                egui::ComboBox::from_id_salt("view_demosaic_pick")
                    .selected_text(tier.label())
                    .show_ui(ui, |ui| {
                        for d in raw_decode::Demosaic::ALL {
                            ui.selectable_value(&mut tier, d, d.label());
                        }
                    });
                ui.label(egui::RichText::new(view_quality_hint(tier)).small());
            });
            // ⚠ Not decoration. Unlike the export setting, this cost is *recurring*: the
            // full-resolution decode is eager, so a slower tier is paid on every photo you
            // stop on. The dialog is the only place a user would find that out.
            ui.label(
                egui::RichText::new(
                    "RAW only, and separate from the export setting. Sets the demosaic used \
                     for the zoomed view. A slower tier costs that much extra on every photo \
                     you stop on, not just on export — the full-resolution decode starts on \
                     its own. The measured difference is small, which is why the default is \
                     Fast. Changing this re-decodes the open photo.",
                )
                .small(),
            );

            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        });
        close |= dismissed;

        if confirm != self.settings.confirm_preset_delete {
            self.settings.confirm_preset_delete = confirm;
            self.save_settings();
        }
        if tier != self.settings.view_demosaic {
            self.apply_view_demosaic(tier, ctx);
        }
        if close {
            self.modal = Modal::None;
        }
    }
```

⚠ The comparison `tier != self.settings.view_demosaic` is what stops the worker being rebuilt
and a decode being fired on **every frame the dialog is open**. `settings_modal` runs once per
frame; only a genuine change may act.

- [ ] **Step 3: Fix the export dialog's now-false hint**

In the export dialog (search for `"RAW only. Higher is slower"`), replace that `ui.label(..)`
with:

```rust
            ui.label(
                egui::RichText::new(
                    "RAW only. Higher is slower — roughly 1.2 s to 11 s per photo. \
                     Independent of the 1:1 view's own quality setting, so judging this \
                     means exporting a frame at two settings and comparing crops.",
                )
                .small(),
            );
```

⚠ The old clause "Not shown in the preview, which skips demosaic entirely" is now half false:
it holds for the fit preview before the full-resolution buffer lands, and not for the 1:1 view
once it has.

- [ ] **Step 4: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings — the `view_quality_hint` dead-code warning from task 2 is gone now that
the dialog calls it — and all tests pass.

- [ ] **Step 5: Launch the GUI**

```bash
cargo run --release -p app
```

- [ ] **Step 6: Verify the dialog**

Open Settings from the toolbar. Expect a "1:1 view quality" dropdown showing **Fast**, the
words "the default" beside it, and the warning paragraph below. Change the dropdown to
**Better** and watch the hint beside it change to "about 2.4× the decode time of Fast"
immediately, before closing the dialog.

- [ ] **Step 7: Verify the re-decode**

With a RAW photo open and settled (wait for the status line to finish "Loading full
resolution…"), open Settings and switch the tier to **Best**. Expect:

1. "Loading full resolution…" reappears in the status line within a frame or two.
2. The image on screen does **not** flicker or blank — the old buffer stays until the new one
   arrives, roughly 11 s later on a 26 Mpx RAW.
3. Zooming to 1:1 afterwards shows the image normally.

Then reopen Settings and confirm the dropdown still reads **Best**, and restart the app and
confirm it still reads **Best** — the persistence path.

- [ ] **Step 8: Verify it costs nothing at the default**

Set the tier back to **Fast**. Open a fresh photo and confirm the full-resolution decode
completes in its usual ~3 s, i.e. exactly the pre-feature behaviour.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): choose the 1:1 view's demosaic tier in Settings"
```

- [ ] **Step 10: Update `CLAUDE.md`**

Add a section in the "Current status" block, house style: what landed; that mirroring the
export preset was **considered and rejected**, with the three reasons (no mismatch at the
default, "the export preset's demosaic" is not one value, and the eager decode makes a slower
tier recurring); why the tier is captured in the closure rather than passed per request; why
the hint is a multiplier rather than seconds; and the six accepted caveats from the spec.

Then update the full-resolution section's **caveat (6)**, which currently describes this as an
open follow-up — it is now resolved, by an explicit setting rather than by mirroring. Move the
**▶ NEXT ACTION** on to **Phase 3**, the last remaining item, and add this spec and plan to the
docs list at the bottom.

- [ ] **Step 11: Commit the documentation**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-view-demosaic-setting.md
git commit -m "docs: record the 1:1 view quality setting"
```
