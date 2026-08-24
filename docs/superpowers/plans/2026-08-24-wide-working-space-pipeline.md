# Wide working space — the pipeline switch

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the whole pipeline from linear sRGB to linear ProPhoto, so saturated colour survives the decode instead of being clipped before any slider is touched.

**Architecture:** libraw decodes straight to ProPhoto (`output_color = 4`); the eight primary-dependent stages take their luma from `color::luminance()` and their Oklab from the ProPhoto pair; the output transform converts ProPhoto→sRGB before encoding. Golden charts are authored in sRGB and converted at construction, so their intent survives and the `neutral` case still round-trips.

⚠⚠ **This change is atomic and lands as ONE commit.** There is no ordering of these edits that leaves the suite green partway through: the moment the output transform converts, every golden disagrees until the charts, the stages and the decode all agree with it. Tasks 1–4 therefore make edits **without committing**; Task 4 regenerates the references behind a gate, and the single commit comes at the end of Task 5. Committing a red suite to keep task granularity would trade a real property — every commit builds and passes — for a cosmetic one.

**Tech Stack:** Rust, libraw via `rsraw`, `color` (from plan 1).

**Spec:** `docs/superpowers/specs/2026-08-24-wide-working-space-design.md`. This is **plan 2 of 2**; plan 1 landed 2026-08-24.

---

## ⚠⚠ Read the spec's corrected finding first

An earlier version of the spec claimed ProPhoto's luminance row "collapses blue" and specified Oklab
L to work around it. **That was wrong and has been corrected.** Luminance is physical: sRGB blue
reads 0.0722 via Rec.709 and 0.0606 via ProPhoto's row. The `0.0001` applies to ProPhoto's own blue
*primary*, not to real blues.

Two consequences for this plan:

- **`color::luminance()` is linear luminance**, so **every tuned constant carries over unchanged** —
  `local_contrast`'s `MID_GRAY = 0.18`, `sharpening`'s `LUMA_EPS`, `GAIN_MAX`, `HALO_KNEE`, and
  `dehaze`'s airlight percentile. **Do not re-derive them.** An earlier draft of this plan told you
  to, and that instruction is gone.
