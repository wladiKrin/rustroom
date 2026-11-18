# The display's own colours on Windows and Linux: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. ⚠ This project runs plans **inline** (executing-plans) by the author's standing choice.

**Goal:** on Windows and Linux the preview, thumbnails and previews are converted into the monitor's own colours —
an assigned profile, else its EDID primaries — unless the OS already does it; Settings → Display defaults to Automatic.

**Architecture:** `color::edid` (pure parser) and `color::display` (`Display`, `DisplayProfile` — LittleCMS sampled
into a 3-D table) carry no GUI. `rustroom_app::display_detect` gathers per-platform FACTS and decides in ONE pure
function. `color::Display` replaces `OutputSpace` on the display path only.

**Spec:** `docs/superpowers/specs/2026-11-18-display-colour-design.md`

## Global Constraints

- Exports, soft proofing and `OutputSpace` unchanged; macOS behaviour unchanged.
- Every fast-path gate that read `display_space == Srgb` reads `Display::is_srgb()` — never a second definition.
- The EDID fixture is CAPTURED from the M27Q, with its serial number blanked and the checksum redone — said in its
  README.
- One cargo mode; `CARGO_BUILD_JOBS=2` for test builds on this machine; no commit trailer; never push.

## Review Focus

1. **Never two conversions:** the OS managing colour (macOS tag, Windows WCG/HDR) yields sRGB in `decide`, tested.
2. **The table is measured against LittleCMS itself**, and a P3 profile through it agrees with the existing P3 matrix
   path (two implementations, one answer) — so neither can be wrong alone.
3. **The histogram and the sRGB readout stay image-referred** under a profile (the existing pack tests, extended).
4. **A manual choice in an old settings file survives** the new Automatic default.
5. **The windows FFI is checked by use** on the M27Q: Automatic must name it, and switching on Windows' Auto Color
   Management must turn our conversion off within two seconds.

### Task 1: `color::edid`
- [ ] Capture the M27Q's EDID (registry), blank the serial, fix the checksum → `crates/color/testdata/m27q.edid` + README.
- [ ] Tests first: the M27Q's primaries, white, gamma, name; a bad checksum is `None`; the sRGB flag read; a
  constructed sRGB EDID `is_close_to_srgb`.

### Task 2: `color::display`
- [ ] `lcms2` joins `color`'s dependencies. `DisplayProfile::from_primaries` / `from_icc`, sampled to an N³ table;
  `Display { Space(OutputSpace), Profile(Arc<DisplayProfile>) }` with `is_srgb`, `convert` (float, encoded in and out),
  `convert_srgb8`, equality by profile id.
- [ ] Tests first: table vs LittleCMS on 4,096 samples (choose N by the measured maximum); an sRGB-primaries profile
  is the identity within 1/255; a P3 ICC through `from_icc` matches `OutputSpace::DisplayP3` within 1/255; the M27Q
  makes pure sRGB green less green and more red/blue in display values.

### Task 3: the display path carries `color::Display`
- [ ] Session, frame request, `pack`, the render gates, `convert_thumb`, `ThumbLoader`, `PreviewLoader`,
  `Settings::effective_display_space` → `color::Display`. The compiler lists the sites; tests adjust.
- [ ] A pack test under a profile: pixels change, histogram and sRGB channel do not; a render-gate test: a profile
  keeps the float path.

### Task 4: `rustroom_app::display_detect`
- [ ] `Facts { os_managed, icc: Option<(name, bytes)>, edids: Vec<bytes> }`, `decide(&Facts) -> Detected`, tests for
  every branch and its order.
- [ ] Windows (`windows-sys`): the window's monitor → device → EDID (registry), Advanced Color (`activeColorMode`,
  HDR), `GetICMProfileW` (the generic sRGB profile counts as none). Linux: `/sys/class/drm/*/{status,edid}`.

### Task 5: Settings and wiring
- [ ] `Settings.display_auto` (default true; a file with a manual non-sRGB space and no field keeps manual);
  `effective_display(os_managed, &Detected)`; Settings → Display: Automatic + the three, a caption under Automatic.
- [ ] Bootstrap detects with the window; a pump re-checks every 2 s and re-applies on change (the same steps as
  `apply_display_space`). `RUSTROOM_DEBUG_DISPLAY` prints the detection.

### Task 6: Measure
- [ ] `pack` at 3200×2136: none / P3 matrix / profile table — record; the fast paths' cost is M8.3g's.

### Task 7: Docs, gate, hand over
- [ ] README/CONTRIBUTING lines if user-visible behaviour needs saying; `AGENTS.md` one rule; journal; backlog.
- [ ] fmt, clippy, test on the tip; launch; the author compares Automatic against the monitor's own sRGB mode.
