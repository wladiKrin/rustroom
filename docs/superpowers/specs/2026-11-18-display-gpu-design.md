# The display's colours on the GPU — design (2026-11-18)

**Asked for by the author** after the display's own colours merged ("then start on the GPU display transform for
monitor profiles"). The decisions below are the builder's, each with its reason; none changes what a user sees beyond
speed (the bytes agree with today's within one 8-bit level).

## What is wrong today

A non-sRGB `color::Display` — a monitor's profile under Automatic, or P3 / Adobe RGB chosen by hand — turns off M8.3g's
two GPU fast paths: the tail's last chain cannot download bytes (`render.rs`'s `float_needed`), and the last prefix step
cannot hand its output to the tail (`handoff_ok`). So every Develop frame on such a display downloads the float frame
(82 MB at the 3200 rung), quantizes it for the histogram, and converts it again on the CPU for the texture
(`pack`: 19.4 ms through a profile's table, ~51 ms through the P3 matrix).

## The design

1. **The GPU's last dispatch writes two planes** when the display is not sRGB: the sRGB bytes it writes today (the
   histogram, the colour readout, the clipping view — all image-referred, `pack`'s ordering) and the display's own
   bytes (the texture), converted from the same float pixel before quantizing. One buffer, one download of
   8 bytes a pixel instead of 12 of float.
2. **The table is the CPU's own** — the profile's `n³` entries uploaded as they are, looked up by the same trilinear
   arithmetic (`DisplayProfile::lookup`, line for line). Never a second sampling.
3. **A fixed space (P3, Adobe RGB) gets a table of the same size for the GPU**, sampled from its exact conversion and
   interned like a profile. The CPU path keeps the exact formula. ⚠ The table's error against the formula is
   MEASURED (`a_fixed_spaces_table_is_within_a_third_of_a_step`) — a third of a step is the bound the profile table
   was chosen by. One mechanism on the GPU rather than a matrix kernel beside a table kernel.
4. **Both fast paths open again for every display.** The three float consumers stay exactly as they are: Visualize
   Spots, soft proofing, the grade range view. `Packed::Rgba8` carries the display plane beside the sRGB one; on a
   GPU failure the frame falls back to the float path and `pack`'s CPU conversion, unchanged.
5. **Rounding and NaN follow the CPU.** The quantization is `quantize.rs`'s own `q`. A NaN in any channel makes the
   CPU's lookup NaN in all three (its fractions carry it), which quantizes to black — the shader tests for it and
   writes black. ⚠ The display plane is compared in LEVELS (≤ 1), not bit-exact: the trilinear blend is a chain of
   multiply-adds a GPU compiler may fuse. The sRGB plane stays bit-exact.
6. **Not on the GPU:** thumbnails and rendered previews — converted once per image, 8-bit, on the loaders' threads
   (`convert_thumb`); not a per-frame cost.

## Done when

- The GPU's display plane equals `ImageBuf::to_rgba8_with(display.convert)` within one level on a fixture carrying
  every quantization tie, a negative, a value above 1 and a NaN — and differs from the sRGB plane (a premise asserted
  first).
- A profile display takes the byte path and hands off; its texture equals the float path's within one level and its
  sRGB channel equals an sRGB display's frame.
- Measured through the drag probe (`RUSTROOM_DEBUG_RENDER`, `PROBE_DISPLAY`), before and after, on a real X-T4 frame
  at the 3200 rung: sRGB, the M27Q, P3.
