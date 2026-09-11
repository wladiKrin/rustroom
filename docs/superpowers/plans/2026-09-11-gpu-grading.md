# M8.3d Colour Grading on the GPU — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **PROJECT CONVENTIONS, NOT NEGOTIABLE.** Commit messages carry **NO `Co-Authored-By`
> trailer**. **NEVER push.** Branch `m8.3d-gpu-grading` already exists.
>
> ⚠ Before any cargo command:
> ```
> export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
> ```
>
> ⚠⚠ **Iterate with `cargo test --release -p gpu-pipeline`** — a debug build pays ~45 s once for the
> adapter.
>
> ⚠ Read every gate's exit code **from a file, never a pipe**, with a **fresh log name each time**.
> After every scripted edit assert the new text landed; after every sabotage confirm the **named**
> test failed. **Restore a sabotage from a `cp` backup, never `git checkout -- <file>`** — that has
> destroyed uncommitted work in this repo twice.

**Goal:** `color_grading` runs on the GPU, so `hsl + grade + sharpening` is **one** chain of three.

**Architecture:** A `GpuGradingStage` on `GpuHslStage`'s template, uploading the CPU stage's own
256-entry LUT. The Oklab WGSL both shaders need moves into one shared prelude.

**Spec:** `docs/superpowers/specs/2026-09-11-gpu-grading-design.md`

⚠⚠ **Acceptance is the bench's third row**, which reads **1.04× across two chains** today. If it
does not improve, the slice did not work — report that rather than reframing it.

---

## File Structure

| | |
|---|---|
| **Modify** `crates/pipeline/src/stages/color_grading.rs` | `lut()` accessor. |
| **Create** `crates/gpu-pipeline/src/oklab_wgsl.rs` | The shared WGSL prelude. |
| **Modify** `crates/gpu-pipeline/src/hsl.rs` | Use the prelude instead of its inline copy. |
| **Create** `crates/gpu-pipeline/src/grading.rs` | `GpuGradingStage` and its tests. |
| **Modify** `crates/gpu-pipeline/src/accelerate.rs` | One `REPLACEABLE` entry, plus the run tests. |
| **Modify** `crates/gpu-pipeline/examples/gpu_run_bench.rs` | Relabel the third row. |

---

## Task 1: The shared prelude

**Files:** create `oklab_wgsl.rs`; modify `hsl.rs`, `lib.rs`

- [ ] **Step 1: Write the prelude**

`crates/gpu-pipeline/src/oklab_wgsl.rs`:

```rust
//! The Oklab conversions, in WGSL, shared by every shader that needs them.

/// `cbrt_signed`, ProPhoto→Oklab and Oklab→ProPhoto, as WGSL source.
///
/// ⚠⚠ **One copy, because a second is how the two come to disagree.** This project already
/// records **six** copies of a Rec.709 luma triple, every one of which became wrong the day the
/// working space stopped being sRGB. These matrices are the same kind of constant and there is no
/// compiler checking them against `color::oklab`.
///
/// ⚠⚠ **`sign(x) * pow(abs(x), 1/3)`, never `pow(x, 1/3)`.** WGSL's `pow` returns NaN for a
/// negative base, and the CPU uses `cbrt` **specifically because it is sign-preserving** — a wide
/// working space produces scene-referred negatives routinely. A naive port turns every such pixel
/// into NaN, which then propagates through the output matrix and paints holes.
///
/// ⚠ Crate-internal rather than public API: the narrow version of the `pub use stages::hue_table`
/// decision, which deliberately re-exported one item instead of opening a whole module.
pub(crate) const OKLAB_WGSL: &str = r#"
fn cbrt_signed(x: f32) -> f32 { return sign(x) * pow(abs(x), 0.3333333333); }

fn prophoto_to_oklab(r: f32, g: f32, bl: f32) -> vec3<f32> {
    let lc = 0.7153451038*r + 0.3527967327*g - 0.0682510308*bl;
    let mc = 0.2744153967*r + 0.6678313229*g + 0.0577801949*bl;
    let sc = 0.1098335093*r + 0.1862950046*g + 0.7043511321*bl;
    let l_ = cbrt_signed(lc); let m_ = cbrt_signed(mc); let s_ = cbrt_signed(sc);
    return vec3<f32>(
        0.2104542553*l_ + 0.7936177850*m_ - 0.0040720468*s_,
        1.9779984951*l_ - 2.4285922050*m_ + 0.4505937099*s_,
        0.0259040371*l_ + 0.7827717662*m_ - 0.8086757660*s_
    );
}

fn oklab_to_prophoto(bigL: f32, A: f32, B: f32) -> vec3<f32> {
    let li = bigL + 0.3963377774*A + 0.2158037573*B;
    let mi = bigL - 0.1055613458*A - 0.0638541728*B;
    let si = bigL - 0.0894841775*A - 1.2914855480*B;
    let l3 = li*li*li; let m3 = mi*mi*mi; let s3 = si*si*si;
    return vec3<f32>(
        1.7386588588*l3 - 0.9880931444*m3 + 0.2495310446*s3,
        -0.7071491793*l3 + 1.9343303974*m3 - 0.2272015198*s3,
        -0.0840839768*l3 - 0.3575352425*m3 + 1.4409285251*s3
    );
}
"#;
```

