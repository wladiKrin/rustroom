# W5 track A — creative 3-D LUTs: Implementation Plan

> **For the agent executing this:** use `superpowers:executing-plans`, task by task. Spec:
> `docs/superpowers/specs/2026-09-04-creative-luts-design.md`. **Every decision in that spec is
> settled — none is yours to re-open.** If you meet a question it does not answer, `PARALLEL-WORK.md`
> §6 applies: **stop and report**, do not choose.

**Goal:** A `.cube` file can be applied as a creative look, at a declared input space, with a
strength slider — placed so a film emulation replaces the camera rendering and a grade modifies it.

**Architecture:** A pure parser and trilinear lookup with no GUI dependency; one stage; the declared
`LutSpace` decides whether the stage runs *after* `DisplayRenderingStage` or *instead of* it.

**Tech Stack:** Rust; `pipeline` (zero GUI dependency), `edit-model`, `crates/app`.

**Baseline:** `main` at **1896 tests, 53 goldens**, all gates green.

---

## ⚠⚠ Surfaces this track OWNS, and surfaces it must not touch

**Owns, exclusively this wave:** the `Edits` field list **and its five exhaustive sites**, pipeline
stage order, `PrefixStepKey`, `DisplayRenderingStage`'s placement.

**Barred — another track owns these:**
- `Mask`, `MaskStage`, `develop/panel/masks.rs` → **track C**
- `render_engine::pack`, `Session` view state, `commands.rs`, `menu.rs` → **track B**

⚠ **Goldens are shared with track C.** Rule 3: **append, never insert** — new goldens go at the end
of the list and new PNGs are new files.

---

## Build environment — every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Conventions:** no `Co-Authored-By` trailer; **never push**; `fmt` + `clippy --all-targets
--all-features -- -D warnings` + `test --all` green before reporting done.

⚠ **Read every exit code from a file, never through a pipe** — `${PIPESTATUS[0]}` is bash, this shell
is zsh, and a check printing `EXIT: ` reads as a pass.

⚠⚠ **Sabotage discipline. Nine sabotages passed while proving nothing in the two sessions before
this wave, three of them the FIXTURE's fault rather than the guard's.** Every time: (1) grep that the
edit landed; (2) confirm a `test result:` line appeared at all; (3) confirm the **named** test
failed; (4) check the target — `-p pipeline --lib` for stages, `-p app --bin Rustroom` for panels;
(5) `cargo test` takes a **substring, not a regex**. **If a sabotage does not fail, suspect the
fixture before concluding the guard is redundant.**

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Create** `crates/pipeline/src/lut.rs` | `.cube` parsing, `Cube`, trilinear lookup. No GUI, no I/O beyond a `&str`. |
| **Create** `crates/pipeline/src/stages/creative_look.rs` | The stage: shaper in, lookup, shaper out, strength lerp. |
| **Modify** `crates/edit-model/src/lib.rs` | `Look`, `LutSpace`, `Edits.look` + the five exhaustive sites. |
| **Modify** `crates/pipeline/src/lib.rs` | Placement, in the existing `match edits.rendering` region. |
| **Modify** `crates/app/src/` | Looks directory discovery + the Rendering panel section. |

---

## Task 1: the parser and the cube

**Files:** create `crates/pipeline/src/lut.rs`; add `pub mod lut;` to `crates/pipeline/src/lib.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest legal cube: 2×2×2, the identity.
    fn identity_text(n: usize) -> String {
        let mut s = format!("LUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |i: usize| i as f32 / (n - 1) as f32;
                    s.push_str(&format!("{} {} {}\n", f(r), f(g), f(b)));
                }
            }
        }
        s
    }

    /// ⚠⚠ **Red varies fastest.** Getting the index order wrong produces a cube that parses, has
    /// the right entry count, and transposes the colour axes — a plausible wrong image, which is
    /// this project's worst failure shape. A non-identity fixture is what catches it.
    #[test]
    fn the_red_axis_varies_fastest() {
        // 2×2×2 where every entry is its own (r, g, b) index — so a transposition is visible.
        let text = "LUT_3D_SIZE 2\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
                    0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let cube = Cube::parse(text).expect("parses");
        assert_eq!(cube.size, 2);
        // Entry 1 is (r=1, g=0, b=0): red stepped first.
        assert_eq!(cube.at(1, 0, 0), [1.0, 0.0, 0.0]);
        assert_eq!(cube.at(0, 1, 0), [0.0, 1.0, 0.0]);
        assert_eq!(cube.at(0, 0, 1), [0.0, 0.0, 1.0]);
    }

    /// ⚠⚠ **An identity LUT is BIT-EXACTLY the identity.** `assert_eq!`, no tolerance: a shaper or
    /// an interpolator that is *nearly* right is exactly what a tolerance hides, and this is the
    /// test `docs/BACKLOG.md` names as the one available exact check.
    #[test]
    fn an_identity_cube_is_bit_exactly_the_identity() {
        let cube = Cube::parse(&identity_text(17)).expect("parses");
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0, 0.123_456] {
            let got = cube.lookup([v, v, v]);
            assert_eq!(got, [v, v, v], "identity lookup moved {v}");
        }
    }

    /// ⚠ A value above 1.0 clamps to the cube's edge. It must not wrap, extrapolate or produce NaN.
    #[test]
    fn a_value_above_one_clamps_to_the_cube_edge() {
        let cube = Cube::parse(&identity_text(2)).expect("parses");
        assert_eq!(cube.lookup([2.0, -1.0, 0.5]), [1.0, 0.0, 0.5]);
    }

    /// ⚠ A malformed file is REFUSED, never silently treated as the identity — a look that does
    /// nothing is indistinguishable from a look that is not applied.
    #[test]
    fn a_malformed_cube_is_refused() {
        assert!(Cube::parse("not a cube at all").is_err());
        assert!(Cube::parse("LUT_3D_SIZE 3\n0 0 0\n").is_err(), "too few entries");
        assert!(Cube::parse("LUT_3D_SIZE 0\n").is_err(), "degenerate size");
    }
}
```

- [ ] **Step 2: Run, expect a compile failure naming `Cube`**

```bash
cargo test -p pipeline --lib lut > /tmp/w5a-t1.log 2>&1; echo "EXIT=$?"
grep -E "^error" -A4 /tmp/w5a-t1.log | head -8
```

- [ ] **Step 3: Implement**

```rust
//! Parsing and sampling a 3-D lookup table in the `.cube` format.
//!
//! ⚠ Pure: text in, `Cube` out. No I/O and no GUI — reading the file is the app's job, so this is
//! testable with a string literal and needs no fixture on disk.

