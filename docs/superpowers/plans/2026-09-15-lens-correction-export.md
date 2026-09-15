# M8.9 plan 2 — lens correction reaches export, and crosses the seam

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan
> task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **Conventions that are not negotiable, and they override the harness's own reminder:** commit
> messages carry **no `Co-Authored-By` trailer**; branch per feature; **never push**. Grep anything
> you write for that trailer before committing it.
>
> ⚠ Gate every commit on `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
> warnings` and `cargo test --workspace`, **reading each exit code from its own command** — never
> through a pipe, and joined with `&&`, never `;`.
>
> ⚠ `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"` before anything that builds `raw-decode`.

**Goal:** the GUI's export applies lens correction, and offers every prefix step to the GPU seam.

**Spec:** `docs/superpowers/specs/2026-09-15-lens-correction-gpu-design.md` — read **§2.1** first.

---

## ⚠⚠ Read this before Task 1: the milestone's headline is a bug fix

`export_one` builds through `pipeline::build_pipeline_spec`, which hardcodes `lens: None`, and
`decode_for_export` throws the metadata away keeping only the white-balance baseline. So **the GUI's
Export dialog renders every RAW with no lens correction at all**, while the preview and
`rustroom render` both apply it.

Verified end to end through the shipped CLI on `DSCF6296.RAF`, XF16-80 at 19.2 mm f/8 — a *mild*
case, both corrections being worst at the wide end and wide open:

> corrected against uncorrected: **worst 187 of 255, mean 6.95, and 33.5% of samples differing by
> more than 4 levels.**

⚠ `lens_diff`'s radial bands read ~1.00 at this aperture, so most of that is **geometry**, which
that probe structurally cannot see. The recorded worst displacement on this lens is **50.59 px**.

**Two consequences, stated rather than discovered.** Every exported RAW changes — that is the fix,
the export is supposed to match the preview. And every RAW export gets ~700 ms/photo slower, which
is what the GPU half of this milestone pays back.

## File structure

- **Modify** `crates/app/src/decode.rs` — `decode_for_export` returns the lens shot.
- **Modify** `crates/app/src/batch_export.rs` — `ExportSource`, `ExportRender`, the prefix loop.
- **Modify** `crates/app/src/export_flow.rs` — pass `gpu_tail` from `Settings`.
- **Modify** `crates/app/src/modals/settings.rs` — the hint.

⚠ `crates/app/src/batch_export.rs` is **~1600 lines, already past the ~600 marker.** It grows here by
roughly forty production lines plus tests. Reported rather than silently added to.

---

### Task 1: the export decode carries the lens

**Files:**
- Modify: `crates/app/src/decode.rs`, `crates/app/src/batch_export.rs`

- [ ] **Step 1: name what a decode hands over**

In `batch_export.rs`, above `ExportWorker`:

```rust
/// What a decode hands the exporter.
///
/// ⚠⚠ **A struct rather than a growing tuple, and the third field is why it exists.** Until
/// M8.9 this was `(ImageBuf, Illuminant)` and the lens shot was simply **discarded** — which is
/// how the GUI's export came to render every RAW with no lens correction while the preview and
/// the CLI both applied it. A named field is harder to drop than a tuple element.
pub struct ExportSource {
    pub image: ImageBuf,
    /// The white-balance baseline the decode left the picture at. Drop it and every exported RAW
    /// gets D65's gains where the preview used the camera's own.
    pub baseline: color::wb::Illuminant,
    /// The matched lens profile, `None` for a JPEG or an unlisted lens.
    pub lens: Option<lens_profile::LensShot>,
}
```

- [ ] **Step 2: return it from the decode**

In `decode.rs`, change `decode_for_export`'s return type to
`Result<crate::batch_export::ExportSource, String>` and fill in all three fields. The RAW arm has
the metadata in hand:

```rust
        .map(|d| crate::batch_export::ExportSource {
            lens: resolve_lens(&d.metadata).shot,
            baseline: d.metadata.as_shot.unwrap_or_default(),
            image: d.image,
        })
```

⚠ `resolve_lens` is already `pub(crate)` in that module and already runs on a decode thread — it is
what the preview's decode uses, so both paths match one lens the same way.

The non-RAW arm carries `lens: None`, which is correct rather than a fallback: a JPEG has no lens
metadata through `image_decode`.

- [ ] **Step 3: update `spawn`, `run` and the one caller**

`spawn`'s `decode` bound becomes `impl Fn(&Path) -> Result<ExportSource, String> + Send + 'static`,
and `run`'s the same. The compiler names every test fixture that builds one.

- [ ] **Step 4: build**

```bash
cargo clippy -p app --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Expected: 0, after the test fixtures are updated.