Add `mod oklab_wgsl;` to `lib.rs` (no `pub use` — it is crate-internal).

- [ ] **Step 2: Make `hsl.rs` use it**

Replace `hsl.rs`'s inline `cbrt_signed` and both matrix blocks with calls. Its shader becomes
`format!("{OKLAB_WGSL}{SHADER}")` — or a `const` concat — and the body reads:

```wgsl
    let lab = prophoto_to_oklab(r, g, bl);
    let bigL = lab.x; let A = lab.y; let B = lab.z;
```

and at the end:

```wgsl
    let out = oklab_to_prophoto(L2, A2, B2);
    data[p]      = max(out.x, 0.0);
    data[p + 1u] = max(out.y, 0.0);
    data[p + 2u] = max(out.z, 0.0);
```

- [ ] **Step 3: Run — `hsl`'s OWN tests are the proof this moved nothing**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "hsl::|test result" | head -8
```
Expected: **all 4 hsl tests still pass, unchanged**. ⚠⚠ That is the whole verification for this
task — the extraction is behaviour-preserving or it is not, and `the_gpu_hsl_matches_the_cpu_within_a_hundredth_of_a_level`
is what says so. A prelude that transcribed one digit wrongly fails it.

- [ ] **Step 4: Sabotage**

```bash
cp crates/gpu-pipeline/src/oklab_wgsl.rs /tmp/m83d-prelude.bak
python3 - <<'PY'
p='crates/gpu-pipeline/src/oklab_wgsl.rs'; s=open(p).read()
o="0.7153451038*r"
assert o in s, "sabotage did not match"
open(p,'w').write(s.replace(o, "0.7253451038*r"))
PY
cargo test --release -p gpu-pipeline > /tmp/m83d-sab1.log 2>&1
grep -q "test result:" /tmp/m83d-sab1.log && grep -E "^test .* FAILED" /tmp/m83d-sab1.log | head -3 || echo "  DID NOT COMPILE"
cp /tmp/m83d-prelude.bak crates/gpu-pipeline/src/oklab_wgsl.rs
```
Expected: `the_gpu_hsl_matches_the_cpu_within_a_hundredth_of_a_level ... FAILED`.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83d-cl1.log 2>&1; echo "CLIPPY=$?"
cargo test --release -p gpu-pipeline > /tmp/m83d-t1.log 2>&1; echo "TEST=$?"
```
⚠ **Both must be 0 before committing.** Three milestones in a row have committed with clippy red.

```bash
git add -A && git commit -F - <<'EOF'
refactor: one WGSL Oklab prelude, shared by every shader

⚠⚠ A second copy of these matrices is how the two come to disagree.
This project already records six copies of a Rec.709 luma triple, every
one of which became wrong the day the working space changed -- and WGSL
has no compiler checking these against color::oklab.

⚠ The verification is that hsl's OWN equivalence test still passes
unchanged: the extraction is behaviour-preserving or it is not, and a
prelude with one digit transcribed wrongly fails it. Sabotage-verified.
EOF
```

---

## Task 2: `ColorGradingStage::lut()`

**Files:** modify `crates/pipeline/src/stages/color_grading.rs`

- [ ] **Step 1: Add the accessor**