/// A 3-D lookup table: `size³` RGB entries with **red varying fastest**.
pub struct Cube {
    pub size: usize,
    entries: Vec<[f32; 3]>,
}

/// Why a `.cube` could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum CubeError {
    NoSize,
    BadSize(usize),
    WrongCount { want: usize, got: usize },
}

impl std::fmt::Display for CubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CubeError::NoSize => write!(f, "no LUT_3D_SIZE line"),
            CubeError::BadSize(n) => write!(f, "LUT_3D_SIZE {n} is not usable (need 2..=64)"),
            CubeError::WrongCount { want, got } => {
                write!(f, "expected {want} entries, found {got}")
            }
        }
    }
}

impl Cube {
    /// The largest cube we will read. 64³ is 262 144 entries; beyond that a file is more likely
    /// corrupt than intentional, and the allocation is the caller's memory.
    const MAX_SIZE: usize = 64;

    pub fn parse(text: &str) -> Result<Self, CubeError> {
        let mut size = None;
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            // ⚠ `#` comments and the metadata keywords this format allows are skipped rather than
            // refused: a real .cube commonly carries TITLE and DOMAIN_MIN/MAX, and refusing them
            // would reject files that are perfectly valid.
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                size = rest.trim().parse::<usize>().ok();
                continue;
            }
            if line.starts_with(char::is_alphabetic) {
                continue;
            }
            let v: Vec<f32> = line.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            if let [r, g, b] = v[..] {
                entries.push([r, g, b]);
            }
        }
        let size = size.ok_or(CubeError::NoSize)?;
        if !(2..=Self::MAX_SIZE).contains(&size) {
            return Err(CubeError::BadSize(size));
        }
        let want = size * size * size;
        if entries.len() != want {
            return Err(CubeError::WrongCount { want, got: entries.len() });
        }
        Ok(Self { size, entries })
    }

    /// The entry at integer cube coordinates. ⚠ Red varies fastest — the format's own ordering.
    pub fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.entries[(b * self.size + g) * self.size + r]
    }

    /// Trilinear sample. Inputs outside `[0, 1]` clamp to the cube's edge.
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size - 1;
        let mut lo = [0usize; 3];
        let mut fr = [0f32; 3];
        for c in 0..3 {
            // ⚠ The CLAMP is here, on the index. A `.cube` stores an absolute output colour, so
            // above 1.0 there is nothing to scale — the `ProfileHueSatMap` trick of clamping the
            // index and applying to the real value does NOT transfer, because that table stores a
            // scale. See the spec, §4.
            let x = (rgb[c].clamp(0.0, 1.0)) * n as f32;
            let i = (x.floor() as usize).min(n.saturating_sub(1));
            lo[c] = i;
            fr[c] = x - i as f32;
        }
        let hi = [
            (lo[0] + 1).min(n),
            (lo[1] + 1).min(n),
            (lo[2] + 1).min(n),
        ];
        let mut out = [0f32; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let c000 = self.at(lo[0], lo[1], lo[2])[i];
            let c100 = self.at(hi[0], lo[1], lo[2])[i];
            let c010 = self.at(lo[0], hi[1], lo[2])[i];
            let c110 = self.at(hi[0], hi[1], lo[2])[i];
            let c001 = self.at(lo[0], lo[1], hi[2])[i];
            let c101 = self.at(hi[0], lo[1], hi[2])[i];
            let c011 = self.at(lo[0], hi[1], hi[2])[i];
            let c111 = self.at(hi[0], hi[1], hi[2])[i];
            let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let c00 = lerp(c000, c100, fr[0]);
            let c10 = lerp(c010, c110, fr[0]);
            let c01 = lerp(c001, c101, fr[0]);
            let c11 = lerp(c011, c111, fr[0]);
            *o = lerp(lerp(c00, c10, fr[1]), lerp(c01, c11, fr[1]), fr[2]);
        }
        out
    }
}
```

- [ ] **Step 4: Run — all four pass**

```bash
cargo test -p pipeline --lib lut > /tmp/w5a-t2.log 2>&1; echo "EXIT=$?"; grep "^test result" /tmp/w5a-t2.log
```

⚠ If `an_identity_cube_is_bit_exactly_the_identity` fails, **do not add a tolerance.** The lerp at
`fr = 0` must return `a` exactly; `a + (b - a) * 0.0` does. If it fails, the indexing is wrong.

- [ ] **Step 5: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | `at()` indexes `(r * size + g) * size + b` | `the_red_axis_varies_fastest` |
| 2 | Drop the `.clamp(0.0, 1.0)` | `a_value_above_one_clamps_to_the_cube_edge` |
| 3 | `parse` returns `Ok` on a count mismatch | `a_malformed_cube_is_refused` |

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/w5a-c1.log 2>&1; echo "CLIPPY=$?"
git add -A && git commit -q -m "feat(pipeline): parse a .cube and sample it trilinearly"
```

