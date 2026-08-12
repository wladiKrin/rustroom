# Demosaic Backend Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user choose libraw's demosaic quality as an export setting carried by export presets, and stop inheriting AHD by accident — a 3.9× faster export at the new default.

**Architecture:** A `Demosaic` enum in `raw-decode` owning the single `user_qual` mapping; `DecodeOptions` gains the field and `backend_rsraw` sets the libraw param beside the ones it already sets. `ExportOptions` carries it, so the preset does too. Nothing in the pipeline, the cache or `PrefixKey` is touched — this affects only full-resolution decodes, which are never cached.

**Tech Stack:** Rust, `rsraw`/libraw, `serde`, `clap` for one CLI flag, `egui` for one dropdown.

**Spec:** `docs/superpowers/specs/2026-08-11-demosaic-backend-design.md`

---

## Build environment

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**⚠ CI is stricter than the usual local command**: `cargo clippy --all-targets --all-features`, and `rust-toolchain.toml` is unpinned `stable`. **⚠ Verify by hunting**, not by running once — this codebase has produced four flaky tests a single run never showed:

```bash
for i in $(seq 1 20); do cargo test --all 2>&1 | grep "test result: FAILED"; done
```

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/raw-decode/src/lib.rs` | modify | `Demosaic` enum + `DecodeOptions.demosaic`; the gated fixture test |
| `crates/raw-decode/src/backend_rsraw.rs` | modify | one line: `params.user_qual` |
| `crates/raw-decode/Cargo.toml` | modify | `serde` |
| `crates/app/src/batch_export.rs` | modify | `ExportOptions.demosaic` |
| `crates/app/src/main.rs` | modify | thread it into `decode_for_export`; one dropdown |
| `crates/rustroom/src/main.rs` | modify | `--demosaic` on `render` |
| `CLAUDE.md` | modify | status; next action |

---

## Task 1: `Demosaic` in `raw-decode`

**Files:**
- Modify: `crates/raw-decode/src/lib.rs`, `crates/raw-decode/src/backend_rsraw.rs`, `crates/raw-decode/Cargo.toml`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/raw-decode/src/lib.rs`:

```rust
    #[test]
    fn demosaic_maps_to_libraw_user_qual() {
        // ⚠ Hard numbers, because this mapping is the entire contract with libraw and a
        // silent off-by-one would change every export's quality with nothing failing.
        assert_eq!(Demosaic::Fastest.user_qual(), 0);
        assert_eq!(Demosaic::Fast.user_qual(), 1);
        assert_eq!(Demosaic::Better.user_qual(), 2);
        assert_eq!(Demosaic::Best.user_qual(), 3);
    }

    #[test]
    fn the_default_is_fast_not_libraws_own_default() {
        // libraw's own default is 3 (AHD), which is what we inherited by never setting it —
        // ~10.6 s of a ~11.4 s decode. `Fast` is 2.0 s and visibly better than bilinear.
        assert_eq!(Demosaic::default(), Demosaic::Fast);
        assert_eq!(DecodeOptions::linear_scene().demosaic, Demosaic::Fast);
        assert_eq!(DecodeOptions::preview().demosaic, Demosaic::Fast);
    }

    #[test]
    fn demosaic_round_trips_json() {
        for d in [
            Demosaic::Fastest,
            Demosaic::Fast,
            Demosaic::Better,
            Demosaic::Best,
        ] {
            let back: Demosaic = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
            assert_eq!(d, back);
        }
    }

    #[test]
    fn demosaic_names_are_stable_in_json() {
        // The name is part of the on-disk format for export presets and settings. Renaming a
        // variant would silently reset a saved preset to the default.
        assert_eq!(serde_json::to_string(&Demosaic::Fast).unwrap(), "\"Fast\"");
        assert_eq!(serde_json::to_string(&Demosaic::Best).unwrap(), "\"Best\"");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p raw-decode 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find type 'Demosaic' in this scope`.

- [ ] **Step 3: Add `serde` and `serde_json`**

In `crates/raw-decode/Cargo.toml`:

```toml
serde = { workspace = true }
```

and, if the crate has no `[dev-dependencies]` section yet, add one for the round-trip tests:

```toml
[dev-dependencies]
serde_json = { workspace = true }
```