```rust
    /// The `(da, db, dl)` table, one entry per lightness step.
    ///
    /// ⚠⚠ **Exposed so a GPU twin uploads THIS table rather than rebuilding it.** Two
    /// constructions of one table is how they come to disagree about Blending and Balance — and
    /// nothing in the rendered image would announce it, because both answers are plausible
    /// photographs. The rule `HueTable::entries` already follows, for the same reason.
    ///
    /// ⚠ The caller must reproduce [`Self::grade_at`]'s interpolation, **index-clamped**: see the
    /// headroom warning there.
    pub fn lut(&self) -> &[[f32; 3]] {
        &self.lut
    }
```

⚠ `pipeline` is a **library**, so a `pub fn` with no caller yet is public API rather than dead
code — unlike the bin-crate trap this file records nine times. It can land on its own.

- [ ] **Step 2: Gate and commit** (same two-exit-code gate as Task 1).

---

## Task 3: `GpuGradingStage`

**Files:** create `crates/gpu-pipeline/src/grading.rs`; modify `lib.rs`

- [ ] **Step 1: Write the stage**

The shader, after the prelude:

```wgsl
struct Params {
    pixels: u32,
    row_groups: u32,
    lut_len: u32,
    pad0: u32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> lut: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    let lab = prophoto_to_oklab(data[p], data[p + 1u], data[p + 2u]);

    // ⚠⚠ The INDEX is clamped, never the pixel's own lightness. A scene-referred value above
    // white has L > 1, and clamping the VALUE would drag every highlight back toward white --
    // destroying exactly the headroom the wide working space exists to preserve, on a photograph,
    // invisibly, because the result is still a plausible picture.
    let t = clamp(lab.x, 0.0, 1.0) * f32(params.lut_len - 1u);
    let i = min(u32(floor(t)), params.lut_len - 1u);
    let j = min(i + 1u, params.lut_len - 1u);
    let f = t - f32(i);
    let da = mix(lut[i*3u],       lut[j*3u],       f);
    let db = mix(lut[i*3u + 1u],  lut[j*3u + 1u],  f);
    let dl = mix(lut[i*3u + 2u],  lut[j*3u + 2u],  f);

    let out = oklab_to_prophoto(lab.x + dl, lab.y + da, lab.z + db);
    // ⚠ Clamped at 0 only, never at 1 -- the output transform is the only place clipping belongs.
    data[p]      = max(out.x, 0.0);
    data[p + 1u] = max(out.y, 0.0);
    data[p + 2u] = max(out.z, 0.0);
}
```

The Rust half mirrors `hsl.rs` exactly — a 16-byte uniform (`pixels`, `row_groups`, `lut_len`,
pad), a storage buffer of `lut.len() * 3` floats, a three-entry bind group, 2D dispatch.

```rust
pub struct GpuGradingStage {
    table: Vec<f32>,
    cpu: pipeline::ColorGradingStage,
}

impl GpuGradingStage {
    pub fn new(g: &edit_model::ColorGrading, _ctx: &Arc<GpuContext>) -> Self {
        let cpu = pipeline::ColorGradingStage::new(g);
        // ⚠ The CPU stage's OWN table, flattened -- never rebuilt here.
        let table = cpu.lut().iter().flatten().copied().collect();
        Self { table, cpu }
    }
}
```

`scratch_planes` is left at its default **0** — per-pixel and in-place-safe — and `encode` takes
`_scratch: &[wgpu::Buffer]`.

- [ ] **Step 2: Write the tests**

