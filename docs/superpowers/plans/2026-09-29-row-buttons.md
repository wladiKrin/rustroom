# Row buttons — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use `- [ ]`.
> **Commits carry no `Co-Authored-By` trailer. Never push.**

**Goal:** list-row action glyphs always drawn at `text3`, `text` when the pointer is on the row, a
`hover` fill under the pointed cell; cells 20 pt wide.

**Spec:** `docs/superpowers/specs/2026-09-29-row-buttons-design.md`.

**Gate:** fmt · `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings`
· `cargo test --workspace`.

1. [ ] **Token.** `theme/tokens.rs`: `pub const ROW_ACTION: f32 = 20.0;` beside `LIST_ROW`, with a doc
   line (the cell width of a list row's action; 30 pt tall with the row).
2. [ ] **Tests first** in `list_row`'s `mod tests`: paint a row with two actions with no pointer and
   with the pointer on the row's name; collect the painted glyph shapes (`Shape::Text` whose text is `×`
   or `+`) with their colours; assert (a) both glyphs painted at rest, (b) the rest colour is
   `tokens.text3` and the hovered colour `tokens.text`, and the two differ; (c) the AccessKit centres of
   "Delete {row}" and the second action are `ROW_ACTION` apart. Run → (a)–(c) fail.
3. [ ] **Implement** in `row_with`: cell width `tokens::ROW_ACTION`; colour per cell —
   disabled `text3.gamma_multiply(0.45)`, else `text` when `pointer_here`, else `text3`; remember
   `r.hovered() && on` per cell and paint a `tk.hover` rounded fill under that cell before its glyph;
   paint the glyphs unconditionally. Rewrite the P-13 comment: reversed by the author 2026-09-29.
4. [ ] **Run** `cargo test --workspace list_row` and `every_count_in_the_rail` → pass.
5. [ ] **Sabotages** (each red, reverted): paint under `pointer_here` only; `text3` for both states;
   cell width `ROW_HEIGHT`.
6. [ ] **Commit** `feat(app): list-row buttons always drawn …`.
7. [ ] **Gate**, then docs: `DESIGN.md` §7.6, §5 (the token), §13 (the decision); the journal (a round
   in M10.6a's entry); CLAUDE.md state. Commit. GUI.