- [ ] **Step 5: gate and commit**

```bash
cargo fmt --all && cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add -A
git commit -m "refactor(export): a decode hands over its lens profile rather than discarding it"
```

---

### Task 2: the export applies lens correction

**Files:**
- Modify: `crates/app/src/batch_export.rs`

- [ ] **Step 1: write the failing test**

⚠⚠ **This is the carrier, and it must go through `export_one` rather than through `pipeline`.**
Every `pipeline` test already passes: the stage works, and nothing proved it was ever *reached* from
export. That gap is exactly how creative LUTs (W5) shipped fully tested and 100% non-functional.

```rust
    /// ⚠⚠ **THE CARRIER FOR THE §2.1 FIX.** `build_pipeline_spec` hardcodes `lens: None`, so for
    /// the life of the export dialog every RAW was written out with no lens correction while the
    /// preview and the CLI both applied it — measured at worst 187 of 255 levels on an ordinary
    /// frame. Nothing in `pipeline` could see it, because the stage itself was never broken.
    #[test]
    fn a_lens_profile_reaches_the_exported_pixels() {
        let dir = TempDir::new("export-lens");
        let dest = dir.path().join("out.png");
        let src = grid_chart(120, 90);
        let mut edits = Edits::default();
        edits.lens_correction = Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        });
        let opts = ExportOptions {
            format: ExportFormat::Png8,
            ..ExportOptions::default()
        };
        let render = ExportRender {
            resampler: pipeline::Resampler::CatmullRom,
            gpu_tail: false,
        };

        let without = {
            export_one(
                ExportSource {
                    image: src.clone(),
                    baseline: Default::default(),
                    lens: None,
                },
                &edits,
                &dest,
                &opts,
                render,
                &export::FileMetadata::default(),
            )
            .unwrap();
            std::fs::read(&dest).unwrap()
        };
        let with = {
            export_one(
                ExportSource {
                    image: src,
                    baseline: Default::default(),
                    lens: Some(lens_fixture()),
                    },
                &edits,
                &dest,
                &opts,
                render,
                &export::FileMetadata::default(),
            )
            .unwrap();
            std::fs::read(&dest).unwrap()
        };
        assert_ne!(
            without, with,
            "a matched lens profile must change the exported file"
        );
    }
```

⚠ `lens_fixture()` and `grid_chart()` are the synthetic profile and the hard grid from plan 1's
Task 3 tests. Copy the builders in — `batch_export` is in a different crate from `gpu-pipeline`, so
sharing them would mean a test-only export from `pipeline`, which is more machinery than two eight-
line helpers are worth. Say that you copied them.

- [ ] **Step 2: run it to watch it fail**

```bash
cargo test -p app a_lens_profile_reaches_the_exported_pixels
```

Expected: FAIL — the two files are identical, because `build_pipeline_spec` passes `lens: None`.
⚠ **Confirm it fails for that reason** and not because `ExportRender` does not exist yet; introduce
the type in the same step if so.

- [ ] **Step 3: run the prefix as steps**

Replace `export_one`'s single `build_pipeline_spec(..).run(scaled)` with the shape `render_cached`
and `rustroom render` both already use — the prefix as **steps**, then the tail:

```rust
/// Render one photo and write it out.
///
/// ⚠⚠ **The prefix runs as STEPS rather than as one pipeline, and that is what lets each of them
/// be offered to the GPU seam** — the same shape `render_cached` uses, and for the same reason:
/// `accelerate_prefix_stage` substitutes a stage by name and needs to see them one at a time.
/// Building the tail through `build_pipeline_split_in_region_with_brush` and *discarding* its
/// prefix is deliberate: both halves then come from one builder, so they cannot drift about what
/// belongs in which.
pub fn export_one(
    source: ExportSource,
    edits: &Edits,
    dest: &Path,
    opts: &ExportOptions,
    // ⚠ Two parameters rather than two `ExportOptions` fields — see `ExportRender`.
    render: ExportRender,
    meta: &export::FileMetadata,
) -> Result<(), String> {
    let source_long = source.image.width().max(source.image.height());
    let scaled = match opts.max_edge {
        Some(max) => source.image.downscale_to(max),
        None => source.image,
    };
    let scale = scale_for(source_long, opts.max_edge);
    // ⚠⚠ The export must use the SAME airlight definition as the preview — and now the same lens,
    // for the same reason: `prefix_airlight` replays the prefix, so a replay without the lens would
    // estimate from a differently-shaped photograph.
    let airlight =
        pipeline::prefix_airlight(edits, &scaled, render.resampler, source.lens.clone());
    let spec = pipeline::RenderSpec {
        space: opts.space,
        resampler: render.resampler,
        baseline: source.baseline,
        airlight: Some(airlight),
    };
    let steps =
        pipeline::build_prefix_steps(edits, scale, source.lens.clone(), render.resampler, Some(airlight));
    let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        edits,
        scale,
        None,
        None,
        spec,
        source.lens.clone(),
        pipeline::ProfileRendering::default(),
    );

    let mut img = scaled;
    for step in &steps {
        img = step.stage.apply(img);
    }
    let mut rendered = tail.run(img);
    // ... output_sharpen / watermark / save / metadata unchanged
}
```

