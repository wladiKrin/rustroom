#!/bin/sh
# Warn when target/ has accumulated too many build artefacts.
#
# ⚠⚠ Why this exists, measured 2026-09-23: target/ had reached 85 GB across 2.39 MILLION files,
# of which ~98.5% were stale — Cargo never garbage-collects, so every dependency bump, feature-set
# change, rustc upgrade and `-p`/`--workspace` switch wrote new fingerprinted artefacts and left
# the old ones there for ever. A *warm* build is unaffected (1 s), but anything cold cost minutes:
# the first run of the test binaries took 437 s with zero compiles, and `du` on target/debug/deps
# exceeded a two-minute timeout. After reclaiming it: cold run 11 s, whole gate 79 s.
#
# ⚠ Counts FILES, not bytes, and the count is BOUNDED: `find | head -N` makes find die of SIGPIPE
# once the cap is reached, so this costs at most N stats however large the tree is — 155 ms at a
# healthy 36.5k files, ~1.3 s at the cap. A `du` would have been O(tree) and would therefore have
# been slowest exactly when it most needed to run, which is the wrong shape for a session hook.
#
# Warns; never blocks. Always exits 0.

CAP=${RUSTROOM_TARGET_FILE_BUDGET:-250000}
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target="$root/target"

[ -d "$target" ] || exit 0

n=$(find "$target" -type f 2>/dev/null | head -n $((CAP + 1)) | wc -l | tr -d ' ')

if [ "$n" -gt "$CAP" ]; then
  printf '{"systemMessage":"⚠ target/ holds more than %s build files (a healthy tree here is ~37k). Cargo never prunes stale artefacts. Reclaim with: rm -rf target/debug — and see AGENTS.md on using ONE cargo invocation mode, which is what makes them pile up."}\n' "$CAP"
fi

exit 0