- **Sharpening, clarity and dehaze keep their character.** If they feel different on a real photo
  after this change, something is wrong — the only intended change is gamut.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/raw-decode/src/backend_rsraw.rs` | `output_color = 4`. |
| `crates/raw-decode/src/lib.rs` | Doc comments: the buffer is ProPhoto now, not sRGB. |
| `crates/pipeline/src/stages/output_transform.rs` | ProPhoto→sRGB matrix, then the transfer curve. |
| `crates/pipeline/src/stages/{white_balance,tone_regions,saturation,dehaze,sharpening,local_contrast}.rs` | Local luma → `color::luminance()`. |
| `crates/pipeline/src/stages/{hsl,noise_reduction,sharpening,local_contrast}.rs` | Oklab calls → ProPhoto pair. |
| `crates/pipeline/tests/golden.rs` | Charts authored in sRGB; all 38 references regenerated. |
| `crates/image-decode/src/lib.rs` | JPEG/PNG/TIFF land in ProPhoto too, or they are the wrong colour. |

---

### Task 1: The output transform

Do this **first**: it is the piece everything else has to agree with, and its own unit test proves
the seam in isolation before any other file changes. ⚠ It also *breaks all 38 goldens the moment it
lands* — see Step 5 — which is why nothing is committed until Task 5.

**Files:**
- Modify: `crates/pipeline/src/stages/output_transform.rs`

- [ ] **Step 1: Write the failing test**

Add to `output_transform.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    #[test]
    fn a_neutral_survives_the_round_trip() {
        // ⚠ The property the whole switch rests on: ProPhoto→sRGB is exact for in-gamut colour,
        // so a chart authored in sRGB and converted in must come back where it started. If this
        // drifts, every one of the 38 goldens is wrong in the same invisible way.
        let mut img = ImageBuf::new(3, 1);
        let srgb = [[0.18, 0.18, 0.18], [0.9, 0.2, 0.1], [0.1, 0.4, 0.8]];
        for (x, c) in srgb.iter().enumerate() {
            img.set_pixel(x, 0, color::srgb_to_prophoto(*c));
        }
        let out = OutputTransformSrgb.apply(img);
        for (x, c) in srgb.iter().enumerate() {
            let got = out.pixel(x, 0);
            for i in 0..3 {
                let want = color::linear_to_srgb(c[i]);
                assert!(
                    (got[i] - want).abs() < 2e-3,
                    "pixel {x} channel {i}: got {}, want {want}",
                    got[i]
                );
            }
        }
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p pipeline a_neutral_survives_the_round_trip 2>&1 | tail -12
```

Expected: FAIL — the stage currently applies only the transfer curve, so ProPhoto values go out
un-converted and read too saturated.

- [ ] **Step 3: Convert before encoding**

```rust
impl PipelineStage for OutputTransformSrgb {
    fn name(&self) -> &'static str {
        "output_transform_srgb"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // ⚠ Two steps, and the order is not negotiable: primaries first, in linear light, then
        // the transfer curve. Encoding before converting would apply a matrix to gamma values,
        // which is meaningless and does not fail loudly.
        //
        // ⚠ Out-of-gamut colour arrives here as negative components — that is the point of a
        // wide working space. `linear_to_srgb` clamps, so the clipping happens exactly once, at
        // the last possible moment, rather than silently in the decode.
        image
            .map_pixels(color::prophoto_to_srgb)
            .map_samples(color::linear_to_srgb)
    }
}
```

- [ ] **Step 4: Run it**

```bash
cargo test -p pipeline output_transform 2>&1 | tail -8
```

Expected: PASS.

- [ ] **Step 5: Note where the goldens now stand — and do not commit**

```bash
cargo test -p pipeline --test golden 2>&1 | tail -5
```

⚠ Expected: **all 38 FAIL**, and that is correct here. The charts still write raw values that the
transform now reads as ProPhoto. **Do not regenerate and do not commit** — Task 5 does both, once,
after everything agrees.

---

### Task 2: One luminance, six call sites

**Files:**
- Modify: `crates/pipeline/src/stages/{white_balance,tone_regions,saturation,dehaze,sharpening,local_contrast}.rs`

- [ ] **Step 1: Replace each local definition**

Every one of these files defines its own Rec.709 luma. Delete the local definition and call
`color::luminance` instead. The six sites, exactly:

| file | delete | replace uses with |
|---|---|---|
| `white_balance.rs:5` | `const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];` | `color::luminance([a, b, c])` at lines ~19, ~70, ~71 |
| `tone_regions.rs:4` | `const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];` | `color::luminance([r, g, b])` at line ~47 |
| `saturation.rs:5` | `const LUMA_R: f32 = 0.2126;` and its siblings | `color::luminance(px)` |
| `dehaze.rs:34` | the local `fn luma` | `color::luminance(px)` |
| `sharpening.rs:31` | the local `fn luma` | `color::luminance(px)` |
| `local_contrast.rs:25` | the local `fn luma` | `color::luminance(px)` |

⚠ **`white_balance` is the awkward one and deserves reading before editing.** Line ~19 computes
`LUMA[0]*gains[0] + …` — the luminance of *white after the gains*, used to renormalise so a white-
balance change does not alter overall brightness. That is `color::luminance(gains)`, which is
correct because it is still the luminance of a pixel (`[1,1,1]` scaled). Lines ~70–71 are the same
idea on a grey. **Do not** mechanically substitute without checking each is a pixel and not a
coefficient list.

⚠ `saturation.rs`'s **test module** has its own private `fn luma` (line ~62). **Leave it.** A test
that computes its expectation through production code cannot witness a change to production code —
the same reason the golden harness keeps a private copy of the quantiser.

- [ ] **Step 2: Add the dependency where missing**

`color` is already a dependency of `pipeline`. Confirm with:

```bash
grep -n "^color" crates/pipeline/Cargo.toml
```

Expected: a line. If absent, add `color = { workspace = true }`.

- [ ] **Step 3: Verify there is exactly one definition left**

```bash
grep -rn "0\.2126" crates/pipeline/src/ crates/color/src/
```

Expected: **one hit in `saturation.rs`'s test module** and **one in `color/src/lib.rs`'s
`luminance_matches_rec709` test**. Any hit in production code means a site was missed.

- [ ] **Step 4: Build, and check the unit tests only**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test -p pipeline --lib 2>&1 | tail -5
```