⚠ `serde` in a low-level decode crate is a deliberate, small leak of an app concern. The
alternative is a mirror enum in `app` plus a **second copy of the `user_qual` mapping**, and a
mapping that exists twice is a mapping that will disagree.

- [ ] **Step 4: Add the type**

In `crates/raw-decode/src/lib.rs`, before `DecodeOptions`:

```rust
/// Which demosaic algorithm libraw should use, as a **speed/quality tier**.
///
/// ⚠ Labelled by tier rather than by algorithm on purpose. libraw's `user_qual` names
/// (bilinear/VNG/PPG/AHD) describe *Bayer* algorithms, and on **X-Trans** libraw routes every
/// setting through Markesteijn variants instead — measured: `user_qual` 3 and 4 produce
/// **byte-identical** output at identical cost on an X-T4. A label that is false on the
/// user's own camera is worse than no label; the tier is true on every sensor.
///
/// Measured full-resolution on an X-T4 `.RAF` (`unpack` a flat ~845 ms on top of each):
/// `Fastest` 400 ms, `Fast` 2,049 ms, `Better` 6,888 ms, `Best` 10,575 ms. Reproduce with
/// `cargo run --release -p raw-decode --example demosaic_probe <raw> <out-dir>`.
///
/// ⚠ **Inert for a `half_size` decode**, which is what the editing preview uses: libraw skips
/// demosaic entirely there (2×2 binning). This is an export-time decision and cannot be
/// previewed — see `a_half_size_decode_ignores_the_demosaic_setting`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Demosaic {
    /// libraw `user_qual` 0 — bilinear on Bayer.
    Fastest,
    /// libraw `user_qual` 1 — VNG on Bayer. The default: 3.9× faster end-to-end than `Best`,
    /// and visibly better than `Fastest`.
    #[default]
    Fast,
    /// libraw `user_qual` 2 — PPG on Bayer.
    Better,
    /// libraw `user_qual` 3 — AHD on Bayer. ⚠ On X-Trans this is where the scale tops out:
    /// `user_qual` 4 and above are identical to it.
    Best,
}

impl Demosaic {
    /// The libraw `user_qual` value. The single place this mapping exists.
    pub fn user_qual(self) -> i32 {
        match self {
            Demosaic::Fastest => 0,
            Demosaic::Fast => 1,
            Demosaic::Better => 2,
            Demosaic::Best => 3,
        }
    }

    /// Every tier, in order, for populating a dropdown.
    pub const ALL: [Demosaic; 4] = [
        Demosaic::Fastest,
        Demosaic::Fast,
        Demosaic::Better,
        Demosaic::Best,
    ];

    /// The label shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Demosaic::Fastest => "Fastest",
            Demosaic::Fast => "Fast",
            Demosaic::Better => "Better",
            Demosaic::Best => "Best",
        }
    }
}
```

Add the serde import at the top of the file:

```rust
use serde::{Deserialize, Serialize};
```

- [ ] **Step 5: Add the field to `DecodeOptions`**

```rust
pub struct DecodeOptions {
    // ... existing fields ...
    /// Demosaic quality. ⚠ Inert when `half_size` is true — libraw skips demosaic entirely
    /// for a half-size decode, so the editing preview cannot show this setting.
    pub demosaic: Demosaic,
}
```

and add `demosaic: Demosaic::default(),` to **all three** constructors — `linear_scene()`,
`display()` and `preview()`. ⚠ `preview()` gets it too even though it is inert: leaving the
field out of one constructor would mean two shapes of `DecodeOptions` and a reader wondering
which is authoritative.

- [ ] **Step 6: Set the libraw param**

In `crates/raw-decode/src/backend_rsraw.rs`, in the existing params block:

```rust
        let params = &mut raw.as_mut().params;
        params.output_color = 1; // sRGB (Rec.709) primaries
        params.no_auto_bright = i32::from(opts.no_auto_bright);
        // ⚠ Without this libraw uses its own default of 3 (AHD), its slowest — which is what
        // every decode silently inherited before this existed.
        params.user_qual = opts.demosaic.user_qual();
```

- [ ] **Step 7: Run**

```bash
cargo test -p raw-decode 2>&1 | tail -8
```

