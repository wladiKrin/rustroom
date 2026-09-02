# M7.6 (part) — Three tool gaps that need no `Edits` change

*Design, 2026-09-02. Wave 3, track D.*

Three of M7.6's six items, chosen because none of them touches `Edits`, the canvas, or
`adjustments.rs` — so D cannot collide with A (which owns `Edits` and stage order this wave) or C
(which owns `adjustments.rs`). The other three are scheduled elsewhere: grading-inside-masks needs
`Edits` (Wave 4), auto-mask is canvas work (Wave 4), and dehaze's bright-achromatic guard has no
design yet and gets its own brainstorm.

⚠⚠ **Two of these move existing goldens, and you are the only track allowed to move any this
wave.** A adds two new cases; you merge **first** so A rebases onto your PNGs. Every golden you do
*not* name below must stay **byte-identical**, and you check that with `git status` on the PNG
directory rather than by reading the test output.

---

## 1. A halo suppressor for Clarity

**The problem** (backlog): Clarity at +70 puts a halo along every hard edge, because the coarse
band `d_c = luma − blur₄₀(luma)` is largest exactly at an edge. The midtone weight `w(l)` mitigates
it and is not enough.

**The fix is the one `sharpening.rs` already uses**, verbatim in form: a suppressor
`supp = s + (1 − s) / (1 + (|d_c| / CLARITY_HALO_KNEE)²)` applied to the clarity delta before it
is summed. At a hard edge `|d_c|` is far above the knee and the contribution collapses toward `s`;
in texture and gentle gradients it is below the knee and passes at full strength.

- **No slider, no `Edits` field.** Lightroom has no halo control either; it is part of what
  "Clarity" means. `s` is a fixed constant — start at **0.25** — and `CLARITY_HALO_KNEE` a second,
  both named and both `pub(crate)` so the tests can reach them.
- ⚠ The knee is in **luminance delta units on the coarse band**, which is a different scale from
  sharpening's `HALO_KNEE = 0.05` on the fine band — do not copy the number, derive it: measure
  `|d_c|` at the golden chart's hard edge and in its texture region and put the knee between them.
- **Texture is untouched.** The fine band cannot reach the clip on its own (the module doc says
  why), so `texture_plus`/`texture_minus` must stay byte-identical — that is the guard that the
  suppressor landed on the right band.
- `clarity_plus` and `clarity_minus` **move**. Re-author with measurement: at the chart's hard
  edge the overshoot (the max above the flat band's value) must **fall**, and the fine-texture
  contrast must be retained within **10 %**. Report both numbers before and after. A golden nobody
  measured is a hash.

**Tests:** edge overshoot drops with the suppressor and not without (two runs, compared); fine
texture within 10 %; `s = 1` is bit-identical to the old behaviour (the suppressor is inert at
full pass-through — the sanity check that the plumbing did not change the maths).

---

## 2. Edge-aware negative Texture

**The problem** (backlog): negative Texture softens *edges* along with texture, because the fine
band is `luma − gaussian₂(luma)` and a Gaussian blurs edges too.

**The fix:** for `k_t < 0` **only**, compute the fine band against a **guided-filter base** —
`spatial::guided_filter(luma, luma, r, eps)`, the edge-preserving smoother noise reduction already
uses — instead of the Gaussian. The base then keeps edges, so `d_t` is near zero across an edge and
the negative half removes texture without smearing it. The **positive half is unchanged**: a
Gaussian base is what makes positive texture crisp *along* edges, and it must stay.

- ⚠⚠ **This makes the slider behave differently either side of zero**, which the backlog records as
  the cost. Accepted (user's call, this wave), and it must be *written down* in the module doc and
  the slider's hover text — a rule the user cannot see is one they cannot correct.
- Radius and `eps`: `r` from `TEXTURE_SIGMA` via the same σ→r rule `wide_blur` uses, `eps` a named
  constant; start at **1e-3** (the NR default) and measure.
- `texture_minus` **moves**; `texture_plus` and both clarity goldens must be **byte-identical** —
  the guard that the positive half and the other band are untouched. Re-author `texture_minus`
  with measurement: the edge's contrast under −70 must be retained within **5 %** of the untouched
  edge, where the Gaussian version loses far more (report the old figure).

**Tests:** a hard edge under negative texture keeps its contrast (compare against the Gaussian
version, which must lose more — assert *both*); fine texture is still removed (its span falls);
positive texture is bit-identical to before; `k_t = 0` is an exact identity (already tested; keep
it green).

---

## 3. The grading ranges, visualised

**The problem** (backlog, and recorded in the grading section's caveats): Blending and Balance
have no visual feedback — where the shadow/midtone/highlight ranges start and end is judged only by
their effect on the photograph.

**The fix:** in `develop/panel/color_wheel.rs`, under the `Shad │ Mid │ High │ Global` selector, a
strip ~10 pt tall spanning the panel width, painted by lightness: at each x, `l = x / width`, the
three range weights tint the pixel — shadows in one hue, midtones another, highlights a third,
mixed by weight — so Blending visibly widens the overlaps and Balance visibly slides all three.

- ⚠⚠ **Call the stage's own `range_weights(l, blending, balance)`**, made `pub` in
  `pipeline::stages::color_grading`. Re-deriving the Gaussians in the widget is how the strip would
  come to *lie* about the ranges — the exact failure this feature exists to prevent. That is the
  one visibility change you make in `pipeline`.
- One textured quad, not a mesh — the brush tint's lesson. 256 samples across is plenty.
- The three tints are fixed, low-chroma colours, **not** the wheels' current hues: the strip shows
  *where* the ranges are, and colouring it by the grade would confuse the two. Not the theme
  accent either (it is user-switchable, including `Neutral`).
- Drawn only while the Color section is open, inside `color_wheel::show`'s caller path — you do
  **not** edit `adjustments.rs`; if the strip needs the section's `blending`/`balance`, `show`
  already receives the `GradeWheel`, and the two sliders live beside it — thread them through
  `color_wheel`'s own signature.

**Tests:** the strip's weights at a sample of `l` values equal `range_weights`'s (sabotage:
hardcode the weights in the widget — must fail); the strip changes when Blending or Balance
changes (a headless `test_ui` draw, comparing sampled colours); a probe of every glyph or label the
strip introduces through `every_*_glyph_is_renderable` if it introduces any.

---

## 4. Ownership and order

**Yours:** `pipeline/src/stages/local_contrast.rs`, `pipeline/src/stages/color_grading.rs`
(visibility of `range_weights` only), `develop/panel/color_wheel.rs`, the three golden PNGs named
above (`clarity_plus`, `clarity_minus`, `texture_minus`), the hover text for the Texture slider
(one string in `adjustments.rs` — ⚠ that is the **only** line you touch there, and say so in your
report).

**Not yours:** `Edits`, `pipeline/src/lib.rs`, `golden.rs` (you regenerate PNGs through the
existing cases; you add none), the canvas, `dehaze.rs`, the brush.

**Merge first** in Wave 3, because you move goldens and A adds them.

---

## 5. Accepted caveats

1. The halo suppressor's strength is a constant, not a control.
2. Negative Texture is edge-aware and positive Texture is not — deliberately asymmetric, and said
   so in the UI.
3. The range strip shows lightness ranges in fixed tints, not the grade's own colours.
4. Dehaze's bright-achromatic over-correction, grading inside masks, and auto-mask are not here.