⚠ Expected: the **unit** tests pass; the goldens still fail from Task 1. Run `--lib` here rather
than the whole suite, so the expected golden failures cannot hide a real regression. **No commit
yet** — see the header.

---

### Task 3: Oklab in the working space

**Files:**
- Modify: `crates/pipeline/src/stages/{hsl,noise_reduction,sharpening,local_contrast}.rs`

- [ ] **Step 1: Swap the calls**

In each file, replace:

```rust
color::linear_srgb_to_oklab(px)   →   color::oklab::prophoto_to_oklab(px)
color::oklab_to_linear_srgb(lab)  →   color::oklab::oklab_to_prophoto(lab)
```

⚠ **Both directions in the same edit, in every file.** A stage that converts *into* ProPhoto-Oklab
and back out through the sRGB inverse produces a colour shift that is smooth, plausible and
extremely hard to attribute later.

- [ ] **Step 2: Verify no sRGB Oklab call survives in the pipeline**

```bash
grep -rn "linear_srgb_to_oklab\|oklab_to_linear_srgb" crates/pipeline/src/
```

Expected: **no output.** The sRGB pair stays in `color` for other callers, but nothing in the
pipeline should reach it.

- [ ] **Step 3: Build, unit tests only**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test -p pipeline --lib 2>&1 | tail -5
```

Expected: unit tests pass. **No commit yet.**

---

### Task 4: The golden charts speak sRGB

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`

- [ ] **Step 1: Add the conversion helper**

Near the top of `golden.rs`, after the imports:

```rust
/// Write an **sRGB** colour into a working-space chart.
///
/// ⚠ The charts are authored in sRGB and converted here, rather than having their literals
/// reinterpreted as ProPhoto. Two reasons, and the second is the important one.
///
/// The literals mean something: `[1.0, 0.0, 0.0]` is written to say "pure red", and read as
/// ProPhoto it silently becomes a far more saturated colour than any camera will produce — the
/// chart would stop resembling the thing it was built to exercise.
///
/// ⚠ And it restores the check this change otherwise destroys. All 38 references move at once,
/// so the usual proof — *"the others stayed byte-identical, therefore the change was scoped"* —
/// is unavailable. Authoring in sRGB means **the `neutral` golden must come back within `TOL`**,
/// because sRGB → ProPhoto → sRGB is a round trip. That single property is the strongest
/// evidence available that the colour maths is right.
fn set_srgb(img: &mut ImageBuf, x: usize, y: usize, srgb: [f32; 3]) {
    img.set_pixel(x, y, color::srgb_to_prophoto(srgb));
}
```

- [ ] **Step 2: Point every chart builder at it**

Six builders call `img.set_pixel(...)`: `chart` (~line 30), `muted_chart` (~51), `hue_sweep_chart`
(~82), `noise_chart` (~128), `local_contrast_chart` (~160), `hazy_chart` (~193). Change each
`img.set_pixel(x, y, PX)` to `set_srgb(&mut img, x, y, PX)`.

```bash
grep -c "set_srgb" crates/pipeline/tests/golden.rs
```

Expected: **7** — one definition plus six call sites.

```bash
grep -n "img.set_pixel" crates/pipeline/tests/golden.rs
```

Expected: **no output.** A surviving `set_pixel` is a chart still speaking ProPhoto, and its golden
would then be regenerated wrong and locked in.

- [ ] **Step 3: Check the neutral case before regenerating anything**

```bash
cargo test -p pipeline --test golden golden_neutral 2>&1 | tail -12
```

⚠ **Expected: PASS, with no regeneration.** This is the load-bearing moment of the whole plan. The
neutral golden goes chart → output transform only, so sRGB → ProPhoto → sRGB must return the
original bytes within `TOL`.

⚠ **If it fails, stop and do not regenerate.** A failure here means the matrices, the transform
order, or the chart conversion is wrong, and regenerating would bake that error into all 38
references where nothing could ever detect it again.