⚠⚠ **Check each of those signatures against `crates/pipeline/src/lib.rs` before writing this.**
`build_prefix_steps`, `build_pipeline_split_in_region_with_brush` and `ProfileRendering` all take
exactly what `render_engine.rs` passes them; mirror that call site rather than this sketch.

⚠ **`ProfileRendering::default()` is a real limitation and is inherited, not introduced**:
`build_pipeline_spec` already passed it, so the camera rendering's profile curve and baseline
exposure do not reach an export today either. **Do not fix it here** — it is a second bug with its
own measurement, and mixing it in would make this change's diff unattributable. Record it in
`docs/BACKLOG.md` instead.

And add the params struct:

```rust
/// What the *render* needs that `ExportOptions` deliberately does not carry.
///
/// ⚠⚠ **Neither field may become an `ExportOptions` field, and the reason is written down in
/// `spawn`'s own doc comment**: in `ExportOptions` they would be saved into every export preset, so
/// a preset could disagree with the preview about how the photograph is rendered. Two facts that
/// can disagree.
///
/// ⚠ `gpu_tail` is named for the *tail* and now governs three things — the tail, the preview's
/// prefix seam, and this. That is a known naming hazard, accepted (user's call, 2026-09-15), and
/// the Settings hint is reworded to describe the rule rather than enumerate what it reaches.
#[derive(Clone, Copy)]
pub struct ExportRender {
    pub resampler: pipeline::Resampler,
    pub gpu_tail: bool,
}
```

- [ ] **Step 4: run the test**

```bash
cargo test -p app a_lens_profile_reaches_the_exported_pixels
```

Expected: PASS.

- [ ] **Step 5: thread `gpu_tail` from Settings**

In `export_flow.rs`, pass `self.settings.gpu_tail` into `ExportWorker::spawn` beside
`self.settings.resampler`, with a comment that says the same thing the resampler's does: it comes
from `Settings`, never from `opts`.

- [ ] **Step 6: gate and commit**

```bash
cargo fmt --all && cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
```

⚠⚠ **The 63 goldens must not move.** They render the CPU pipeline through `build_pipeline`, which
this does not touch. A moved golden here means the tail was rebuilt differently, not that lens
correction arrived. Check `git status` over `crates/pipeline/tests` — an unmodified tree plus a green
run is what establishes that, never a green run alone.

```bash
git add -A
git commit -m "fix(export): the GUI's export applies lens correction, which it never has"
```

---

### Task 3: export crosses the GPU seam

**Files:**
- Modify: `crates/app/src/batch_export.rs`

- [ ] **Step 1: write the failing test**

⚠⚠ **A test asserting only "the picture is the same" would pass with the whole seam deleted**, the
two paths agreeing by design. So the observable is the **count of accelerated steps**, exactly as
`render_cached` returns `prefix_stages` and for the same reason.

