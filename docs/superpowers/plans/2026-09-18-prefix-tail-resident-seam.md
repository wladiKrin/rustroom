# Plan — a GPU buffer resident across the prefix seams

Spec: `docs/superpowers/specs/2026-09-18-prefix-tail-resident-seam.md`
Branch: `prefix-tail-resident-seam` (already carries the probe + `RUSTROOM_DEBUG_XFER`)

## Task 1 — a second slot on `GpuContext`

`crates/gpu-pipeline/src/context.rs`: add `prefix_plane: SharedPlane<GpuImage>` beside
`shared_plane`, with `pub fn prefix_plane(&self)`.

⚠⚠ The field doc must say **why two slots rather than one**, naming
`the_outer_tails_own_chain_never_takes_a_masks_leftover_plane`. A future reader will try to merge
them; the reason they cannot be merged is a sabotage this project already paid for.

## Task 2 — `GpuChain` learns which slot it means

`crates/gpu-pipeline/src/chain.rs`: a `Slot { Mask, Prefix }` enum; `offer_output(Slot)` and
`allow_shared_input(Slot)` take it; `run_with` reads/writes the named slot.

⚠ Keep `shares_input()` as the observable — M8.3n's note explains why the rule is observed directly
rather than through a render.

## Task 3 — `accelerate_prefix_stage` returns `Option<GpuChain>`

`crates/gpu-pipeline/src/accelerate.rs`. `GpuChain` implements `PipelineStage`, so callers box it
where a box is wanted. Update the existing callers and tests.

## Task 4 — `accelerate` is told the prefix offered

Group its arguments (clippy fired at 8 on `accelerate_prefix_stage`; do not silence the lint) and
set `allow_shared_input(Slot::Prefix)` on the tail's **first** chain when the flag is set.

⚠ The first chain, not the trailing one. For an unmasked tail they are the same chain; with a mask
the tail is `[chain, mask, chain]` and only the first may take.

## Task 5 — wire the loop in `render_engine`

`crates/app/src/render_engine.rs`, the prefix step loop:
- an accelerated step takes from `Slot::Prefix` and offers to it;
- ⚠⚠ the `None` arm — a CPU step — **clears** the prefix slot. Without it the next accelerated step
  takes a plane that is not its input: same dimensions, same thread, **a plausible photograph**;
- pass the flag to `accelerate` saying whether the last prefix step offered.

## Task 6 — guards, each with its sabotage

| test | sabotage that must fail it |
|---|---|
| a mask's leftover is never taken by a prefix consumer | cross-wire the slots |
| a prefix offer is never taken by a mask's sub-chain | cross-wire the slots |
| a CPU step between two accelerated ones clears | delete the `None`-arm clear |
| the take fires at all | force a miss — a **fixture check** must fail, not a pixel test |
| equivalence: two accelerated steps + tail, taking on vs off | — |

⚠⚠ **Measure the equivalence band at both ends before writing the bound.** M8.13's first attempt
borrowed `< 0.02` from a one-stage chain and failed at 0.088 for an unrelated reason.

⚠ A counter says a plane moved, never that it was the right plane. Every slot guard needs the
counter **and** the picture.

## Task 7 — measure, gate, confirm

- Re-run `probe_prefix_tail_seam` on a quiet machine; `took=true` and `memcpy≈0.2` on each
  consumer; saving in the 36–47 ms family.
- FMT / CLIPPY / TEST, **each exit code read from its own command, before committing**; 74 goldens.
- Launch the GUI. **Do not merge, do not push.**