- [ ] **Step 4: Regenerate the rest**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden 2>&1 | tail -5
cargo test -p pipeline --test golden 2>&1 | tail -5
git status --short crates/pipeline/tests/
```

Expected: the second run passes, and `git status` lists the changed PNGs. ⚠ **`golden_neutral.png`
should be unchanged or nearly so** — if it moved substantially, Step 3 passed by luck of tolerance
and something is still wrong.

- [ ] **Step 5: Look at the references, do not just count them**

```bash
open crates/pipeline/tests/golden/
```

⚠ **A golden nobody has looked at is just a hash** — this project has recorded that lesson once
already, when a noise chart's regions turned out to be bleeding into each other. Check that each
chart still looks like what its name says: the primaries chart still shows red, green and blue
bands; the hue sweep still sweeps; the haze chart still has a horizon.

⚠ **Still no commit.** The decode is not switched yet, so the pipeline expects ProPhoto while
libraw is still emitting sRGB — the suite would pass while the *application* rendered wrongly. Task 5
closes that gap and commits.

---

### Task 5: The decode lands in ProPhoto

**Files:**
- Modify: `crates/raw-decode/src/backend_rsraw.rs:69`, `crates/raw-decode/src/lib.rs`, `crates/image-decode/src/lib.rs`

- [ ] **Step 1: Switch libraw's output space**

`crates/raw-decode/src/backend_rsraw.rs`, line 69:

```rust
        // ⚠ 4 = ProPhoto (ROMM), not 1 = sRGB. libraw applies the camera matrix into this space
        // directly, so the widest transform in the chain is one we do not have to write.
        //
        // ⚠ The whole pipeline reads this as its working space. Changing this number changes
        // every rendered pixel and every golden — it is not a tuning knob.
        params.output_color = 4;
```

- [ ] **Step 2: Correct the doc comments that now lie**

`crates/raw-decode/src/lib.rs` says the samples are "linear light in sRGB primaries" (~line 186) and
the module doc mentions sRGB (~line 8). Both are now wrong. Replace with linear ProPhoto, and note
that `DecodeOptions::display()` is the exception:

```rust
/// With [`DecodeOptions::linear_scene`] the samples are linear light in **ProPhoto (ROMM)
/// primaries** — the pipeline's working space.
///
/// ⚠ [`DecodeOptions::display`] still asks libraw for sRGB, because it feeds `rustroom decode`'s
/// viewable dump rather than the pipeline. Those two paths deliberately differ.
```

- [ ] **Step 3: Convert non-RAW input too**

⚠ **`image-decode` is easy to forget and produces a spectacular bug if it is.** It decodes
JPEG/PNG/TIFF, calls `srgb_to_linear`, and hands back linear **sRGB** — which the pipeline would now
treat as ProPhoto, rendering every JPEG heavily oversaturated. Add the conversion after the transfer
decode:

```rust
    // ⚠ Non-RAW input is sRGB by assumption (ICC is deferred), so it must be converted into the
    // working space like everything else. Without this a JPEG renders as though its colours were
    // ProPhoto — a large, obvious oversaturation that no unit test here would catch.
    let buf = buf.map_pixels(color::srgb_to_prophoto);
```

- [ ] **Step 4: Verify against a real RAW**

```bash
export RAW_FIXTURE=/absolute/path/to/a.RAF
cargo test -p raw-decode --release 2>&1 | tail -8
```

Expected: the gated decode tests pass. ⚠ They check dimensions and scale, not colour — colour is
judged in Task 6.

- [ ] **Step 5: The single commit**

Everything from Tasks 1–5 lands together, for the reason in the header: no earlier point leaves both
the suite green *and* the application correct.

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: **full suite green** — this is the first moment since Task 1 that it is, and if it is not,
do not commit.

```bash
git add crates/pipeline/ crates/raw-decode/src/ crates/image-decode/src/
git commit -m "feat: the working space is linear ProPhoto

libraw decodes with output_color = 4, the output transform converts
ProPhoto->sRGB before encoding, the six duplicated Rec.709 luma copies
collapse into color::luminance(), and the four Oklab call sites use the
ProPhoto pair. image-decode converts non-RAW input in, or a JPEG would be
read as ProPhoto and render heavily oversaturated.