Expected: `test result: ok.`

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(raw-decode): choose libraw's demosaic quality instead of inheriting AHD"
```

---

## Task 2: The gated fixture test

The claim "the preview cannot show this" is the one in this design most likely to be quietly wrong. It becomes an assertion.

**Files:**
- Modify: `crates/raw-decode/src/lib.rs`

- [ ] **Step 1: Write the test**

Append inside `mod tests`:

```rust
    #[test]
    fn a_half_size_decode_ignores_the_demosaic_setting() {
        // ⚠ The claim the whole UI rests on: this is an *export-time* setting because a
        // half-size decode skips demosaic entirely (2x2 binning), so the editing preview
        // cannot show it. Asserted rather than commented.
        //
        // Gated on RAW_FIXTURE like the other real-decode tests, so `cargo test --all` stays
        // fast and green without a fixture.
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let half = |d: Demosaic| {
            decode_raw(
                &path,
                &DecodeOptions {
                    demosaic: d,
                    ..DecodeOptions::preview()
                },
            )
            .expect("decode")
            .image
        };
        assert_eq!(
            half(Demosaic::Fastest).data(),
            half(Demosaic::Best).data(),
            "a half-size decode must be unaffected by the demosaic setting"
        );
    }

    #[test]
    fn a_full_size_decode_honours_the_demosaic_setting() {
        // The other half: if this failed, `user_qual` was never reaching libraw and the whole
        // feature would be an inert dropdown.
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let full = |d: Demosaic| {
            decode_raw(
                &path,
                &DecodeOptions {
                    demosaic: d,
                    ..DecodeOptions::linear_scene()
                },
            )
            .expect("decode")
            .image
        };
        assert_ne!(
            full(Demosaic::Fastest).data(),
            full(Demosaic::Best).data(),
            "the demosaic setting never reached libraw"
        );
    }
```

- [ ] **Step 2: Run it against the real file**

```bash
RAW_FIXTURE=$HOME/Desktop/fuji/DSCF6509.RAF cargo test --release -p raw-decode 2>&1 | tail -8
```

Expected: both pass. **⚠ `--release`** — these decode a 26 Mpx RAW four times and a debug build takes minutes.

If `a_full_size_decode_honours_the_demosaic_setting` fails, `params.user_qual` is not reaching libraw — check it is set *before* `process()` and after `unpack()`.

- [ ] **Step 3: Confirm the suite still skips cleanly without a fixture**

```bash
cargo test -p raw-decode 2>&1 | grep -E "^test result|skipping"
```

Expected: green, with the skip messages.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(raw-decode): pin that only a full-size decode honours the demosaic setting"
```

---

## Task 3: Carry it in `ExportOptions` and the preset

**Files:**
- Modify: `crates/app/src/batch_export.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/app/src/settings.rs`:

```rust
    #[test]
    fn settings_round_trip_carries_the_demosaic_choice() {
        let f = TempFile::new("demosaic_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            export: ExportOptions {
                demosaic: raw_decode::Demosaic::Best,
                ..ExportOptions::default()
            },
            ..Settings::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().export.demosaic, raw_decode::Demosaic::Best);
    }

    #[test]
    fn a_settings_file_from_before_the_demosaic_choice_still_loads() {
        // The same forgiving-load property that `ExportOptions::default()` exists for.
        let f = TempFile::new("demosaic_old");
        std::fs::write(
            &f.0,
            r#"{"export":{"dest_dir":"/tmp/x","quality":80,"max_edge":null}}"#,
        )
        .unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert_eq!(loaded.export.quality, 80, "the old fields must survive");
        assert_eq!(loaded.export.demosaic, raw_decode::Demosaic::default());
    }
```

and inside `mod tests` in `crates/app/src/export_presets.rs`:

```rust
    #[test]
    fn a_preset_carries_the_demosaic_choice() {
        let d = TempDir::new("demosaic");
        let store = d.store();
        let p = ExportPreset {
            name: "Archive".into(),
            options: ExportOptions {
                demosaic: raw_decode::Demosaic::Best,
                ..ExportOptions::default()
            },
        };
        store.save(&p).unwrap();
        assert_eq!(store.list()[0].options.demosaic, raw_decode::Demosaic::Best);
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p app --lib 2>&1 | grep -E "^error" | head -3
```