Have `export_one` return, or record into a `#[cfg(test)]` thread-local counter, how many prefix steps
it accelerated — a counter rather than a flag, because an observable describing the current state
cannot see its own absence (M8.3f's finding). Then:

```rust
    /// ⚠⚠ **THE CARRIER.** An accelerated export and a CPU export produce the same photograph by
    /// design, so every correctness test in the workspace passes with the seam deleted — the W5
    /// failure mode. This asserts the seam **fired**.
    #[test]
    fn an_accelerated_export_really_uses_the_seam() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        // dehaze alone, which is already ported and needs no lens profile
        let mut edits = Edits::default();
        edits.dehaze = 60.0;
        assert_eq!(accelerated_steps(&edits, false), 0, "with the setting off, nothing crosses");
        assert!(accelerated_steps(&edits, true) >= 1, "with it on, dehaze must cross");
    }
```

⚠ The fixture uses **dehaze**, not lens correction, deliberately: dehaze is already ported and needs
no profile, so this test says something about the **wiring** even if plan 1 has not landed. That is
what makes plan 2 worth landing on its own merits.

- [ ] **Step 2: offer each step to the seam**

```rust
    let mut img = scaled;
    for step in &steps {
        // ⚠⚠ **Per step and at apply time, exactly as `render_cached` does it** — never over the
        // whole list up front. `dehaze::airlight` is estimated from the image the stage is HANDED,
        // and geometry crops, so substituting up front would estimate it from the wrong pixels: a
        // slightly different airlight is a different photograph, not an error.
        let accelerated = if render.gpu_tail {
            gpu_pipeline::shared().and_then(|ctx| {
                gpu_pipeline::accelerate_prefix_stage(
                    step.stage.name(),
                    edits,
                    scale,
                    &img,
                    source.lens.as_ref(),
                    render.resampler,
                    ctx,
                )
            })
        } else {
            None
        };
        img = match accelerated {
            Some(gpu) => gpu.apply(img),
            None => step.stage.apply(img),
        };
    }
```

⚠⚠ **`if` rather than `match (render.gpu_tail, shared())`** — a tuple scrutinee evaluates both
elements, so with the setting off `shared()` would still acquire an adapter. That was a real bug when
`gpu_tail` first landed, and it cost ~42 s per debug test run.

⚠ Export decodes are **never cached**, so there is no `PrefixStepKey` question here at all: the
seam returns a **stage**, never a step, and nothing stores the result.

- [ ] **Step 3: run the tests**

```bash
cargo test --release -p app export
```

- [ ] **Step 4: sabotage — force `render.gpu_tail` to `false` inside the loop**

Expected: `an_accelerated_export_really_uses_the_seam` fails on its second assertion and nothing else
does. Restore from a `cp` copy named per file — **never `git checkout`**, which has destroyed
uncommitted work in this repo once already.

- [ ] **Step 5: gate and commit**

```bash
cargo fmt --all && cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add -A
git commit -m "feat(export): every prefix step is offered to the GPU seam"
```

---

### Task 4: the Settings hint

**Files:**
- Modify: `crates/app/src/modals/settings.rs`

⚠⚠ **This hint has been corrected for exactly this twice already**, and its own comment says why:
*a hint that lists a growing set is a hint that goes silently wrong.* It currently says "the
per-pixel develop stages" — and lens correction is a **resampler**, not a per-pixel stage — and says
nothing about export at all.

- [ ] **Step 1: reword to describe the rule**

Keep the comment above it and extend it with this milestone's instance. The text should say, without
enumerating anything: it runs develop work on the graphics card **where it pays**, in the preview and
in an export alike; the CPU is the reference and the two agree to about a hundredth of a level, so
the picture should not change; a stage is moved across only when the work saved is worth the
transfer; the first render after switching it on spends about a sixth of a second compiling shaders.

⚠ **Anything added to the seam or to `REPLACEABLE` must not require editing this string**, which is
what "describe the rule" means and is the property to preserve.

- [ ] **Step 2: gate and commit**

```bash
cargo fmt --all && cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add -A
git commit -m "docs(settings): the GPU hint describes the rule, export included"
```

---

### Task 5: measure the real thing

⚠⚠ **Composing component measurements has missed in this crate three times.** The spec projects
nothing; this task is why.

- [ ] **Step 1: export one real photo four ways**

On a `.RAF` with a matched profile, at full resolution, three runs each, reporting a **band** and the
machine's load:

| | |
|---|---|
| before this milestone (no lens correction at all) | the old behaviour, for scale only |
| lens correction on, `gpu_tail` off | the honest new cost |
| lens correction on, `gpu_tail` on | what the port buys |
| *dehaze or NR active, `gpu_tail` on* | the free lift plan 2 gives on its own |

Use the export dialog with `RUSTROOM_DEBUG_RENDER=1`, or extend `probe_lens` to drive `export_one`
directly — whichever measures **the shipped path**, which is the whole point of this task.

- [ ] **Step 2: write the result into `CLAUDE.md`**

A band, never a point. State plainly that the middle row is **slower than before** and why that is
the fix rather than a regression.

- [ ] **Step 3: commit**

```bash
git add -A
git commit -m "docs: M8.9 measured through the shipped export path"
```

- [ ] **Step 4: launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

What to look at, in order:

1. ⚠⚠ **Export a photo with a matched lens profile and compare it against the canvas.** Before today
   they disagreed by up to 187 of 255 levels; they should now agree. **That is the milestone's own
   acceptance check**, and no test can make it.
2. **Settings → Rendering → "Use the GPU"** on, and export the same photo again. ⚠⚠ **The picture
   must not change** — and it should be visibly quicker.
3. ⚠ **A photo with NO lens profile** (a JPEG, or an unlisted lens), which must export exactly as it
   did before. That is the half most easily broken by threading a profile through.
4. ⚠ **A batch of several photos**, which is where the sequential cost is felt and where the port is
   aimed.