All 38 goldens move deliberately. The charts are authored in sRGB and
converted at construction, which keeps their intent and makes the neutral
case a round trip -- the check that replaces 'the others stayed identical'.

One commit because no earlier point leaves the suite green and the
application correct at the same time."
```

---

### Task 6: Judge it on a photograph

⚠ **Nothing above proves the result is *better*.** The goldens prove it is consistent; only a real
file shows whether the gamut win appears and whether anything regressed.

- [ ] **Step 1: Export the same RAW before and after**

```bash
git stash
cargo run --release -p rustroom -- render "$RAW_FIXTURE" /tmp/before.jpg
git stash pop
cargo run --release -p rustroom -- render "$RAW_FIXTURE" /tmp/after.jpg
```

- [ ] **Step 2: Compare, on these four things specifically**

1. **A saturated subject** — a red flower, a deep blue sky, foliage. This is where the gain should
   be: detail inside the saturated area that was previously flat, because it had been clipped at
   decode.
2. **Skin tones and neutrals must not shift.** They are well inside both gamuts, so they should look
   the same. A visible change here means the matrices or the white adaptation are wrong.
3. **Sharpening, clarity and dehaze must feel unchanged.** `luminance()` is linear luminance, so
   their tuned constants still mean what they meant. If they feel different, something is wrong —
   see this plan's opening section.
4. **A JPEG re-exported** — the check that Task 5 Step 3 actually landed. Heavy oversaturation means
   `image-decode` is still handing sRGB to a ProPhoto pipeline.

- [ ] **Step 3: Launch the GUI and look at the preview**

```bash
cargo run --release -p app
```

The preview goes through the same transform, so it should match the export. ⚠ A preview that looks
right while the export does not (or vice versa) means the two paths disagree about the working
space — check that nothing besides `OutputTransformSrgb` is converting.

- [ ] **Step 4: Update CLAUDE.md**

Record: the working space is linear ProPhoto and where it is set; that `image-decode` converts too;
one `color::luminance()` replacing six copies; the corrected blue-collapse finding and that the
tuned constants were therefore untouched; the chart-authoring change and the `neutral`-golden
round-trip that replaces the "others stayed identical" check; and that all 38 references moved
deliberately.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the move to a ProPhoto working space"
```

---

## Self-review notes

Checked against the spec:

- Working space linear ProPhoto via `output_color = 4` → Task 5. ✅
- Output transform ProPhoto→sRGB → Task 1. ✅
- Six luma sites → one `color::luminance()` → Task 2. ✅
- Four Oklab call sites → ProPhoto pair → Task 3. ✅
- Charts authored in sRGB; `neutral` within `TOL` → Task 4, with an explicit stop-if-it-fails gate. ✅
- All 38 regenerated → Task 4 Step 4. ✅
- Judged on a real `.RAF` → Task 6. ✅
- Tuned constants **not** re-derived → stated in the opening section and absent from every task. ✅

**Beyond the spec, and why.** Task 5 Step 3 converts **non-RAW input**. The spec never mentions
`image-decode`, but it hands the pipeline linear sRGB, which after this change would be read as
ProPhoto — every JPEG would render heavily oversaturated. It is the largest bug this change can
produce and the easiest to miss, since no test in that crate looks at colour.

**⚠ One commit, and that is a considered choice rather than laziness.** The first draft of this plan
committed after each task, which would have put **several commits with 38 failing golden tests** into
the history. There is no ordering that avoids it: the instant the output transform converts, every
golden disagrees until the charts, the stages *and* the decode agree with it. Granular commits are
worth a lot, but not more than "every commit builds and passes" — so the tasks stay separate for
reviewability and the commit happens once, at the end of Task 5, behind the neutral-golden gate.

**Ordering within that is still deliberate.** Task 1 proves the transform in isolation with its own
unit test; Tasks 2 and 3 are pure substitutions; Task 4 gates on the neutral round trip *before*
regenerating anything; Task 5 closes the decode gap last, because until it lands the suite can be
green while the application renders wrongly.