Expected: `struct 'ExportOptions' has no field named 'demosaic'`.

- [ ] **Step 3: Add the field**

In `crates/app/src/batch_export.rs`:

```rust
pub struct ExportOptions {
    pub dest_dir: PathBuf,
    /// JPEG quality, 1..=100.
    pub quality: u8,
    /// Fit within this many pixels on the long edge. `None` = full resolution.
    pub max_edge: Option<usize>,
    /// Demosaic quality for RAW sources.
    ///
    /// ⚠ An **export** setting rather than a develop edit, and by structure rather than by
    /// preference: the editing preview decodes `half_size`, which skips demosaic entirely, so
    /// this can only ever affect a full-resolution decode.
    pub demosaic: raw_decode::Demosaic,
}
```

and in its `Default`:

```rust
            demosaic: raw_decode::Demosaic::default(),
```

⚠ Several test fixtures construct `ExportOptions` exhaustively and will stop compiling. That is the intended behaviour — add `..ExportOptions::default()` to each, or the field where the test is about it.

- [ ] **Step 4: Run**

```bash
cargo test -p app --lib 2>&1 | grep -E "^test result|^error" | head -4
```

Expected: `test result: ok.`

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(app): carry the demosaic choice in ExportOptions and export presets"
```

---

## Task 4: Use it, and expose it

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Thread it into the export decode**

`decode_for_export` currently takes only a path. Give it the setting:

```rust
fn decode_for_export(
    path: &std::path::Path,
    demosaic: raw_decode::Demosaic,
) -> Result<core_image::ImageBuf, String> {
    if catalog::is_raw(path) {
        raw_decode::decode_raw(
            path,
            &raw_decode::DecodeOptions {
                demosaic,
                ..raw_decode::DecodeOptions::linear_scene()
            },
        )
        .map(|d| d.image)
        .map_err(|e| e.to_string())
    } else {
        // Not a RAW: there is no demosaic step at all.
        image_decode::decode(path)
            .map(|d| d.image)
            .map_err(|e| e.to_string())
    }
}
```

In `start_export`, capture it into the closure (it already builds one):

```rust
        let demosaic = opts.demosaic;
        // ...
            move |p: &std::path::Path| decode_for_export(p, demosaic),
```

⚠ Capture from `opts` — the committed options — not from `self.export_draft`, so a cancelled
edit cannot leak into a running export.

- [ ] **Step 2: Add the dropdown**

In `export_modal`, after the long-edge control and before the separator:

```rust
            ui.horizontal(|ui| {
                ui.label("Demosaic");
                egui::ComboBox::from_id_salt("demosaic_pick")
                    .selected_text(opts.demosaic.label())
                    .show_ui(ui, |ui| {
                        for d in raw_decode::Demosaic::ALL {
                            ui.selectable_value(&mut opts.demosaic, d, d.label());
                        }
                    });
            });
            ui.label(
                egui::RichText::new(
                    "RAW only. Higher is slower — roughly 1.2 s to 11 s per photo. \
                     Not shown in the preview, which skips demosaic entirely.",
                )
                .small(),
            );
```

⚠ The hint is not decoration. This is the one export control whose effect **cannot be
previewed**, and saying so in the dialog is the only place a user would find that out.

- [ ] **Step 3: Verify**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE "^(error|warning: )"
cargo test --all 2>&1 | grep -E "^test result"
```

Expected: clippy `0`, all green.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(app): demosaic quality dropdown in the export dialog"
```

---

## Task 5: The CLI flag

`rustroom render` silently inherits AHD too.

**Files:**
- Modify: `crates/rustroom/src/main.rs`

- [ ] **Step 1: Add the argument**

On the `Render` variant, beside `--quality`:

```rust
        /// Demosaic quality for RAW input: fastest, fast, better, best.
        #[arg(long, default_value = "fast")]
        demosaic: String,
