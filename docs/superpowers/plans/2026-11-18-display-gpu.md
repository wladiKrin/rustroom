# The display's colours on the GPU — plan (2026-11-18)

Spec: `docs/superpowers/specs/2026-11-18-display-gpu-design.md`. Branch `display-gpu`, from `main`.

1. **The baseline first.** `probe_straighten_drag` takes `PROBE_DISPLAY=m27q|p3` (the M27Q from its captured EDID).
   Measure sRGB, M27Q and P3 on the old code at the 3200 rung: median drag frame and the tail frames after release.
2. **`color::display`:** `Display::gpu_table()` — a profile's own table, a fixed space's interned table sampled from
   its exact conversion, `None` for sRGB; `DisplayProfile::{table, n}` for the upload; `Eq`. Measure the fixed spaces'
   table error and pin the test around the number.
3. **`gpu-pipeline`:** `quantize::encode` takes the table and writes a second plane through a trilinear lookup
   mirroring `DisplayProfile::lookup`; `Download::Rgba8` carries the table; `run_rgba8*` take it and return both
   planes (`Rgba8 { srgb, display }`), read out of one mapped buffer. Tests: the display plane against the CPU within
   one level on the tie fixture (premise: it differs from sRGB), the sRGB plane unchanged, no table → no second plane.
4. **`app`:** `TailRun.display`; `byte_tail` and the handoff pass `display.gpu_table()`; `Packed::Rgba8.display`;
   `pack` takes the texture from it; `float_needed` and `handoff_ok` drop the display. Tests: a profile and a fixed
   space take the byte path (the old "keeps the float path" test inverted) and hand off; the carrier test for a profile
   (texture vs the float path ≤ 1 level, `srgb` vs an sRGB display's frame).
5. **Measure again** (step 1's rows), gate at `-j 2`, journal + `AGENTS.md`'s display rule, launch the app.