---

## Task 2: `Look` and `LutSpace` in `edit-model`

**Files:** `crates/edit-model/src/lib.rs`

⚠⚠ **This is the contended surface.** Append the field at the **end** of `Edits` and at the **end**
of every exhaustive fixture — never mid-struct. Git merges appends cleanly and conflicts on
insertions almost every time.

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠⚠ Hand-written `Default`, not derived: `strength` is 100 at neutral, and the
    /// container-level `#[serde(default)]` fills a missing field from the struct's `Default` — so a
    /// derived one gives every partial sidecar `strength: 0`, a look silently doing nothing.
    /// `RadialGradient::default` and `ColorGrading::default` record this trap.
    #[test]
    fn a_looks_default_strength_is_full() {
        assert_eq!(Look::default().strength, 100.0);
        assert_eq!(Look::default().file, None);
    }

    /// ⚠ A look with no file is inert, so `Edits::default()` stays strictly neutral and every
    /// golden is untouched.
    #[test]
    fn a_default_look_is_not_active() {
        assert!(!Look::default().is_active());
    }

    /// ⚠ Strength 0 is not active either — it is the identity by definition, and inserting a stage
    /// that does nothing costs a whole-image pass.
    #[test]
    fn a_look_at_zero_strength_is_not_active() {
        let l = Look {
            file: Some("x.cube".into()),
            strength: 0.0,
            ..Look::default()
        };
        assert!(!l.is_active());
    }
```

- [ ] **Step 2: Run, expect a compile failure naming `Look`.**

- [ ] **Step 3: Implement**

```rust
/// The colour space a `.cube` was authored against.
///
/// ⚠⚠ **This decides WHERE the look runs**, not merely how it is shaped — see the spec, §2. A
/// display-referred LUT modifies the camera rendering; a linear one runs in place of it, because it
/// IS the rendering. Stacking a film emulation on our S-curve double-renders.
///
/// ⚠ Log encodings (LogC, S-Log3, ACEScct) are deliberately absent: each is a specific transfer
/// function needing validation, and there is no footage here to validate against. Add a variant
/// when there is something to check it with — never before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LutSpace {
    /// sRGB primaries and transfer curve. What most downloadable looks expect.
    #[default]
    SrgbDisplay,
    /// Rec.709 primaries with its own transfer curve — NOT sRGB's, though they are often confused.
    Rec709,
    /// Scene-linear. A LUT declaring this **is** the display rendering.
    Linear,
}

