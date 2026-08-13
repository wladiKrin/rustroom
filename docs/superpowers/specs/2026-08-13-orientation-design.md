# Rotate and flip — design

Date: 2026-08-13
Status: approved

## Goal

Geometry offers a ±45° straighten and a crop. There is no way to turn a photo a quarter turn or
mirror it — so a frame shot in the wrong orientation, or one that wants mirroring, cannot be
fixed at all.

Add the eight rigid transforms of a rectangle: quarter turns and mirroring.

**Not in any milestone.** The capability inventory lists "Transform / geometry: perspective,
rotate, crop, straighten, aspect ratios", but the P0–P4 table never schedules it. This is an
unscheduled gap beside the crop/straighten work that already shipped, not a deferral.

## 1. Two fields, eight states

```rust
/// A quarter turn, clockwise.
pub enum Quarter { Zero, Cw90, Cw180, Cw270 }

/// One of the eight rigid transforms of a rectangle: mirror about the vertical axis if
/// `mirror`, then turn by `rotate`.
pub struct Orientation { pub rotate: Quarter, pub mirror: bool }
```

⚠ **`mirror` is one flag, not two.** A *vertical* flip is a horizontal flip plus 180°, so
storing `flip_h` and `flip_v` separately would make four of the eight states representable two
ways — the invalid-state-unrepresentable argument that shaped `ExportFormat` and
`Decision::Cancel`. Flip-vertical is therefore a **derived action** in the UI, not a stored
field: four buttons over two fields.

The canonical order is **mirror first, then rotate**. Any convention works as long as
composition respects it; this one is stated so the composition rules below can be checked.

## 2. ⚠ Composition is non-abelian, and that is where this would silently break

Rotating composes trivially. Flipping *after* a rotation does not:

| action | effect on `(rotate, mirror)` |
|---|---|
| rotate CW | `rotate += 90` |
| rotate CCW | `rotate -= 90` |
| flip horizontal | `rotate = -rotate`, `mirror = !mirror` |
| flip vertical | flip horizontal, then rotate 180 |

The middle rule is the one that matters: `M ∘ rot_r = rot_{-r} ∘ M`. Get it wrong and every
button behaves correctly *in isolation* while producing nonsense in sequence — the kind of bug
that survives casual testing.

So the rules live in pure methods on `Orientation`, with a **property test rather than a table
of cases**: applying the generators in every order up to a useful depth must yield **exactly 8
distinct states**, and each generator must return to identity after 4 applications (rotate) or
2 (flip). A wrong composition rule collapses or inflates that count.

## 3. Pipeline order: orient → straighten → crop

Matching Lightroom, and for the same reasons the existing stage order has: orientation is a
lossless whole-image reframe, straighten is a fine adjustment *within* the oriented frame, and
the crop is expressed in oriented-and-straightened space.

It stays **inside `GeometryStage`** rather than becoming its own prefix step. A separate step
would earn its own checkpoint — 313 MB at full resolution — to save re-running a transform that
changes rarely and is cheap. Recorded as a follow-up if straighten-after-rotate ever feels slow.

**Lossless.** Quarter turns and mirroring are index permutations; no resampling, so no quality
cost and no interaction with the bilinear straighten beyond running first.

## 4. ⚠ The crop follows the rotation

The crop rect is normalised in the *oriented* frame. Rotate a cropped photo and, without
transforming the rect, the crop lands on entirely different content.

So a rotation also transforms the stored `CropRect`. In the unit square a clockwise turn maps
`(x, y) → (1 − y, x)`, so `[l, t, r, b] → [1 − b, l, 1 − t, r]`; a horizontal flip maps
`(x, y) → (1 − x, y)`, so `[l, t, r, b] → [1 − r, t, 1 − l, b]`.

Pinned by a round trip — four clockwise turns must return any rect to itself — and by a
content check: the left half becomes the top half after a clockwise turn.

## 5. The dimension swap is already handled

90° and 270° swap width and height, so `GeometryStage`'s output dimensions change. Checked
rather than assumed, this needs no work elsewhere:

- `view_region::fit_cap` uses the **buffer's** long edge, which a swap does not change.
- `k` compares two caps, so it is unaffected.
- `VignetteStage`'s `Region` is computed from the prefix output's actual dimensions.
- `fit_image_size` does change — and the canvas already derives it from the returned frame,
  which is exactly what the fit-cap fix established.

## 6. UI: the Crop panel becomes the Geometry panel

Four buttons beside Straighten: rotate left, rotate right, flip horizontal, flip vertical.

**The mode is renamed from Crop to Geometry**, label *and* the `crop_mode` field. A panel
offering rotation, mirroring, straightening and cropping is not a crop panel, and leaving the
code saying `crop` behind a button saying Geometry is the kind of drift that makes a file
harder to read later. Mechanical rename, 15 sites.

⚠ **`preview_edits` keeps resetting the crop and must *not* reset the orientation.** Blanking
the crop is what lets the whole frame show under the overlay; orientation is not a crop, and
resetting it would make the preview show a differently-oriented image from the one being edited.

Each button press is one `history.push`, so one Ctrl+Z undoes one turn.

## 7. Testing

| test | asserts |
|---|---|
| `the_generators_produce_exactly_eight_states` | the property of §2 — a wrong composition rule collapses or inflates the count |
| `each_generator_returns_to_identity` | rotate ×4, each flip ×2 |
| `flipping_after_a_rotation_negates_it` | the non-abelian rule directly, so a failure names itself |
| `a_vertical_flip_is_a_horizontal_flip_and_a_half_turn` | the derived action, so the UI's four buttons stay honest against two fields |
| `orienting_is_an_index_permutation` | pixel-exact against a hand-written expectation on a small asymmetric image; dimensions swap on quarter turns |
| `orienting_twice_by_180_is_the_identity` | round trip on real pixels, bit-identical |
| `a_clockwise_turn_moves_the_left_half_to_the_top` | the crop-rect transform of §4, by content |
| `four_crop_rotations_restore_the_rect` | crop round trip |
| `the_prefix_still_holds_geometry_noise_reduction_and_dehaze` | unchanged — orientation stays inside `GeometryStage` |
| goldens | two new (`orient_cw90`, `orient_mirror`); **the 29 existing stay byte-identical**, which is the proof the stage is inert at its default |

## Non-goals

- **Perspective correction.** A genuinely separate, larger feature.
- **Auto-rotation from EXIF.** Already applied at decode — by libraw's `flip` for RAW and
  `open_oriented` for JPEG/PNG/TIFF. This is a *user* rotation on top of that.
- **A separate prefix checkpoint for orientation**, per §3.
- **Aspect-preset awareness.** The crop tool's presets stay as they are; a 3:2 preset after a
  quarter turn simply means the frame's 3:2, which is what the numbers say.

## Accepted caveats

1. **Rotating requires entering Geometry mode**, as with Straighten today.
2. **Orientation is not free at full resolution** — a 313 MB index permutation, paid whenever
   the prefix is invalidated, not only when orientation changes.
3. **An existing crop is transformed, not preserved in content terms, under a *flip***: the rect
   mirrors with the frame, which is the only sensible reading, but a deliberately asymmetric
   crop will end up on the other side.
4. **Sidecars written before this change** load with the identity orientation via
   `#[serde(default)]`, so nothing existing is reinterpreted.
