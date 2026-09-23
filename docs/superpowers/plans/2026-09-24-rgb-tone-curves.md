# Per-channel RGB tone curves — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Red, Green and Blue curves beside the composite, on one widget, with the render and the GPU agreeing.

**Architecture:** `ToneCurveStage::new(points)` keeps its ten callers and its cheap `map_samples` path; a `with_channels` builder adds three optional LUTs and switches `apply` to `map_pixels` only when one is set. The GPU uploads the CPU's four tables as one concatenated buffer. The widget gains a channel selector held in session state, not in `Edits`.

**Spec:** `docs/superpowers/specs/2026-09-24-rgb-tone-curves-design.md`. Read it first.

---

## Conventions

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By`. Never push.** Branch `rgb-tone-curves`.
- ⚠⚠ `cargo test --workspace <filter>`, **never `-p`**. A filter matching nothing **exits 0 with zero tests** — check the passed count.
- ⚠⚠ **Make a test prove its sample before asserting about it** (2026-09-24's lesson): a query that finds nothing must fail, not pass.
- ⚠ **A fixture that makes two different things equal cannot see a bug about telling them apart** — so **every channel fixture uses a DIFFERENT curve per channel**, never the same one three times.
- ⚠⚠ After any scripted edit: `git diff <commit> -- <file> | grep '^-' | grep '///'` must be empty unless intended.

---

### Task 1: The stage carries three more curves

**Files:** `crates/pipeline/src/stages/tone_curve.rs`

- [ ] **Step 1: Failing tests**

```rust
    /// ⚠⚠ **A different curve per channel.** Three copies of one curve would pass against an
    /// implementation that ignored the channel index entirely.
    #[test]
    fn a_per_channel_curve_moves_only_its_own_channel() {
        let lift = [[0.0, 0.2], [1.0, 1.0]];
        let drop = [[0.0, 0.0], [1.0, 0.8]];
        let stage = ToneCurveStage::new(&[]).with_channels(&lift, &[], &drop);
        let out = stage.apply(flat(0.5));
        let px = [out.data()[0], out.data()[1], out.data()[2]];
        assert!(px[0] > 0.5, "red was lifted: {px:?}");
        assert!((px[1] - 0.5).abs() < 1e-4, "green untouched: {px:?}");
        assert!(px[2] < 0.5, "blue was dropped: {px:?}");
    }

    /// ⚠ The order the spec chose: composite first, then the channel.
    #[test]
    fn the_composite_runs_before_the_channel_curve() {
        let composite = [[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]];
        let red = [[0.0, 0.5], [1.0, 1.0]];
        let a = ToneCurveStage::new(&composite).with_channels(&red, &[], &[]);
        let got = a.apply(flat(0.5)).data()[0];
        // composite(0.5) ≈ 0.25, then red lifts it. The other order would give red(0.5) ≈ 0.75
        // then composite ≈ 0.6 — measurably different.
        let composite_then_red = ToneCurveStage::new(&red).sample(
            ToneCurveStage::new(&composite).sample(0.5),
        );
        assert!(
            (got - composite_then_red).abs() < 1e-4,
            "expected composite-then-channel: got {got}, wanted {composite_then_red}"
        );
    }

    /// ⚠ With no channel curve the cheap path must be kept — and identical to today's output.
    #[test]
    fn with_no_channel_curves_the_result_is_unchanged() {
        let c = [[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let plain = ToneCurveStage::new(&c).apply(flat(0.4));
        let built = ToneCurveStage::new(&c)
            .with_channels(&[], &[], &[])
            .apply(flat(0.4));
        assert_eq!(plain.data(), built.data());
    }

    /// ⚠⚠ The clamp is inherited, not re-decided: a scene-referred highlight above 1.0 is still
    /// pulled to the curve's top entry, because that is what `sample` does and what the GPU mirrors.
    #[test]
    fn a_channel_curve_inherits_the_input_clamp() {
        let red = [[0.0, 0.0], [1.0, 0.5]];
        let stage = ToneCurveStage::new(&[]).with_channels(&red, &[], &[]);
        let out = stage.apply(flat(4.0));
        assert!((out.data()[0] - 0.5).abs() < 1e-3, "clamped to the top entry");
    }
```

⚠ Add a `flat(v)` helper returning a 1×1 `ImageBuf` with all three samples at `v`, or reuse whatever
the file's existing tests use — **read them first**.

- [ ] **Step 2: Run, expect failure to compile.**

- [ ] **Step 3: Implement**

```rust
pub struct ToneCurveStage {
    lut: Vec<f32>,
    /// Per-channel tables, composed **after** `lut`.
    ///
    /// ⚠⚠ `None` when every channel curve is empty, which keeps `apply` on the cheap
    /// `map_samples` path — the common case by far, and byte-identical to before this existed.
    channels: Option<[Vec<f32>; 3]>,
}
```

with `new` unchanged (`channels: None` — ⚠ **its ten callers must not move**), and:

```rust
    /// Add per-channel curves, composed after the composite.
    ///
    /// ⚠ Empty points for a channel mean identity, the rule the composite already follows; all three
    /// empty leaves the stage exactly as `new` built it.
    pub fn with_channels(mut self, red: &[[f32; 2]], green: &[[f32; 2]], blue: &[[f32; 2]]) -> Self { … }

    /// The four tables, composite first — what `gpu-pipeline` uploads.
    ///
    /// ⚠⚠ Uploaded from here rather than rebuilt there, the rule this stage's `lut` already states:
    /// two constructions of one curve is how a preview stops being evidence about the render.
    pub fn luts(&self) -> [&[f32]; 4] { … }
```

and `apply`:

```rust
    fn apply(&self, image: ImageBuf) -> ImageBuf {
        match &self.channels {
            // ⚠ The common path, untouched.
            None => image.map_samples(|s| self.sample(s)),
            Some(ch) => image.map_pixels(|p| {
                std::array::from_fn(|c| sample_lut(&ch[c], self.sample(p[c])))
            }),
        }
    }
```

⚠ Factor `sample` and `sample_lut` so **one** interpolation is used by both, or the composite and the
channels could round differently.

- [ ] **Step 4: Run the stage's tests. Step 5: Commit.**

---

### Task 2: The model, and the stage reaching the pipeline

**Files:** `crates/edit-model/src/lib.rs`, `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Failing tests** in `edit-model`:

```rust
    /// ⚠⚠ The three curves must travel with `Tone` and with nothing else.
    #[test]
    fn the_channel_curves_transfer_with_tone_only() {
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::only(1)); // 1 == "Tone"
        assert_eq!(got.tone_curve_red, src.tone_curve_red);
        assert_eq!(got.tone_curve_blue, src.tone_curve_blue);
        for g in 0..EDIT_GROUP_NAMES.len() {
            if g == 1 { continue; }
            let mut other = Edits::default();
            other.merge_from(&src, EditGroups::only(g));
            assert!(
                other.tone_curve_red.points.is_empty(),
                "group {} carried a channel curve it should not", EDIT_GROUP_NAMES[g]
            );
        }
    }

    /// ⚠⚠ A sidecar written before these fields existed must load as identity — the container's
    /// `#[serde(default)]` doing its job, with no `version` bump involved.
    #[test]
    fn a_sidecar_without_the_channel_curves_loads_as_identity() {
        let json = r#"{"version":7,"exposure":0.5}"#;
        let e: Edits = serde_json::from_str(json).expect("must parse");
        assert_eq!(e.exposure, 0.5);
        assert!(e.tone_curve_red.points.is_empty());
        assert!(e.tone_curve_green.points.is_empty());
        assert!(e.tone_curve_blue.points.is_empty());
    }
```

⚠⚠ **`all_nonneutral()` must set all three to DIFFERENT curves.** If it leaves them default, every
"every group transfers" test in the file passes while ignoring them — the enumerating-guard failure this
project keeps paying for.

- [ ] **Step 2: Add the fields**, in `Edits` beside `tone_curve`, with a doc saying they compose after
the composite and are `Tone`-group. Then `merge_from`'s destructure and the **three** exhaustive
fixtures. ⚠ Let the compiler find the fixtures: the exhaustive literals will fail to build until each
is updated — do not add `..Default::default()` to silence one.

- [ ] **Step 3: The pipeline sites.** ⚠⚠ **Both** of them, and the mask one is a trap:

```rust
// crates/pipeline/src/lib.rs:1459 (global)
if !edits.tone_curve.points.is_empty()
    || !edits.tone_curve_red.points.is_empty()
    || !edits.tone_curve_green.points.is_empty()
    || !edits.tone_curve_blue.points.is_empty()
{
    tail.push(Box::new(
        ToneCurveStage::new(&edits.tone_curve.points).with_channels(
            &edits.tone_curve_red.points,
            &edits.tone_curve_green.points,
            &edits.tone_curve_blue.points,
        ),
    ));
}
```

⚠ **Line 1158 is the MASK's curve and does not change** — masks have no per-channel curves by decision.
Leave it exactly as it is, and say so in a comment so the asymmetry reads as deliberate.

⚠⚠ An `is_empty` guard that names one field of four is the regression this feature invites. A test:

```rust
    #[test]
    fn a_channel_curve_alone_still_reaches_the_pipeline() {
        let mut e = Edits::default();
        e.tone_curve_red.points = vec![[0.0, 0.2], [1.0, 1.0]];
        let names = stage_names(&e); // whatever the file's helper is — read it
        assert!(names.iter().any(|n| n == &"tone_curve"), "{names:?}");
    }
```

- [ ] **Step 4: `cargo test --workspace` — the 74 goldens must be UNMOVED.** Step 5: Commit.

---

### Task 3: The GPU twin

**Files:** `crates/gpu-pipeline/src/tone_curve.rs`

- [ ] **Step 1** Concatenate the four tables into one buffer and offset the index per channel:

```wgsl
fn curve(v_in: f32, base: u32) -> f32 { … lut[base + lo] … }
…
    data[p]      = curve(curve(data[p],      0u), params.stride);
    data[p + 1u] = curve(curve(data[p + 1u], 0u), params.stride * 2u);
    data[p + 2u] = curve(curve(data[p + 2u], 0u), params.stride * 3u);
```

⚠⚠ **Composite then channel, matching the CPU** — and when a channel is identity its table still has to
be uploaded, because the shader is unconditional. Build identity tables for empty channels.

⚠ `GpuToneCurveStage::new(points, ctx)` keeps its signature; add `with_channels` mirroring the CPU, and
have it take the tables from `ToneCurveStage::luts()` rather than rebuilding them.

- [ ] **Step 2** Equivalence, through the file's existing harness, with **three different** channel
curves. ⚠ Run the CPU and GPU on the same fixture and compare within the harness's tolerance.

- [ ] **Step 3** ⚠ `accelerate.rs` constructs the GPU stage — find where and pass the channels, or the
GPU tail would silently render the composite only. **That is the worst possible failure here**: a preview
that disagrees with the export. Grep `GpuToneCurveStage` and check every construction site.

- [ ] **Step 4: Gate. Step 5: Commit.**

---

### Task 4: The widget

**Files:** `crates/app/src/develop/panel/tone_curve.rs`, the session's view state, `adjustments.rs`

- [ ] **Step 1** A `CurveChannel` enum (`Rgb`, `Red`, `Green`, `Blue`) with `ALL` and `label()`, beside
`HslChannel`'s idiom. ⚠ It lives in **session view state**, never in `Edits` — `visualize`'s rule — or
switching channels would push a history entry. A test asserts it pushes none.

- [ ] **Step 2** `theme::segmented` above the curve; the selected channel's `ToneCurve` is what the
widget edits; the plotted line and handles take the channel's colour (RGB stays neutral).

- [ ] **Step 3** ⚠ The ghost shows the **selected** channel: `histogram::normalised(hist)[c]` for a
channel, `envelope` for RGB — both already exist and already share one scale, so nothing new is computed.

- [ ] **Step 4** ⚠ `tone_curve::show`'s signature grows; `mask_adjust.rs` also calls it for a mask's
curve — pass `Rgb` there and **do not** draw the selector, since masks have no channel curves.

- [ ] **Step 5: Gate. Step 6: Commit.**

---

### Task 5: A golden with pixel evidence

- [ ] Add **one** golden rendering a fixed RAW with a red curve set, following the existing goldens'
harness exactly. ⚠ The other **74 must be unmoved** — check the count and that none changed.

---

### Task 6: Look at it

```bash
cargo run --release -p app
```

1. Tone → the curve now has **RGB · Red · Green · Blue**; RGB behaves exactly as before.
2. A red curve tints; the line draws red; the ghost follows the channel.
3. ⚠ **Compare the preview against an export** of the same photo — the GPU tail is on by default, and a
   missed construction site would show up exactly here.
4. Switching channels pushes **no** undo step; the curve's own edits do.
5. A **mask's** curve still works and shows **no** selector.
6. Reset Tone clears all four.

Then the journal and `CLAUDE.md` (**407** lines; something moves out, not in).

## Self-review

| Spec requirement | Task |
|---|---|
| Space and clamp unchanged | 1 |
| Composite-then-channel, asserted where the orders differ | 1, 3 |
| Cheap path kept when no channel curve | 1 |
| Three fields, `Tone` group, no `version` bump | 2 |
| Old sidecar loads identity | 2 |
| `all_nonneutral` sets three **different** curves | 2 |
| Stage reaches the pipeline on a channel curve alone | 2 |
| Masks unchanged, deliberately | 2, 4 |
| GPU uploads the CPU's tables; every construction site | 3 |
| Selector is view state, no history | 4 |
| Ghost follows the channel | 4 |
| 74 goldens unmoved + one new | 5 |
| Seen in use, preview vs export | 6 |

**Soft spots:** the composite-then-channel **order is a choice**, not a verified standard — if it reads
wrong in use it is one line plus the shader's mirror. `adjustments.rs` (1,185) and `immich_source.rs`
(864) remain oversized; this touches the former.