/// A creative look: a 3-D LUT applied as a grade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    /// The `.cube`'s filename, resolved against the looks directory.
    ///
    /// ⚠ A NAME, not a path: a preset carrying a look must be shareable, and an absolute path is
    /// machine-specific. Camera profiles already behave this way.
    pub file: Option<String>,
    pub space: LutSpace,
    /// 0..=100. 100 is the LUT exactly as authored.
    pub strength: f32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            file: None,
            space: LutSpace::default(),
            strength: 100.0,
        }
    }
}

impl Look {
    pub fn is_active(&self) -> bool {
        self.file.is_some() && self.strength > 0.0
    }
}
```

Then, **at the end** of `Edits`:

```rust
    /// A creative look. ⚠ Rides the `Rendering` group — a look is a rendering decision, not a
    /// Color adjustment.
    pub look: Look,
```

- [ ] **Step 4: Follow the compiler through the five exhaustive sites**

The compiler names each. Add `look` **at the end** of every one:

1. `merge_from`'s destructure → assign it under the **`Rendering`** group (`EDIT_GROUP_NAMES` index 9)
2. `EditGroups` — no new group; it joins `rendering`
3. `pipeline::all_edits()` (`crates/pipeline/src/lib.rs:809`)
4. `edit_model::all_nonneutral()` (`crates/edit-model/src/lib.rs:2015`)
5. `app::edit_clipboard`'s payload test

⚠ These are exhaustive **on purpose** — they exist so a new field is a compile error until someone
decides what it means. Do not weaken any of them.

- [ ] **Step 5: Run the whole suite — 53 goldens must be byte-identical**

```bash
cargo test --all > /tmp/w5a-t3.log 2>&1; echo "TEST=$?" >> /tmp/w5a-t3.log
grep "^test result:" /tmp/w5a-t3.log | awk '{p+=$4; f+=$6} END {print p, f}'
grep -c "^test golden" /tmp/w5a-t3.log
```

⚠ `Edits::default()` must stay strictly neutral, so **no golden may move**. If one does, `look` is
active by default and the `Default` is wrong.

- [ ] **Step 6: Sabotage** — make `Look::default()` derived; `a_looks_default_strength_is_full` must
  fail. Revert.

- [ ] **Step 7: Commit.**

---

## Task 3: the stage

**Files:** create `crates/pipeline/src/stages/creative_look.rs`; register it in `stages/mod.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
    /// ⚠⚠ Strength 0 is BIT-EXACTLY the input. `assert_eq!`: a lerp that is nearly the identity is
    /// what a tolerance hides.
    #[test]
    fn strength_zero_is_bit_exactly_the_input() { /* build a 1×1 image, apply, assert_eq! */ }

    /// ⚠ And strength 100 is bit-exactly the LUT's own output, so the ends of the slider mean what
    /// they say.
    #[test]
    fn strength_one_hundred_is_bit_exactly_the_lut() { /* as above */ }

    /// ⚠⚠ The shaper round-trips. An identity LUT declared sRGB must leave a linear image
    /// bit-unchanged: in → sRGB → identity → back → out. If the shaper converts into the wrong
    /// space, this is *nearly* the identity, which is exactly the failure a tolerance would hide.
    #[test]
    fn an_identity_lut_through_the_srgb_shaper_is_the_identity() { /* assert_eq! per channel */ }
