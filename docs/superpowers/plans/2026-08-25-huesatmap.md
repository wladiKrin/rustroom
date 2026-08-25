# The profile's HueSatMap (C3) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply a DCP's dual-illuminant `ProfileHueSatMap` after C2's matrix, so a camera profile finally changes what you see.

**Architecture:** `camera-profile` parses the table; `color::huesatmap` converts to HSV, interpolates the table and converts back; `raw-decode` applies it per pixel after the matrix. Still opt-in via `camera_profile`, so nothing existing changes.

**Tech Stack:** Rust, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-25-huesatmap-design.md`

**Before anything:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
git checkout -b huesatmap
```

```
DCP=/Applications/RawTherapee.app/Contents/Resources/share/dcpprofiles/FUJIFILM X-T4.dcp
RAW=~/Desktop/fuji/DSCF6509.RAF
```

---

## File Structure

| file | responsibility |
|---|---|
| `crates/camera-profile/src/lib.rs` | *modify* — parse `HueSatMapDims` and both data tables |
| `crates/color/src/huesatmap.rs` | **create** — HSV conversion and table interpolation |
| `crates/color/src/lib.rs` | *modify* — declare the module |
| `crates/raw-decode/src/backend_rsraw.rs` | *modify* — apply it after the matrix |

---

### Task 1: parse the table

- [ ] **Step 1: Write the failing tests**

Add to `crates/camera-profile/src/lib.rs`'s tests:

```rust
    #[test]
    fn the_x_t4_hue_sat_map_parses_to_its_known_values() {
        // ⚠ Values from the independent Python reader, as with the matrices.
        let Some(path) = fixture() else {
            eprintln!("skipping: set DCP_FIXTURE=/abs/path/to.dcp");
            return;
        };
        let p = CameraProfile::from_file(&path).expect("parse");
        let m = p.hue_sat_map.as_ref().expect("hue sat map");
        assert_eq!(m.dims, [90, 30, 1]);
        assert_eq!(m.data1.len(), 90 * 30 * 1);
        assert_eq!(m.data2.as_ref().map(|d| d.len()), Some(90 * 30));

        // ⚠ Entry 0 is hue 0 / saturation 0, and it is the identity — a profile that shifted a
        // *neutral* would be malformed. A good first assertion because it is meaningful, not
        // merely a number that happens to be there.
        assert_eq!(m.data1[0], [0.0, 1.0, 1.0]);
        let e1 = m.data1[1];
        assert!((e1[0] - -12.117493).abs() < 1e-4, "hue shift {}", e1[0]);
        assert!((e1[1] - 1.07087).abs() < 1e-5, "sat scale {}", e1[1]);

        let mid = m.data1[30 * 30 + 10];
        assert!((mid[0] - -19.264469).abs() < 1e-4, "hue shift {}", mid[0]);
        assert!((mid[1] - 1.586781).abs() < 1e-5, "sat scale {}", mid[1]);

        let d2 = m.data2.as_ref().unwrap();
        assert!((d2[1][0] - -16.583088).abs() < 1e-4, "illuminant 2 differs from 1");
    }
```

- [ ] **Step 2: Run to verify it fails**, then implement

Add to `camera-profile`:

```rust
/// A DNG hue/saturation/value mapping table.
#[derive(Debug, Clone, PartialEq)]
pub struct HueSatMap {
    /// `[hue, saturation, value]` divisions. ⚠ Value is often 1, making the table 2-D.
    pub dims: [usize; 3],
    /// `[hue shift in degrees, saturation scale, value scale]` per entry, hue-major.
    pub data1: Vec<[f32; 3]>,
    /// The second illuminant's table, when the profile is dual-illuminant.
    pub data2: Option<Vec<[f32; 3]>>,
}
```

with `pub hue_sat_map: Option<HueSatMap>` on `CameraProfile`, parsed from tags 50937 (dims,
LONG×3), 50938 and 50939 (FLOAT, `3 × h × s × v` values).

⚠ A table whose length does not match its dims is **rejected** (the whole map becomes `None`)
rather than indexed into — a short table would read past its end or silently mis-map.

