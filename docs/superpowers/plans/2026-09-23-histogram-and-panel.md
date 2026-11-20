# The histogram, the ghost, the curve's centring and the panel's width — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A histogram whose shape survives one huge peak and visibly responds to edits; a readable ghost behind the tone curve; that curve centred like the colour wheel; and a wider default panel.

**Architecture:** One scaling function pair in `histogram.rs` (zero GUI, unit-testable) replaces the duplicated max computation at both draw sites, so the panel's histogram and the curve's ghost cannot drift apart. The other three are single-constant or single-layout changes.

**Tech Stack:** Rust, egui (bin modules only).

**Spec:** `docs/superpowers/specs/2026-09-23-histogram-and-panel-design.md`. Read it before Task 1.

---

## Conventions for every task

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By` trailer. Never push.** Branch is already `histogram-and-panel`.
- ⚠⚠ **ONE cargo invocation mode.** Always `cargo test --workspace <name-filter>`, **never `-p`** — alternating costs 43–531 s per switch, measured.
- ⚠⚠ **The filter is a SUBSTRING, not a regex,** and one matching nothing **exits 0 having run zero tests.** Check the passed count, never the exit code alone.
- **Write cargo output to a file and read `$?` from its own command.** A `| grep` reports grep's status and has produced a false green here.
- ⚠⚠ **After any scripted edit, verify nothing was eaten**: `git diff <commit> -- <file> | grep '^-' | grep '///'` must be empty unless you meant it. A scripted move in `copies.rs` destroyed a 17-line doc block on 2026-09-23 and the whole suite stayed green.
- ⚠ Plan snippets go stale when `cargo fmt` rewraps a line. If an anchor does not match, **read the file** — do not force it. One anchor in the last plan was off by the single word "that".
- **egui only in bin modules.** `histogram.rs` is a **lib** module: no egui in Task 1.

## File structure

| File | Change | Size |
|---|---|---|
| `crates/app/src/histogram.rs` | **new** `normalised` + `envelope` + scaling, and their tests | 71 |
| `crates/app/src/develop/panel/mod.rs` | `draw_histogram` uses them; `default_size` 270 → 320 | — |
| `crates/app/src/develop/panel/tone_curve.rs` | ghost uses `envelope`, opacity constant, widget centred, stale doc | — |

---

### Task 1: The scale survives a spike

**Files:** modify `crates/app/src/histogram.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`:

```rust
    /// The share of the well a bar must reach to count as legible here.
    const VISIBLE: f32 = 0.05;

    /// ⚠⚠ **The report, as a test (2026-09-23).** A large flat area — a sky, a wall — puts one
    /// enormous count in one bin. Scaling linearly to the tallest bin squashed everything else onto
    /// the floor, which is both "large peaks drown out the whole view" and "very static": if the shape
    /// is pinned at zero, an edit that moves midtones moves nothing you can see.
    #[test]
    fn a_huge_peak_no_longer_flattens_the_rest() {
        let mut h = [[0u32; 256]; 3];
        h[0][128] = 1_000_000; // the flat area
        for c in 0..3 {
            h[c][64] = 500; // ordinary tonal content, 2000x smaller
            h[c][192] = 500;
        }
        let linear = 500.0 / 1_000_000.0;
        assert!(linear < 0.001, "linearly this was invisible: {linear}");

        let n = normalised(&h);
        assert!(
            n[0][64] > VISIBLE,
            "the small counts must be legible, got {}",
            n[0][64]
        );
        assert!(n[0][128] > n[0][64], "and the peak must still be the tallest");
    }

    /// ⚠⚠ The targeted half of the fix: a blown sky or a crushed black is an **edge**, not tonal
    /// content, and it must not set the scale for the graph.
    #[test]
    fn a_clipped_edge_does_not_set_the_scale() {
        let mut h = [[0u32; 256]; 3];
        h[0][255] = 1_000_000; // blown highlights
        h[0][100] = 400;
        let n = normalised(&h);
        assert!(
            n[0][100] > 0.9,
            "the midtone should nearly fill the well, got {}",
            n[0][100]
        );
        let mut h2 = [[0u32; 256]; 3];
        h2[1][0] = 1_000_000; // crushed shadows
        h2[1][100] = 400;
        assert!(normalised(&h2)[1][100] > 0.9);
    }

    /// ⚠ Left out of the *scale*, still drawn — clamped rather than running off the top.
    #[test]
    fn the_edge_bins_are_still_drawn_and_clamped() {
        let mut h = [[0u32; 256]; 3];
        h[0][255] = 1_000_000;
        h[0][100] = 400;
        let n = normalised(&h);
        assert_eq!(n[0][255], 1.0, "drawn at full height, not above it");
        assert!(n[0][255].is_finite());
    }

    /// ⚠⚠ **The degenerate case, which would otherwise be `NaN`.** A frame whose only content sits at
    /// the two edges has a non-edge maximum of zero, and dividing by it paints nothing at all.
    #[test]
    fn a_frame_with_content_only_at_the_edges_is_finite() {
        let mut h = [[0u32; 256]; 3];
        h[0][0] = 5000;
        h[0][255] = 5000;
        let n = normalised(&h);
        assert!(
            n.iter().flat_map(|c| c.iter()).all(|v| v.is_finite()),
            "every height must be finite"
        );
        assert!(n[0][0] > 0.0, "and the content must be visible");
    }

    #[test]
    fn an_empty_frame_is_all_zero_and_does_not_divide_by_zero() {
        let n = normalised(&[[0u32; 256]; 3]);
        assert!(n.iter().flat_map(|c| c.iter()).all(|&v| v == 0.0));
    }

    /// ⚠ More samples is never drawn shorter. Obvious, and the property a bad scaling breaks.
    #[test]
    fn more_samples_is_never_drawn_shorter() {
        let mut h = [[0u32; 256]; 3];
        h[0][50] = 10;
        h[0][51] = 100;
        h[0][52] = 1000;
        let n = normalised(&h);
        assert!(n[0][50] < n[0][51] && n[0][51] < n[0][52]);
    }

    /// ⚠ The ghost is the per-bin maximum over the channels, so a saturated colour is not hidden by
    /// its dark siblings — the behaviour the tone curve already documented.
    #[test]
    fn the_envelope_takes_the_brightest_channel_per_bin() {
        let mut h = [[0u32; 256]; 3];
        h[0][100] = 1000; // red only
        h[1][100] = 1;
        h[2][100] = 1;
        h[1][150] = 1000; // green only
        let e = envelope(&h);
        assert!(e[100] > 0.9 && e[150] > 0.9);
        assert_eq!(e[200], 0.0);
    }

    /// ⚠ One scaling, two callers: the panel's histogram and the curve's ghost cannot drift apart.
    /// They each computed their own maximum before this.
    #[test]
    fn the_envelope_and_the_channels_share_one_scale() {
        let mut h = [[0u32; 256]; 3];
        h[0][100] = 1000;
        h[1][100] = 10;
        h[2][100] = 10;
        assert_eq!(envelope(&h)[100], normalised(&h)[0][100]);
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test --workspace histogram > /tmp/h.log 2>&1; echo "EXIT: $?"
grep -E '^error' -A 4 /tmp/h.log | head
```

Expected: FAIL to compile — `normalised` and `envelope` do not exist.

- [ ] **Step 3: Implement**

Add to `histogram.rs`:

```rust
/// The bins left out of the scale: pure black and pure white.
///
/// ⚠⚠ A blown sky or a crushed black is an **edge**, not tonal content, and it is the commonest cause
/// of a spike that flattens the graph. They are still drawn — clamped by the mesh — and
/// [`clipped_edges`] already reports them, so nothing is hidden by leaving them out of the scale.
const EDGE_BINS: [usize; 2] = [0, 255];

/// The tallest bin that counts towards the scale, across every channel.
///
/// ⚠ Falls back to the overall maximum when the non-edge bins are all empty: a frame whose only
/// content sits at the two edges would otherwise divide by zero and paint `NaN`.
fn scale_reference(hist: &[[u32; 256]; 3]) -> f32 {
    let counts = |skip_edges: bool| {
        hist.iter()
            .flat_map(|c| c.iter().enumerate())
            .filter(move |(i, _)| !skip_edges || !EDGE_BINS.contains(i))
            .map(|(_, &n)| n)
            .max()
            .unwrap_or(0)
    };
    let m = match counts(true) {
        0 => counts(false),
        n => n,
    };
    (m as f32).sqrt()
}

/// Normalised bar heights per channel, in `0.0..=1.0`.
///
/// ⚠⚠ **Square root, not linear** (user's choice of four, 2026-09-23). Linear scaling to the tallest
/// bin squashed ordinary tonal content onto the floor whenever the frame held one large flat area —
/// which read both as "large peaks drown out the whole view" and as "very static", because a shape
/// pinned at zero does not visibly move when you edit. ⚠ Chosen over log, which flatters almost-empty
/// bins so that sensor noise in an empty shadow reads as tonal content.
pub fn normalised(hist: &[[u32; 256]; 3]) -> [[f32; 256]; 3] {
    let scale = scale_reference(hist);
    if scale <= 0.0 {
        return [[0.0; 256]; 3];
    }
    std::array::from_fn(|c| std::array::from_fn(|i| ((hist[c][i] as f32).sqrt() / scale).min(1.0)))
}

/// One curve for the tone curve's ghost: the per-bin maximum across the channels, on the **same**
/// scale as [`normalised`].
///
/// ⚠ The maximum rather than the sum or the mean, so a saturated colour is not hidden by its dark
/// siblings — the tone curve already worked this way and this keeps it.
pub fn envelope(hist: &[[u32; 256]; 3]) -> [f32; 256] {
    let n = normalised(hist);
    std::array::from_fn(|i| n.iter().map(|c| c[i]).fold(0.0f32, f32::max))
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace histogram > /tmp/h.log 2>&1; echo "EXIT: $?"
grep -cE '^test ' /tmp/h.log; grep -E 'FAILED|panicked' /tmp/h.log | head
```

Expected: PASS, with a non-zero count and the two pre-existing tests still green.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/histogram.rs
git commit -m "The histogram's scale survives one huge peak

Square root rather than linear, with bins 0 and 255 left out of the scale: a blown sky or a
crushed black is an edge, not tonal content, and it was the commonest cause of a spike that
squashed everything else onto the floor. That flattening was both reports — peaks drowning the
view, and the graph looking static, because a shape pinned at zero does not move when you edit.

Both draw sites computed their own maximum; one function pair now serves both, so the panel's
histogram and the curve's ghost cannot drift apart. A frame whose only content sits at the two
edges falls back to the overall maximum rather than dividing by zero and painting NaN."
```

---

### Task 2: Both draw sites use it

**Files:** modify `crates/app/src/develop/panel/mod.rs`, `crates/app/src/develop/panel/tone_curve.rs`

- [ ] **Step 1: The panel's histogram**

In `draw_histogram`, delete the local `max` block and replace the per-channel bins:

```rust
fn draw_histogram(ui: &mut egui::Ui, hist: &[[u32; 256]; 3]) {
    // ⚠ The scale lives in `histogram.rs`, shared with the tone curve's ghost. See `normalised`.
    let bins = rustroom_app::histogram::normalised(hist);
    let (rect, _) = ui.allocate_exact_size(
```

and in the channel loop:

```rust
    for (ci, chan) in bins.iter().enumerate() {
        painter.add(egui::Shape::mesh(area_mesh(
            plot,
            chan,
            CHANNEL_COLOURS[ci].linear_multiply(0.55),
        )));
    }
```

⚠ `clipped_edges` is still called on the **raw** `hist`, not on `bins` — it is about counts, not
heights. Leave that line exactly as it is.

- [ ] **Step 2: The ghost**

In `tone_curve.rs`, replace the `max` block and the `bins` construction with:

```rust
        if let Some(h) = hist {
            let bins = rustroom_app::histogram::envelope(h);
            painter.add(egui::Shape::mesh(super::area_mesh(
                rect,
                &bins,
                v.weak_text_color().linear_multiply(GHOST_OPACITY),
            )));
        }
```

and add the constant beside the other consts at the top of the file:

```rust
/// How strongly the ghost histogram is painted behind the curve.
///
/// ⚠ 0.35, up from 0.15 (user's call, 2026-09-23): at 0.15 it was there but not readable. A number
/// to tune by looking, which is why it is named rather than inline.
const GHOST_OPACITY: f32 = 0.35;
```

- [ ] **Step 3: Build and run the suite**

```bash
cargo build -p app > /tmp/b.log 2>&1; echo "BUILD EXIT: $?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST EXIT: $?"
```

⚠ Expect the **74 goldens unmoved** — this touches no pipeline stage.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/develop/panel
git commit -m "Both histograms draw from the shared scale, and the ghost is readable

The ghost goes from 0.15 to 0.35: at 0.15 it was present but not readable. clipped_edges still
reads the raw counts, not the scaled heights — it is about quantity, not drawn size."
```

---

### Task 3: The curve is centred, like the wheel

**Files:** modify `crates/app/src/develop/panel/tone_curve.rs`

- [ ] **Step 1: Centre it**

Replace:

```rust
    let size = ui.available_width().min(MAX_SIZE);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
```

with:

```rust
    let full = ui.available_width();
    let size = full.min(MAX_SIZE);
    // ⚠⚠ **Centred, the same way the colour wheel is** (`color_wheel.rs`, user's call 2026-09-02):
    // `allocate_exact_size` in a vertical layout starts at the left edge, so in a panel wider than
    // `MAX_SIZE` the curve sat left while everything around it spanned the full width.
    //
    // ⚠ The pad is allocated **beside** the widget rather than by widening its rect, so the click
    // target stays tight to the curve — a full-width response would let a drag grab a control point
    // from anywhere on the row.
    let pad = ((full - size) * 0.5).max(0.0);
    let (rect, resp) = {
        let mut allocated = None;
        ui.horizontal(|ui| {
            ui.add_space(pad);
            allocated = Some(ui.allocate_exact_size(
                egui::vec2(size, size),
                egui::Sense::click_and_drag(),
            ));
        });
        allocated.expect("the horizontal closure always runs")
    };
```

- [ ] **Step 2: Correct the stale doc on `MAX_SIZE`**

```rust
/// Widest the editor is drawn, and it stays **fixed** while the panel grows.
///
/// ⚠ The panel is 320 pt (2026-09-23). The curve is deliberately narrower and centred, the way the
/// colour wheel is: growing it to fill the panel would make that centring a no-op. ⚠ Its doc said
/// "the panel is 240 pt" until 2026-09-23, stale since the panel went to 270 two days earlier —
/// **a size named in a comment is a size that goes stale.**
const MAX_SIZE: f32 = 220.0;
```

- [ ] **Step 3: Build**

```bash
cargo build -p app > /tmp/b.log 2>&1; echo "BUILD EXIT: $?"
```

⚠ If the borrow checker objects to `resp` escaping the closure, the colour wheel's exact pattern is
already in `color_wheel.rs` around line 220 — copy its shape rather than inventing one.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/develop/panel/tone_curve.rs
git commit -m "The tone curve is centred in the panel, like the colour wheel

allocate_exact_size starts at the left edge in a vertical layout, so in a panel wider than the
curve it sat left of everything around it. The pad goes beside the widget rather than into its
rect, so a drag cannot grab a control point from the empty space."
```

---

### Task 4: A wider panel

**Files:** modify `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Widen it**

```rust
        // ⚠ 320, up from 270 (2026-09-23, user's call); 270 was up from 240 (2026-09-21), when the
        // snapshot strip's Now row and the longer slider names were cut off.
        //
        // ⚠⚠ The panel is **not persisted** — egui's panel state is not saved here — so this default
        // is the width every launch actually gets, not merely a first-run hint.
        .default_size(320.0)
```

- [ ] **Step 2: Gate**

```bash
cargo fmt --all
cargo fmt --all -- --check > /tmp/f.log 2>&1; echo "FMT EXIT: $?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY EXIT: $?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST EXIT: $?"
```

Expected: all three **0**, the **74 goldens unmoved**.

- [ ] **Step 3: Verify no scripted edit ate anything**

```bash
git diff d751faf -- crates/app/src | grep '^-' | grep -v '^---' | grep '///'
```

Expected: only doc lines you deliberately replaced (`MAX_SIZE`'s, and the ghost's `max` block).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "The Develop panel defaults to 320 pt

Its state is not persisted, so the default is the width every launch gets."
```

---

### Task 5: Look at it

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app
```

- [ ] **Step 2: What to check, on real photographs**

1. A photo with a **large flat area** — sky, wall, snow. The histogram should show tonal structure
   where it used to show one spike and a flat line.
2. Drag **Exposure** and watch it move. That is the "static" complaint answered, or not.
3. A photo with a **blown sky**: the clipping triangle still lights, and the rest of the graph is not
   squashed.
4. The **ghost** behind the tone curve is readable but still clearly a background.
5. The curve sits **centred**, like the colour wheel below it.
6. ⚠ A deliberately **high-contrast** frame, mostly pure black and pure white: it must draw something,
   not an empty well. That is the degenerate case the fallback exists for.

- [ ] **Step 3: Write it up**

⚠ Add a dated section at the top of `docs/JOURNAL.md` and overwrite `CLAUDE.md`'s live state **in
place**. ⚠ `CLAUDE.md` is at **400** lines against a ~400 target: something must move out rather than
in.

---

## Self-review against the spec

| Spec requirement | Task |
|---|---|
| sqrt scaling, the user's choice | 1 |
| bins 0 and 255 excluded from the scale | 1 |
| edges still drawn, clamped | 1 |
| the edges-only frame is finite, not `NaN` | 1 |
| an empty frame does not divide by zero | 1 |
| monotonic: more samples never shorter | 1 |
| `envelope` is the per-bin max across channels | 1 |
| one scale for both callers, proven by a test | 1 |
| both draw sites use it; `clipped_edges` still reads raw counts | 2 |
| ghost 0.15 → 0.35, as a named constant | 2 |
| curve centred with the wheel's pad pattern | 3 |
| `MAX_SIZE` stays 220, stale doc corrected | 3 |
| panel 270 → 320 | 4 |
| no golden movement | 4 |
| per-channel RGB curves deferred, not built | — |
| seen in use | 5 |

**Known soft spots, flagged rather than hidden:**

- **`GHOST_OPACITY` and the panel's 320 are taste, not derived.** Both are single named constants so
  they are one-line changes after looking.
- **Square root is a choice, not a truth.** It no longer shows quantity proportionally: a bar twice as
  tall is four times as many pixels. That is the trade the user picked over a percentile clip, and it
  is what every histogram that compresses gives up.
- **`EDGE_BINS` is exactly `[0, 255]`** — the two bins the clipping markers already describe. ⚠ If
  someone widens it to "the outer few bins", the clipping marker and the scale would then disagree
  about what an edge is.