```

⚠ Write the bodies against `ImageBuf` the way `stages/display_rendering.rs`'s tests do — read that
file first for the fixture idiom rather than inventing one.

- [ ] **Step 2: Run, expect failure.**

- [ ] **Step 3: Implement the stage**

Shape: `CreativeLookStage::new(cube: Arc<Cube>, space: LutSpace, strength: f32)`, implementing
`PipelineStage`. Per pixel: shape into the declared space, look up, shape back, lerp by
`strength / 100`.

⚠ `LutSpace::Linear` needs **no shaper** — the data is already linear. That is the branch which makes
a linear LUT a *rendering* rather than a grade.

- [ ] **Step 4: Run — all three pass.**

- [ ] **Step 5: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Skip the inverse shaper | `an_identity_lut_through_the_srgb_shaper_is_the_identity` |
| 2 | `strength / 100.0` → `strength` | `strength_zero_is_bit_exactly_the_input` |
| 3 | Apply the shaper for `LutSpace::Linear` too | the identity test, in the linear case |

- [ ] **Step 6: Commit.**

---

## Task 4: placement

**Files:** `crates/pipeline/src/lib.rs`, in the `match edits.rendering` region (~line 577).

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠⚠ **Assert on the STAGE LIST, not on pixels.** The two placements are indistinguishable
    /// from output whenever a LUT happens to be gentle, so a pixel test would pass against the
    /// wrong one — and double-rendering a film emulation is precisely the bug this prevents.
    #[test]
    fn a_linear_lut_replaces_the_camera_rendering_and_a_display_lut_follows_it() {
        let names = |space: LutSpace| {
            let mut e = Edits::default();
            e.rendering = Rendering::Camera;
            e.look = Look { file: Some("x.cube".into()), space, strength: 100.0 };
            stage_names(&e)   // the existing helper these tests use for order
        };
        let display = names(LutSpace::SrgbDisplay);
        assert!(
            display.windows(2).any(|w| w[0] == "display_rendering" && w[1] == "creative_look"),
            "a display-referred look must FOLLOW the rendering: {display:?}"
        );
        let linear = names(LutSpace::Linear);
        assert!(
            !linear.contains(&"display_rendering"),
            "a linear look IS the rendering and must replace it: {linear:?}"
        );
        assert!(linear.contains(&"creative_look"));
    }
```

⚠ Find the existing order-asserting helper before writing this — six tests already name stage order,
and adding a second helper is how two come to disagree.

- [ ] **Step 2: Run, expect failure.**

- [ ] **Step 3: Implement** — in the tail, replace the `match edits.rendering` block so that a
  `Linear` look suppresses `DisplayRenderingStage` and a display-referred look is pushed after it.
  Both go **before** `OutputTransform`.

- [ ] **Step 4: Run. ⚠ All 53 goldens must still be byte-identical** — `Edits::default()` has no look.

- [ ] **Step 5: Sabotage** — always push after the rendering; the placement test must fail.

- [ ] **Step 6: Commit.**

---

## Task 5: discovery and the panel

**Files:** `crates/app/src/` — a `looks` module beside the camera-profile discovery, and the
Rendering section of `develop/panel/`.

- [ ] **Step 1:** Looks are discovered from `<config>/rustroom/looks/`, mirroring how camera
  profiles are found. ⚠ Read that code first and follow it rather than inventing a second
  convention; `dirs::config_dir()` is **not** `~/.config` on macOS.

- [ ] **Step 2: The panel** — in the Rendering section: a look dropdown, a space dropdown, a
  Strength slider.

⚠⚠ **The panel must say when the camera rendering is not in force.** Selecting a `Linear` look means
`DisplayRenderingStage` does not run; two renderings apparently both on with one silently ignored is
the invisible-state failure this project records repeatedly.

- [ ] **Step 3: A missing file renders WITHOUT the look and says so** in the status line — never
  silently, and never as an error that stops the photo opening.

- [ ] **Step 4:** Test through the panel harness (`test_ui::find` / `click_in_panel`), commit.

---

## Task 6: ⚠⚠ MEASURE AND REPORT — then STOP

**This task produces no code and ends the track's autonomous work.**

- [ ] **Step 1:** Obtain two or three real `.cube` files — a film emulation and a grade.

- [ ] **Step 2:** Run each through **every** `LutSpace`, on a real RAW, and record what each
  produces: whether it double-renders, clips, or looks as authored.

- [ ] **Step 3: Report the comparison and stop.** Do **not** tune a transfer constant. Do **not**
  decide a conversion "looks right".

⚠ `PARALLEL-WORK.md` §6: *an agent given an open design choice does not stall — it picks one silently
and the choice ships.* This is the planned checkpoint that prevents that, and it is the same shape as
W1's M7.3a resampler track.

---

## Task 7: verify

- [ ] `fmt --check`, `clippy --all-targets --all-features -D warnings`, `test --all` — **all three,
  each exit code read from a file.** `cargo test` being green is not evidence about clippy; that has
  caught this project out eight times.
- [ ] `git diff --stat main -- crates/pipeline/tests` — goldens untouched unless this track
  deliberately appended one, and 53 must have **run**.
- [ ] State the test arithmetic.
- [ ] Report: the §6 comparison; any sabotage that did not fail, **named as such**; and any file over
  ~600 lines you added to.