```

⚠ `render` takes its arguments as parameters, so this means **three** edits, not one: the
`Render` variant above, the `render(...)` call in the `match` that dispatches it, and the
`fn render(...)` signature. Then parse inside `render`:

```rust
    let demosaic = match demosaic.to_ascii_lowercase().as_str() {
        "fastest" => raw_decode::Demosaic::Fastest,
        "fast" => raw_decode::Demosaic::Fast,
        "better" => raw_decode::Demosaic::Better,
        "best" => raw_decode::Demosaic::Best,
        other => anyhow::bail!("unknown demosaic {other:?} (fastest|fast|better|best)"),
    };
    let decoded = raw_decode::decode_raw(
        input,
        &raw_decode::DecodeOptions {
            demosaic,
            ..raw_decode::DecodeOptions::linear_scene()
        },
    )
```

⚠ A plain `String` plus a match rather than a `clap::ValueEnum` derive, to avoid adding a
`clap` dependency to `raw-decode` just for a CLI concern. The error message lists the valid
values, which is what a `ValueEnum` would have bought.

- [ ] **Step 2: Verify it runs**

```bash
cargo run --release -p rustroom -- render ~/Desktop/fuji/DSCF6509.RAF /tmp/fastest.jpg --demosaic fastest
cargo run --release -p rustroom -- render ~/Desktop/fuji/DSCF6509.RAF /tmp/best.jpg --demosaic best
cargo run --release -p rustroom -- render ~/Desktop/fuji/DSCF6509.RAF /tmp/bad.jpg --demosaic nonsense
```

Expected: the first two succeed with a visible time difference (~1 s vs ~11 s); the third fails with the list of valid values.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(cli): --demosaic on rustroom render"
```

---

## Task 6: Validate, measure, document

- [ ] **Step 1: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Tell the user what to check:

1. The dropdown appears in the export dialog, defaults to **Fast**, and the hint says it is not previewable.
2. Export one photo at **Fast** and one at **Best** — the second should take visibly longer (~2.9 s vs ~11.4 s per photo).
3. Save a preset with **Best**, switch the dropdown to Fastest, re-pick the preset — it should come back as Best.

- [ ] **Step 2: Compare the two exports at 1:1**

⚠ This is the validation the design's central trade rests on, and it cannot be done in the
preview. Open both exports and compare a detailed region at 100%. The expectation from the
probe is that `Best` has slightly crisper fine detail and `Fast` is marginally softer — a
subtle difference, not a dramatic one.

**If the difference turns out to be more visible than the probe suggested, say so and revisit
the default** rather than defending the choice. The whole justification for `Fast` is that the
gap is small relative to 3.9× the time.

- [ ] **Step 3: Update `CLAUDE.md`**

- Add a "**Demosaic backend selection done**" section: the measured table (process **and**
  end-to-end, since the process-only ratios overstate the win); the X-Trans finding that
  `user_qual ≥ 3` collapses and DCB is byte-identical to AHD; why the labels are tiers rather
  than algorithm names; why it lives in `ExportOptions` structurally; and the caveat that it
  cannot be previewed.
- Record the **default change** and that it alters output relative to earlier exports.
- Update the crate-layout line for `raw-decode`.
- Set the **next action** to **export to PNG/TIFF**, carrying forward the two things already
  identified: **the ICC question** (16-bit TIFF untagged is a real correctness gap, and
  plausibly where lcms2 stops being deferrable) and **`dest_for` hardcoding `.jpg`**, where
  the collision logic keys on the produced filename.
- Add the spec and plan paths to the docs list.

- [ ] **Step 4: Final check and commit**

```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings
cargo test --all 2>&1 | tail -5
for i in $(seq 1 20); do cargo test --all 2>&1 | grep "test result: FAILED"; done; echo done
git add -A
git commit -m "docs: record demosaic backend selection; next action is PNG/TIFF export"
```

**Do not push.** The user pushes to `github.com/wladiKrin/rustroom` themselves.

---

## Definition of done

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all` green
- [ ] 20 consecutive full-suite runs with no failures
- [ ] `RAW_FIXTURE=... cargo test --release -p raw-decode` green, including **both** gated tests — half-size unaffected, full-size affected
- [ ] An older settings file and an older export preset both still load, defaulting the new field
- [ ] Two real exports compared at 1:1, and the default revisited if the gap is larger than the probe suggested
- [ ] `CLAUDE.md` records the measurement, the X-Trans finding, the default change, and points at PNG/TIFF