```rust
    /// ⚠⚠ **The prelude's cross-check, and it needs NO extra machinery.** A neutral grade has an
    /// all-zero LUT, so the shader becomes exactly `oklab_to_prophoto(prophoto_to_oklab(rgb))` —
    /// a pure round trip through both matrices. Comparing that against `color::oklab`'s own round
    /// trip tests the prelude against the Rust it must agree with, through the real stage.
    ///
    /// ⚠ Compared against the CPU **round trip**, not against the input: `cbrt` then cube is not
    /// the identity in f32, so asserting `out == in` would be asserting something false about
    /// correct code — the trap this project records five times.
    ///
    /// ⚠⚠ The fixture spans a **negative** sample and one **above 1.0**. An in-gamut-only set
    /// cannot see a wrong `cbrt_signed` (which needs a negative LMS component) or a stray value
    /// clamp (which needs L > 1), which are the two ways this can be wrong.
    #[test]
    fn the_wgsl_prelude_agrees_with_color_oklab() {
        let Some(ctx) = ctx() else { return };
        let src = vec![
            0.18, 0.18, 0.18, 0.80, 0.20, 0.35, -0.50, 0.02, 0.03, 1.60, 1.20, 0.90, 0.0, 0.0, 0.0,
            0.04, 0.90, 0.30,
        ];
        let img = ImageBuf::from_vec(6, 1, src.clone());
        let gpu = chained(&ctx, &ColorGrading::default(), img);
        let want: Vec<f32> = src
            .chunks_exact(3)
            .flat_map(|p| {
                let lab = color::oklab::prophoto_to_oklab([p[0], p[1], p[2]]);
                let o = color::oklab::oklab_to_prophoto(lab);
                [o[0].max(0.0), o[1].max(0.0), o[2].max(0.0)]
            })
            .collect();
        let d = crate::compare(&ImageBuf::from_vec(6, 1, want), &gpu);
        assert!(d.worst_levels < 0.01, "worst {} levels", d.worst_levels);
    }

    #[test]
    fn a_lightness_above_one_keeps_its_headroom() {
        // ⚠⚠ Where a value-clamping shader fails and NOWHERE ELSE. `clamp(l, 0, 1)` instead of
        // clamping the index drags every highlight back toward white; an in-gamut fixture sees
        // nothing, and the result is still a plausible photograph.
        let Some(ctx) = ctx() else { return };
        let g = graded();
        let img = ImageBuf::from_vec(2, 1, vec![2.4, 2.1, 1.8, 0.4, 0.4, 0.4]);
        let cpu = pipeline::ColorGradingStage::new(&g).apply(img.clone());
        let gpu = chained(&ctx, &g, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.01, "worst {} levels", d.worst_levels);
        // ⚠ And the fixture check: the bright pixel must still BE bright, or the test would pass
        // against two implementations that both crushed it.
        assert!(gpu.data()[0] > 1.5, "the highlight was crushed to {}", gpu.data()[0]);
    }

    #[test]
    fn the_gpu_grade_matches_the_cpu_within_a_hundredth_of_a_level() { /* spread fixture */ }

    #[test]
    fn a_negative_sample_does_not_become_nan() {
        // ⚠⚠ Use `[-0.5, 0.02, 0.03]` — COMPUTED, not guessed. M8.3b's first attempt used
        // `[-0.05, 0.3, 0.2]`, whose LMS components are all POSITIVE, so `pow` never saw a
        // negative base and the sabotage passed.
    }

    #[test]
    fn the_stage_really_grades_rather_than_copying() {
        // ⚠⚠ Every equivalence test above passes against a GPU and CPU stage that BOTH do
        // nothing. This is what says the fixture is one the grade moves.
    }

    #[test]
    fn it_asks_for_no_scratch_planes() { /* a chain of hsl + grade allocates zero */ }
```

- [ ] **Step 3: Run, then sabotage**

Each restored from a `cp` backup, each expected to fail the **named** test:
`clamp(lab.x, 0.0, 1.0)` moved onto the value → `a_lightness_above_one_keeps_its_headroom`;
rebuilding the table in the stage with a wrong Blending → the equivalence test;
`scratch_planes` returning 1 → `it_asks_for_no_scratch_planes`;
`cbrt_signed` → bare `pow` → `a_negative_sample_does_not_become_nan`.

- [ ] **Step 4: Gate and commit.**

---

## Task 4: One line of substitution, and the three-stage run

**Files:** modify `crates/gpu-pipeline/src/accelerate.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn three_consecutive_gpu_stages_merge_into_one_chain() {
        // ⚠⚠ The whole point of the slice: with the grade accelerated, hsl + grade + sharpening
        // is ONE chain rather than two. This is the first three-stage run.
        let Some(ctx) = ctx() else { return };
        let mut e = with_sharpening(with_hsl());
        e.color_grading.shadows.saturation = 30.0;
        let tail = tail_of(&e);
        let before = tail.stage_names();
        assert!(
            before
                .windows(3)
                .any(|w| w == ["hsl", "color_grading", "sharpening"]),
            "fixture check: the three must be consecutive, got {before:?}"
        );
        let after = accelerate(tail, &e, SCALE, &ctx).stage_names();
        assert_eq!(chains(&after), 1, "got {after:?}");
        assert_eq!(after.len(), before.len() - 2, "three collapse to one, got {after:?}");
    }

    #[test]
    fn a_three_stage_run_renders_the_same_picture() {
        // ⚠ The carrier. The shape test above passes against a chain that computes nonsense.
    }
```

