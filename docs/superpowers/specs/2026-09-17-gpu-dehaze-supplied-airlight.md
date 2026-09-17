# The GPU dehaze takes the supplied airlight

**Status:** spec · 2026-09-17
**Found by:** using it, and the report diagnosed it. *"It does not breathe for the masked one, but
it does slightly for the global one, especially for straighten values above 25 or below -25."*

## The defect

`GpuDehazeStage::new` estimates its own airlight from the image it is handed:

```rust
// crates/gpu-pipeline/src/dehaze.rs:198
airlight: pipeline::airlight(image),
```

That image is the **rotated** frame, so the angle feeds straight into the estimate. It also builds
its CPU fallback with `DehazeStage::new` — the `None`-airlight constructor — so the fallback
re-estimates too.

⚠⚠ **The module header claims the two paths are *"identical by construction"*, and that claim is
true for SCALE and false for GEOMETRY.** The estimate is taken on a fixed 256 px downscale, which
makes it render-scale-independent — a real and deliberate property. Nothing makes it independent of
what is *in* the frame.

## Why this is the reported defect, point for point

| reported | explained by |
|---|---|
| masked does **not** breathe | a mask's capture stages run in the **prefix**, and `accelerate` is only ever called on the **tail**, so the masked dehaze stays on the CPU — where it now receives the invariant airlight |
| global **does** breathe | the global dehaze **is** accelerated, so it re-estimates from the rotated frame |
| worse past ±25° | bigger empty corners, so the image it estimates from changes more |
| during the drag **and** between settled values | it re-estimates on every construction, not only during a gesture |

⚠ And it explains three negative results: every probe written today measured the **CPU** path, which
after this morning's change genuinely is invariant. On `DSCF6227` the CPU counterfactual moves only
**0.08–0.44 levels** — that frame could not have shown the defect through the code being probed.
**Ask which path is rendering before measuring one.**

## Design

**`accelerate_prefix_stage` gains an `airlight: [f32; 3]`,** joining the `LensShot` and the
`Resampler` its own comment already describes as *"two facts `Edits` cannot carry … both in hand at
every call site"*. The airlight is a third, and both production callers — `render_engine.rs:1067`
and `batch_export.rs:421` — already compute it **unconditionally**.

**`GpuDehazeStage::new` takes the value instead of the image**, and builds its CPU fallback with
`DehazeStage::with_airlight`.

⚠⚠ **Required, not `Option`.** A `None` arm would be a silent fallback to estimating — which is
exactly how this defect survived. Making it unrepresentable beats documenting it.

⚠ **Consequence worth noticing: the seam no longer needs to hand over the buffer for this stage.**
`new`'s own comment says it *"cannot be constructed from `Edits` alone — which is why the prefix
seam hands the buffer over"*. With the airlight supplied, that reason is gone.

## ⚠⚠ A test pins the defect and must be re-expressed, deliberately

`the_airlight_is_the_cpus_own` asserts `stage.airlight == pipeline::airlight(&img)`. Its **intent**
is right — the two paths must not hold two different estimates — but its **expression** now pins the
bug. It becomes: the GPU stage carries **the supplied value**, and the CPU twin beside it carries
the same one.

This is the second test today whose subject a milestone deliberately removed; the first was
`the_replay_moves_when_an_upstream_edit_moves`, which used a crop as its discriminator.

## Done when

1. `accelerate_prefix_stage` takes `airlight: [f32; 3]` and passes it to `GpuDehazeStage::new`.
2. `GpuDehazeStage::new` uses it, and builds its CPU fallback with `with_airlight`.
3. Every caller passes the value it already holds — the two production ones and the examples.
4. `the_airlight_is_the_cpus_own` re-expressed against the supplied value.
5. **A test that the GPU stage's airlight does not move with the angle**, which is the property the
   report is about and which no existing test covers.
6. **A sabotage step:** putting `pipeline::airlight(image)` back must fail that test.
7. `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green together,
   **each exit code read from its own command, and read BEFORE committing** — a lint was committed
   red earlier today because the check and the commit shared a command.
8. **74 goldens ran and none moved.** ⚠ The goldens are CPU-path, so this predicts no movement; if
   one moves, something other than the GPU stage changed.
9. ⚠ Confirmed **by use, with the GPU on**, on `DSCF6227`: dehaze it and drag Straighten past ±25°.
   The tone must not breathe, during the drag or between settled positions.

## Risk

⚠ The failure mode stays a **plausible photograph**. And the GPU path has no golden coverage, so
items 5, 6 and 9 are the whole guard — a green workspace says nothing about this stage.
