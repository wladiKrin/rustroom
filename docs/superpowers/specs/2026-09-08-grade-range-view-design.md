# Grade range view — design

**Status:** implemented 2026-09-08, confirmed by use. Branch `m7.6-grade-range-view`. ⚠ Two corrections made after this was written are marked in place: the tint (§6) and an inert sabotage (§7). The first of M7.6's five
remaining gaps (user's call, 2026-09-08).

A **"Show range"** checkbox in the Color Grading block paints, over the photograph, where the
range selected in the `Shad │ Mid │ High` selector reaches — as a translucent tint whose alpha
is that range's weight at each pixel. It is what Blending and Balance have never had: feedback
on the picture rather than on a legend.

⚠ **What already exists, so this is not built twice.** The panel has carried a *range strip*
since W3 (2026-09-02): a 15 pt band under the selector painted with the stage's own
`range_weights`, showing where the three ranges sit and how far they overlap. That answers *"what
does Blending 30 mean?"* in the abstract. It does not answer *"which pixels of this photograph
does the highlight wheel touch?"* — which is the gap this closes. The strip stays exactly as it
is, and its three legend tints are deliberately **not** reused here (§4).

---

## 1. The constraint that decides the design

The ranges are selected on **Oklab L at the grading stage's input**. The tail then runs, in
order: colour grading → the masks loop → vignette → sharpening → **the display rendering
S-curve** → the creative LUT → the output transform. Both the histogram and the packed pixels sit
on the far side of that S-curve.

So the two cheap designs are wrong by construction, not by tuning:

- **A view mode in `pack` from the output pixels** — decode sRGB back to L and paint. Wrong by
  the whole S-curve, by the LUT, by the grade's own luminance offsets and by any masked
  exposure. The boundaries would land visibly in the wrong place on every rendered photograph.
- **Markers on the histogram** — wrong axis. The histogram is post-S-curve sRGB and the ranges
  are pre-S-curve Oklab L. Mapping between them means pushing a grey through every stage after
  grading, and a marker that is right only for neutrals is a marker people will trust anyway.

⚠ This area's record is *four reasoned-wrong, measured-right findings in a row*, and the resume
block says to measure before theorising. This is the one place where the cheap version can be
ruled out **in advance**: the S-curve is not a tuning constant, it is a stage that moves every
pixel's L, and its size is measured (0.02× at −4 EV, 0.79× at +2.25). What still gets measured is
the cost (§3).

**Decision (2026-09-08, with the user):** a **tap at the grading stage's input**. The engine
runs the tail up to grading, reads each pixel's L there, computes the stage's own weights into an
alpha plane, then runs the rest of the tail unchanged. The photograph, the histogram, the colour
readout and the soft proof are untouched.

Two further decisions, same session: the overlay shows **the selected range only** (not all
three mixed — three low-chroma tints over a colour photograph do not read), and it is switched on
by a **checkbox** rather than appearing transiently during a slider drag.

## 2. The pipeline seam

`build_pipeline_split_in_region_with_brush` gains a tapped sibling:

```rust
/// The tail in two halves, split exactly where the grading stage goes.
pub struct TailHalves {
    /// Everything up to but excluding colour grading.
    pub before_grade: Pipeline,
    /// Colour grading (when active) and everything after it.
    pub from_grade: Pipeline,
}

pub fn build_pipeline_split_tapped(/* the same seven arguments */) -> (Pipeline, TailHalves);
```

The existing function becomes a delegating wrapper: call the tapped one, concatenate the halves.
⚠ **Every existing caller is therefore byte-identical by construction** — same stages, same
order, same constructors — and all 59 goldens must run and stay byte-identical, which the plan
checks rather than assumes.

⚠ The split point exists **whether or not grading is active**. `ColorGradingStage` is only pushed
when a wheel is non-zero, but Blending and Balance define the ranges regardless, and someone
setting them up before choosing a hue is exactly who wants to see them. So the split is
structural — "after HSL, before the masks loop" — not "find the stage named `color_grading`".

A test asserts that `before_grade ++ from_grade` has the same `stage_names` as the untapped tail
for both an active and an inactive grade, and that with a grade active `from_grade` begins with
`color_grading`.

## 3. The view, in the app lib

`crates/app/src/grade_view.rs`, beside `visualize.rs` and following its rule: **a view mode, not
an edit**. Never `Edits`, never the sidecar, never an export or the CLI. No egui.

```rust
/// Which range the view shows. ⚠ Three, not four: Global is a constant, not a range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GradeRangeView { Shadows, Midtones, Highlights }

/// One byte per pixel: the selected range's weight at that pixel's Oklab lightness, 0..=255.
pub fn coverage(buf: &ImageBuf, range: GradeRangeView, blending: f32, balance: f32) -> Vec<u8>;

/// The plane as RGBA for a texture: a fixed tint, alpha from the weight.
pub fn tint_rgba(plane: &[u8]) -> Vec<u8>;
```

`coverage` is `color::oklab::prophoto_to_oklab(px)[0]` per pixel, then
`ColorGradingStage::range_weights(l.clamp(0.0, 1.0), blending, balance)[range]`. Two things in
that line are load-bearing:

- **`prophoto_to_oklab`, the ProPhoto pair** — the buffer is in the working space, and the stage
  itself records that mixing the sRGB and ProPhoto pairs "produces a plausible-looking colour
  shift that is very hard to attribute later". Here it would misplace the ranges plausibly.
- **`clamp` on the index, exactly as the stage does at its table lookup** — M3.3's rule, *"the
  index clamps to [0,1]; the pixel's own L does not"*. A scene-referred pixel above white has
  `L > 1`; the stage reads it as pure highlight, and so must the view, or the two disagree on the
  brightest pixels — the ones a highlight wheel is for.

⚠ `range_weights` is already `pub` on `ColorGradingStage` (the strip's reason: *"never from a
copy"*). The view calls it, so the three things that show ranges — the stage, the strip and this —
cannot drift.

Rayon-parallel over rows, as `to_rgba8` is. The plane is at the **rendered** buffer's resolution,
which is bounded by the viewport (a fit rung or a region), so no separate cap is needed.

**Cost is measured, not estimated**, at fit and at a native region on a real X-T4 frame, and the
numbers go in `CLAUDE.md`. The comparable measurement is a masked tone curve's whole Oklab round
trip at **+5.4 ms** at fit; this is half a round trip and no table, so single digits are expected
— but this file records four estimates in this area that were wrong, so the number is what
counts.

## 4. The engine

`FrameSpec` gains `pub grade_view: Option<GradeRangeView>`, a view-only property of the
*request* like `visualize` and `soft_proof`: never `Edits`, never the sidecar, and it invalidates
no checkpoint, because `CacheKey` is `(PrefixKey, max_dim)` and the whole tap lives in the tail.
`RenderedFrame` gains `pub grade_view: Option<Arc<Vec<u8>>>`, the plane, `width × height` bytes.

In `render_cached`, beside the proof:

```rust
let (out, plane) = match grade_view {
    Some(range) => {
        let halves = build_tail_tapped(color::OutputSpace::Srgb);
        let mid = halves.before_grade.run(input);
        let g = &edits.color_grading;
        let plane = grade_view::coverage(&mid, range, g.blending, g.balance);
        (halves.from_grade.run(mid), Some(Arc::new(plane)))
    }
    None => (build_tail(color::OutputSpace::Srgb).run(input), None),
};
```

- ⚠ **`None` is the path that exists today** — one tail, one run, no clone — bit-exactly. The
  `Some` arm runs the same stages in the same order with one read between two halves, so the
  frame's pixels are identical too, and a test says so (§7).
- ⚠ **Blending and Balance come from `edits.color_grading`**, the same values the stage reads,
  and they are read whether or not the stage is active (§2).
- The plane's dimensions equal the frame's **by construction**: no stage in `from_grade` changes a
  buffer's size, so `mid` and `out` are the same shape. A test pins it on a region render anyway,
  because `ColorImage::from_rgba_unmultiplied` panics on a mismatch rather than reporting one.
- The **soft proof's second run** uses the untapped tail and is untouched. Visualize Spots is a
  post-step in `pack` and is untouched. The plane is taken before either, so it stays honest under
  both — though in practice they cannot coincide, since `select_tool` clears all three.
- `render_once` never taps, for the reason it is never proofed: it is the before/after baseline
  and the reference the cached path is tested against.
- Every exhaustive `FrameSpec { .. }` literal (two in `session_flow`, one in `multi/pane`,
  `FrameSpec::fit`, and five in the engine tests) gains `grade_view: None` — forced at compile
  time.

## 5. Session and panel

`Session` gains two fields, both view state and both reset per photo for free:

```rust
/// The "Show range" checkbox.
show_grade_range: bool,
/// What the request carries: the checkbox AND a non-Global selection, or `None`.
grade_view: Option<GradeRangeView>,
```

⚠ **Two fields, one derivation site, and the reason it is not one field.** The value on the wire
depends on two inputs — the checkbox and the `Shad │ Mid │ High │ Global` selector — and the
selector is `app.grade_range`, bin-side, which `Session::request` cannot see. Under Global the
wire value is `None` while the checkbox stays ticked; folding both into one `Option` would make
the box untick itself the moment Global was selected. So the panel derives, with a change guard,
in the one place both inputs are in scope:

```rust
let wanted = s.show_grade_range.then(|| grade_range.view()).flatten();
if s.grade_view != wanted { s.grade_view = wanted; ed.changed = true; }
```

where `GradeRange::view(self) -> Option<GradeRangeView>` maps the four-way selector onto the
three-way view (`Global => None`). Switching the selector while the box is ticked therefore
re-renders with the new range; selecting Global paints nothing and leaves the box ticked.

- `Session::request` passes `grade_view: self.grade_view`.
- `select_tool` clears both, **unconditionally**, beside `visualize` and for its recorded reason:
  a guarded clear was tried there and its sabotage went uncaught.
- Photo open: both false/`None`, in the `Session { .. }` literal — and in `test_app`'s, which
  is exhaustive too.
- The checkbox sits **below Balance**, the last of the two sliders it serves, labelled
  **"Show range"**, hover text: *"Tints the photograph where the selected range reaches. Affects
  only what you see — never the render, the export or the histogram."* `adjustments::show` gains
  `show_grade_range: &mut bool` and `grade_view: &mut Option<GradeRangeView>` — passed as the two
  session fields from the `Tool::Edit` arm, which already threads `s.picking_wb` the same way.

## 6. The canvas

`Session` gains `grade_view_texture: Option<egui::TextureHandle>`. It is set in the **one place a
frame lands** (`shell.rs`, where `hist`, `readout_bytes` and `texture` are taken from the frame):

```rust
s.grade_view_texture = frame.grade_view.as_ref().map(|plane| {
    let img = egui::ColorImage::from_rgba_unmultiplied(
        [frame.width, frame.height], &grade_view::tint_rgba(plane));
    ctx.load_texture("grade_view", img, egui::TextureOptions::LINEAR)
});
```

So the texture and the plane can never describe different frames, and a frame that arrives
without a plane — the box unticked, Global selected, a tool switch — drops the texture in the same
assignment. No separate clear.

**Painted at `img_rect`**, immediately after the frame texture and before every tool overlay. ⚠
`img_rect`, not `frame_rect` — the readout's exception and its reason: the plane covers *the
rendered slice*, which is exactly what `img_rect` spans, whether the frame was rendered whole or as
a region. Every other overlay is frame-relative; this one is frame-*sized*.

**The tint** is one fixed colour, alpha `weight / 2` — so full membership reads as a 50% tint
and the soft edge of a range reads as a soft edge, the brush tint's rule ("a weak stroke reads
as a weak tint — which is what makes Flow legible"). ⚠ **Not the strip's three legend tints** —
they are low-chroma by design so the strip reads as a legend, and at 50% over a photograph they
vanish.

⚠⚠ **Corrected by use (2026-09-08): the tint is the brush tint's red, `(255, 40, 40)`, not the
teal this section first chose.** Teal was picked so the two tints could never be confused. The
first GUI check reported it *"very subtle and a bit confusing"* — at the default Blending of 50
the weights sit around a third to a half over the whole frame, and a teal at half that alpha is
a faint wash rather than a region. Red at the same alpha rule reads at a glance and is the tint
people have already learned means "this is where the tool reaches"; the two are never on screen
together, since the brush tint is drawn under the Masks tool and this under Edit, and
`select_tool` clears this view on the way across. User's call.

## 7. Testing — and the sabotage each test exists to catch

**pipeline** — `the_tapped_tail_concatenates_to_the_tail`: stage names equal, grade active and
inactive; `from_grade` begins with `color_grading` when active. Sabotage: split one stage late.

**grade_view** (lib, no window):
- `a_grey_ramp_matches_the_stages_weights` — for greys `L = Y^(1/3)`; the plane equals
  `range_weights` to within one 8-bit level across the ramp, all three ranges.
- `a_chromatic_pixel_is_placed_by_oklab_lightness` — a saturated pixel whose `luminance()` and
  Oklab L put it in *different* ranges lands where L says. Sabotage: `color::luminance` fails it.
- `a_pixel_above_white_is_a_highlight` — `L > 1` reads as pure highlight, as the stage's clamp
  makes it. ⚠ **Corrected at plan time: the sabotage this bullet named — dropping the clamp — is
  inert.** The weights are normalised, so for `L > 1` the highlight weight is ≈1 clamped or not;
  the clamp is parity with the stage's *table index*, and the test pins the behaviour.
- `tint_rgba_is_the_fixed_tint_with_alpha_from_the_weight`, both ends.

**render_engine** (through the cached path, the proof tests' harness):
- `the_grade_view_does_not_touch_the_photograph` — `rgba`, `srgb` and `hist` byte-identical
  with the view on and off. Sabotage: painting the tint into `out` fails it.
- `the_plane_is_taken_before_the_grade` — a luminance wheel at +100 (`LUM_MAX` = 0.15 in L)
  leaves the plane identical to the wheel at 0. Sabotage: tapping after `ColorGradingStage`.
- `the_plane_is_taken_before_the_display_rendering` — with the rendering on, the plane equals
  the plane with it off. Sabotage: tapping after `DisplayRenderingStage`. ⚠ This is the test that
  encodes §1, and the one that would have failed the pack-side design outright.
- `the_plane_has_the_frames_dimensions` — on a **region** render, `plane.len() == w * h`.
- `a_view_off_request_is_the_old_path` — `grade_view: None` produces a frame with no plane.

**Headless, bin** (the `RawInput` harness the panel and canvas tests use):
- `ticking_show_range_issues_a_request_carrying_the_selected_range` — `requests_issued` rises
  by one and `s.grade_view == Some(Shadows)`.
- `switching_the_selector_re_requests_with_the_new_range`, and `global_carries_none_and_keeps_the_box_ticked`.
- `a_tool_switch_clears_the_view` — both fields, the `visualize` test's shape.
- `a_landed_plane_becomes_a_texture_painted_at_the_image_rect` — seed a frame with a plane, count
  meshes as the loupe tests do; and none when the plane is absent.

**Goldens**: all 59 run, none move — the tap is not in the pipeline and the untapped tail is
built from the same halves.

## 8. Where the code goes, and what is already oversized

| Change | File | Lines now |
|---|---|---|
| `TailHalves`, `build_pipeline_split_tapped`, the wrapper | `crates/pipeline/src/lib.rs` | large; one function and a struct |
| `GradeRangeView`, `coverage`, `tint_rgba` | `crates/app/src/grade_view.rs` | **new** |
| `FrameSpec.grade_view`, `RenderedFrame.grade_view`, the tap in `render_cached` | `crates/app/src/render_engine.rs` | **2594 ⚠** |
| `Session` fields, `request`, open literal | `crates/app/src/main.rs`, `session_flow.rs` | 917, 1065 |
| `select_tool` clear | `crates/app/src/interaction.rs` | 570 |
| checkbox and derivation | `crates/app/src/develop/panel/adjustments.rs` | **989 ⚠** |
| `GradeRange::view` | `crates/app/src/develop/panel/color_wheel.rs` | 598 |
| texture landing | `crates/app/src/shell.rs` | **1028 ⚠** |
| paint at `img_rect` | `crates/app/src/develop/canvas/mod.rs` | **1697 ⚠** |

⚠ Four of these already exceed the ~600-line mark and the report says so. Each gains a handful
of lines at an existing seam; nothing new is dumped into them — the logic is in `grade_view.rs`.

## 9. Documents

`CLAUDE.md`: a section *"The grade range view"* in the file's idiom, the resume block's state and
NEXT ACTION (the next of the five gaps), and the M3.3 section's accepted caveat (3) struck.
`docs/BACKLOG.md`: the range-boundaries line under M7.6 struck. The vault's
`03-plan-of-action.md`: M7.6's line, four gaps left. `01-lightroom-capabilities.md` does not
name this gap and needs nothing.

## 10. Accepted caveats

1. **One range at a time.** The user's call; the strip still shows all three together.
2. **No histogram feedback**, and by §1 none that would be honest without the tap; the canvas is
   the instrument.
3. **The plane is at render resolution** — at fit, the fit rung's pixels stretched to the
   screen, with a slightly soft tint edge. The brush tint accepts the same.
4. **One extra Oklab conversion per rendered pixel while on**, measured in the plan and paid only
   then; never optimised by folding the tap into the stage, which would put a view in the
   pipeline.
5. **A masked exposure does not move the ranges** — correct, since grading runs before the masks
   loop, but it may surprise: a face lifted by a mask keeps the range membership it had before
   the lift.
6. **Resets per photo and per tool**, like Visualize Spots.