⚠ **The existing `a_cpu_stage_between_two_gpu_stages_produces_two_chains` must be re-aimed**, since
`color_grading` is no longer a CPU stage. Use a stage that still is — `vignette` or `saturation` —
and **say in the test why**: the split rule is still load-bearing, only its example moved.

- [ ] **Step 2: Add the table entry**

```rust
    ("color_grading", |e, _, ctx| {
        Box::new(GpuGradingStage::new(&e.color_grading, ctx))
    }),
```

⚠ The `at-most-once` guard already loops the whole table, so it covers the new entry with no edit —
which is what that generalisation was for.

- [ ] **Step 3: Run, sabotage, gate, commit.**

---

## Task 5: Measure

**Files:** modify `crates/gpu-pipeline/examples/gpu_run_bench.rs`

- [ ] **Step 1: Relabel the third row** — it is `hsl + grade + sharpening (one chain)` now, and
the printed `chains` count is what shows the structural half.

- [ ] **Step 2: Run on a QUIET machine and record**

```bash
cargo run --release -p gpu-pipeline --example gpu_run_bench 2>&1 | tail -30
```
⚠⚠ Nothing else running. M8.3c's first bench was taken during a `cargo test --workspace` and read
**49.55 ms of CPU for less work than a later 37.97 ms** — caught only because the column was
internally inconsistent.

⚠⚠ **The third row read 1.04× across two chains. If it has not improved, say so plainly** and
investigate rather than reporting the other rows.

- [ ] **Step 3: Commit with the numbers in the message.**

---

## Task 6: Documentation and the GUI

- [ ] **Step 1: The `CLAUDE.md` section**, above M8.3c's, recording: the measured third row; that
the stage was already a LUT and why that made it cheap; the shared prelude and its behavioural
guard; the index-vs-value clamp; and the **renumber** (grading M8.3d, masks M8.3e, prefix M8.3f).

- [ ] **Step 2: Correct the stale numbering** in `CLAUDE.md`'s NEXT ACTION and `docs/BACKLOG.md`,
which both still say "M8.3d — the mask loop". ⚠ The spec flagged this; it is a real trap for the
next reader.

- [ ] **Step 3: Update the Settings hint.** It currently says *"an active colour grade splits the
run so the saving is smaller"* — which this slice makes **false**. ⚠ A hint that describes a
caveat that no longer exists is the stale-string class this file records for "Reset crop" and the
lens Scale slider.

- [ ] **Step 4: Full gate** — `fmt --check`, `clippy`, `test --workspace`, 63 goldens run and none
moved, 0 skipped. **Fresh log names.**

- [ ] **Step 5: Commit and launch the GUI.** Tell the user: Settings → **Rendering → Use the GPU**
now covers HSL, colour grading and sharpening; the picture must not change; and **the grade caveat
is gone** — a graded photo should now be as fast as an ungraded one.

---

## Self-Review

**Spec coverage.** §1 the motivation → Task 5 measures it. §2 the LUT → Tasks 2–3. §3 the prelude
and its guard → Task 1 and Task 3's `the_wgsl_prelude_agrees_with_color_oklab`. §4 the clamp trap →
Task 3. §5 substitution → Task 4. §6 all six tests → Task 3 and Task 4. §7 acceptance → Task 5. §8
non-goals → Task 6.

**Placeholder scan.** ⚠ Task 3 Step 2 gives four tests in full and **sketches three** — the spread
fixture, the NaN case and the copy guard — because each is a near-verbatim copy of an existing test
in `hsl.rs` or `sharpen.rs` and the plan names which. That is a deliberate exception; everything
whose *content* is new is written out.

**Type consistency.** `ColorGradingStage::lut() -> &[[f32; 3]]`; `GpuGradingStage::new(&ColorGrading,
&Arc<GpuContext>)`; `OKLAB_WGSL: &str` crate-internal; `scratch_planes()` left at its default 0;
`REPLACEABLE`'s `Builder` signature unchanged, so the new entry ignores its `RenderScale`.

⚠ **One thing this plan changes that the spec did not mention:** the existing split test must be
re-aimed at a stage that is still on the CPU. Flagged in Task 4 rather than discovered mid-task.