- [ ] **Step 3: Sabotage**

| sabotage | must fail |
|---|---|
| read the entries as `[sat, hue, val]` | `the_x_t4_hue_sat_map_parses_to_its_known_values` |
| use `data1` for `data2` | the illuminant-2 assertion |

- [ ] **Step 4: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add -A && git commit -m "feat(camera-profile): parse the HueSatMap"
```

---

### Task 2: apply the table

- [ ] **Step 1: Write the failing tests** in a new `crates/color/src/huesatmap.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn identity(h: usize, s: usize) -> Table {
        Table { dims: [h, s, 1], data: vec![[0.0, 1.0, 1.0]; h * s] }
    }

    #[test]
    fn the_hsv_round_trip_is_the_identity() {
        // ⚠ Includes v > 1: the buffer is scene-referred, so highlights above 1.0 are ordinary
        // and a round trip that clamped them would destroy the headroom piece A exists for.
        for c in [[0.2, 0.5, 0.9], [1.4, 0.3, 0.1], [0.5, 0.5, 0.5], [0.0, 0.0, 0.0]] {
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-5, "{c:?} round-tripped to {back:?}");
            }
        }
    }

    #[test]
    fn an_identity_table_changes_nothing() {
        for c in [[0.2, 0.5, 0.9], [1.4, 0.3, 0.1], [0.9, 0.9, 0.9]] {
            let out = identity(90, 30).apply(c);
            for i in 0..3 {
                assert!((out[i] - c[i]).abs() < 1e-5, "{c:?} became {out:?}");
            }
        }
    }

    #[test]
    fn a_neutral_is_never_moved() {
        // ⚠ Structural: saturation is 0, so the scale multiplies zero and the hue is undefined.
        // Tested anyway — if a grey ever moves, the HSV round trip is wrong, not the table.
        let mut t = identity(90, 30);
        for e in t.data.iter_mut() {
            *e = [37.0, 1.8, 1.0];
        }
        for v in [0.0, 0.18, 0.5, 1.0, 2.5] {
            let out = t.apply([v, v, v]);
            assert!(
                (out[0] - v).abs() < 1e-5 && (out[1] - v).abs() < 1e-5 && (out[2] - v).abs() < 1e-5,
                "grey {v} became {out:?}"
            );
        }
    }

    #[test]
    fn a_value_above_one_keeps_its_headroom() {
        // ⚠⚠ The specific regression the spec names: the table *index* clamps at the top, but
        // the scale must apply to the real value. Clamping the value itself would drag every
        // highlight back to the table's edge and quietly compress the scene-referred headroom.
        let mut t = identity(90, 30);
        for e in t.data.iter_mut() {
            *e = [0.0, 1.0, 1.0];
        }
        let out = t.apply([2.5, 1.0, 1.0]);
        assert!(out[0] > 2.4, "a highlight at 2.5 came back as {}", out[0]);
    }

    #[test]
    fn a_uniform_saturation_scale_is_applied() {
        let mut t = identity(90, 30);
        for e in t.data.iter_mut() {
            *e = [0.0, 2.0, 1.0];
        }
        // A colour at saturation 0.25 should come back at 0.5.
        let out = t.apply([1.0, 0.75, 0.75]);
        let hsv = rgb_to_hsv(out);
        assert!((hsv[1] - 0.5).abs() < 1e-4, "saturation became {}", hsv[1]);
    }

    #[test]
    fn a_uniform_hue_shift_rotates_by_that_many_degrees() {
        let mut t = identity(90, 30);
        for e in t.data.iter_mut() {
            *e = [60.0, 1.0, 1.0];
        }
        // Pure red is hue 0; +60° is yellow.
        let hsv = rgb_to_hsv(t.apply([1.0, 0.0, 0.0]));
        assert!((hsv[0] - 60.0).abs() < 0.5, "hue became {}", hsv[0]);
    }

    #[test]
    fn the_hue_index_wraps_and_saturation_clamps() {
        // ⚠ Hue is circular so its index wraps; saturation is not, so it clamps. Getting either
        // backwards shows as a seam at hue 0 or a panic at saturation 1.
        let mut t = identity(4, 2);
        t.data[0] = [0.0, 1.0, 1.0];
        t.data[1] = [0.0, 1.0, 1.0];
        for c in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 1.0]] {
            let _ = t.apply(c); // must not panic
        }
    }
}
```

- [ ] **Step 2: Implement**

`Table { dims, data }` with `apply(rgb) -> rgb`, plus `rgb_to_hsv`/`hsv_to_rgb` (hue in degrees,
`v = max(r,g,b)` unclamped), bilinear interpolation over hue and saturation with hue wrapping, and
a `blend(a, b, g)` for the dual-illuminant case that interpolates entry-wise.

⚠ `valDivisions > 1` must be handled (other cameras have it) even though the tested path is 2-D.

- [ ] **Step 3: Sabotage**

| sabotage | must fail |
|---|---|
| clamp `v` to 1.0 in `rgb_to_hsv` | `a_value_above_one_keeps_its_headroom` |
| apply the hue shift in radians | `a_uniform_hue_shift_rotates_by_that_many_degrees` |
| clamp the hue index instead of wrapping | `the_hue_index_wraps_and_saturation_clamps` (add an assertion on a hue near 360°) |
| apply `satScale` additively | `a_uniform_saturation_scale_is_applied` |

- [ ] **Step 4: Commit**

---

### Task 3: use it in the decode

- [ ] **Step 1**: in `backend_rsraw.rs`, build the blended table alongside the matrix, using the
  **same** illuminant weight, and apply it inside the existing `map_pixels` after the matrix.

⚠ One `map_pixels`, not two: a second pass over a full-resolution buffer costs a second traversal
of 313 MB for nothing.

⚠ The table is blended **once**, outside the pixel loop. Blending per pixel would be 2700 entries
of work per pixel.

- [ ] **Step 2**: `cargo test --all` — **39 goldens byte-identical**, since `camera_profile`
  still defaults to `None`.

- [ ] **Step 3: Commit**

---

### Task 4: measure, look, and record

- [ ] **Step 1**: extend `examples/dcp_compare.rs` output — it already reports max, mean and
  signed per-channel means. Run it.

⚠ **Unlike C1 and C2, a large difference is the expected result.** The X-T4 table has a mean
|hue shift| of 11.3° and saturation scales up to 1.95.

- [ ] **Step 2: Look at it.** Render the same file with and without the profile:

```bash
cargo run --release -p rustroom -- render "$RAW" /tmp/plain.jpg
```

and a second render with the profile, then compare by eye. ⚠ This is the first slice of C where
seeing a difference is **correct**; judge whether it looks like a plausible camera rendering, not
whether it matches the old one.

- [ ] **Step 3: Update `CLAUDE.md`** — what landed, the ⚠ findings (the scope reduction from
  inspecting real profiles; why the tone curve and LookTable are excluded; the headroom clamp;
  the shared illuminant weight), the measured difference, the test count, and the caveats.
  Update **▶ NEXT ACTION** to **C4**.

- [ ] **Step 4: Commit.** ⚠ **Do not push.**

---

## Self-Review

**Spec coverage:** parse dims and both tables (Task 1) ✓; HSV round trip preserving `v > 1`
(Task 2) ✓; 2-D interpolation with hue wrap and saturation clamp (Task 2) ✓; neutrals unmoved
(Task 2) ✓; headroom preserved (Task 2) ✓; dual-illuminant blend on C2's weight (Tasks 2, 3) ✓;
opt-in unchanged (Task 3) ✓; measured and judged by eye (Task 4) ✓; tone curve and LookTable
excluded with reasons (spec, recorded in Task 4) ✓.

**Type consistency:** `HueSatMap { dims, data1, data2 }` in Task 1 is read in Task 3.
`Table { dims, data }` with `apply`, `rgb_to_hsv`, `hsv_to_rgb`, `blend` in Task 2 are used in
Task 3.
