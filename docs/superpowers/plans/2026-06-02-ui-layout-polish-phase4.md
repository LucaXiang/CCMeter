# Phase 4 — UI layout polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Three layout fixes the user asked for after seeing the running app:
1. Project cards are too cramped → taller cards (height 6→8), each metric group on its own line.
2. Detail view `Cost/day` and `Tokens/day` charts are side-by-side → stack them, each full-width on its own row.
3. Detail view "Recent sessions" only shows ~5 of hundreds → add up/down scroll with an `N/M` indicator.

**Tech Stack:** Rust 2021, ratatui. Build/test pinned: `cargo +1.95.0` (binary crate, no `--lib`). Rendered glyphs aren't unit-tested; correctness = build/clippy gates + careful layout guards + a real-data probe (orchestrator).

**Spec/decisions:** card layout = the user-approved "Option 2" (height 8, 6 content lines). Charts each own full-width row. Sessions scrollable.

---

## Task 1: Taller, less-cramped project cards (Option 2)

**Files:** `src/ui/cards/render.rs` — `CARD_HEIGHT` const (line 13) + `render_card` (lines ~344-487).

Current: `CARD_HEIGHT = 6` (inner 4 lines): L1 `cost + cc·cx split + ⏱time + ⚡eff + sessions` (overcrowded), L2 tokens, L3 +/- lines, L4 sparkline.

Target (height 8, inner 6 lines), each group its own line:
- L1: cost (bold) ……right-aligned…… sessions count `N sess`
- L2: provider split `cc $X · cx $Y` (dim) — only when `cost_codex>0 && cost_claude>0`; else blank line
- L3: `⏱ <time>`   …   `⚡ <gauge> <eff> tok/ln` (the time + efficiency line)
- L4: `in: … out: … cache: …` (unchanged content, now its own line)
- L5: `+added -deleted` (unchanged)
- L6: sparkline (unchanged)

- [ ] **Step 1: Read** `render_card` (344-487) and note: the line-1 padding math (`eff_spans` right-align), the `split_str` block (Phase 2), `format_cost/format_tokens/format_duration/efficiency_gauge`, and `card.sessions`.

- [ ] **Step 2: Change `CARD_HEIGHT`**

`const CARD_HEIGHT: u16 = 8;` (was 6).

- [ ] **Step 3: Rewrite `render_card`'s line construction to 6 lines.** Build:
  - `line1`: cost span (bold, `t.cost`) + right-aligned `format!("{} sess", card.sessions)` (dim). Right-align by computing `padding = content_width.saturating_sub(cost_str.len() + sess_str.len())` and inserting `Span::raw(" ".repeat(padding))` between.
  - `line2`: the provider split. Reuse the Phase 2 `split_str` logic (mixed-only: `cc $X · cx $Y`), styled `t.text_dim`; empty `Line::default()` when not mixed.
  - `line3`: time + efficiency. `⏱ <format_duration(time_minutes)>` (when >0) on the left (`t.duration`), then the efficiency gauge group (`⚡ ` + `efficiency_gauge(...)` + ` {:.0} tok/ln`) right-aligned via padding (mirror the existing eff_spans right-alignment, but now alone on its line so the math is simpler: `padding = content_width - time_len - eff_group_len`).
  - `line4`: in/out/cache (move the current line2 code verbatim).
  - `line5`: +added/-deleted (move the current line3 code verbatim).
  - `line6`: sparkline (the current line4 `render_sparkline_with_models(...)`).
  - `let text = vec![line1, line2, line3, line4, line5, line6];`
  Remove the now-obsolete combined line-1 (cost+split+time+eff) construction. Keep the early-return guards but update `if inner.height < 4` → keep (still valid; with height 8 inner is 6).

- [ ] **Step 4: Build + visual sanity**

`cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning:'` → empty.
`cargo +1.95.0 test 2>&1 | rg 'test result'` → only the 2 known failures.
Note: the grid `render` fn (line 227) uses `CARD_HEIGHT` for rows/scroll — it adapts automatically (fewer cards per screen, scroll already exists).

- [ ] **Step 5: Commit** `feat(cards): taller card layout, one metric group per line`.

---

## Task 2: Stack detail charts full-width (Cost/day over Tokens/day)

**Files:** `src/ui/cards/render.rs` — `render_detail_charts` (~1217).

Current: `chart_cols` splits `vert_split[0]` Horizontally 50/50 → Cost left, Tokens right; shared legend (vert_split[1]) + x-axis (vert_split[2]) at the bottom.

Target: Cost/day on top full-width, Tokens/day below full-width, each with its own header/legend and x-axis row.

- [ ] **Step 1: Read** `render_detail_charts` fully (1217 to its end) — understand how `left_split`/`right_split`, the legends (`left_spans`/`right_spans`), the chart bodies, and the x-axis labels are rendered into `vert_split[1]`/`[2]`.

- [ ] **Step 2: Restructure the layout** so the area splits into two stacked full-width chart blocks. Replace the top-level split with:
```rust
    let halves = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(charts_area);
    // Each half: [legend/header Length(1), chart Min(3), x-axis Length(1)]
    let cost_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)])
        .split(halves[0]);
    let tok_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)])
        .split(halves[1]);
```
Render the Cost/day header+legend into `cost_rows[0]`, the cost chart into `cost_rows[1]`, its x-axis into `cost_rows[2]`; likewise Tokens/day into `tok_rows[*]`. Reuse the existing legend-building and chart-drawing code, just retargeted to the new full-width rects (each chart now gets `charts_area.width` instead of half). Preserve the existing height guard (if `charts_area.height` is too small, degrade — keep a guard like `if charts_area.height < 6 { render single combined as before }` OR just let Min(3) clamp).

- [ ] **Step 3: Build + suite** (same gates as Task 1 Step 4).

- [ ] **Step 4: Commit** `feat(detail): stack Cost/day and Tokens/day full-width`.

---

## Task 3: Scrollable "Recent sessions" in the detail view

**Files:** `src/app.rs` (scroll state + key handling), `src/ui/cards/render.rs` (`render_detail` + `render_recent_sessions`), `src/ui/dashboard.rs` (pass offset).

`RenderCache.detail_sessions` already holds the FULL filtered+sorted list (Phase 3). `render_recent_sessions` currently shows `.take(rows)` from the top — no scroll.

- [ ] **Step 1: Add scroll state** in `src/app.rs`:
  - Add `pub(crate) detail_session_scroll: usize` to `App` (init `0` in `App::new`).
  - Reset it to `0` wherever `card_scroll` is reset on project change / Esc (the `KeyCode::Left/Right` arms ~712-731 and `KeyCode::Esc` ~691): set `self.detail_session_scroll = 0;` alongside `self.card_scroll = 0;`.

- [ ] **Step 2: Repurpose Up/Down in detail view** (`KeyCode::Down|Char('j')` ~706 and `Up|Char('k')` ~709). When `self.project_index.is_some()`, scroll the sessions instead of cards:
```rust
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.project_index.is_some() {
                        let max = self.render.detail_sessions.len().saturating_sub(1);
                        self.detail_session_scroll = (self.detail_session_scroll + 1).min(max);
                    } else {
                        self.card_scroll = self.card_scroll.saturating_add(1);
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.project_index.is_some() {
                        self.detail_session_scroll = self.detail_session_scroll.saturating_sub(1);
                    } else {
                        self.card_scroll = self.card_scroll.saturating_sub(1);
                    }
                }
```

- [ ] **Step 3: Thread the offset to the renderer.** In `src/ui/dashboard.rs`, the `cards::render_detail(...)` call: add `self.detail_session_scroll` as a new last arg. In `render_detail` (render.rs ~900), add `session_scroll: usize` param and pass it to `render_recent_sessions`. In `render_recent_sessions`, render the window `sessions.iter().skip(scroll).take(rows)` and show a scroll indicator in the header when `sessions.len() > rows`: ` Recent sessions  (<shown range>/<total>)` e.g. `Recent sessions  6-10/879 ↑↓`. Clamp `scroll` to `len.saturating_sub(1)` defensively inside the renderer too.

- [ ] **Step 4: Footer hint (optional, nice):** the detail-view footer (`src/ui/dashboard.rs` hints, or wherever the bottom hint line for detail is) could mention `↑↓ Sessions` when in detail view. Only if there's an obvious spot; skip if it complicates.

- [ ] **Step 5: Build + suite + clippy** (gates). Manual: in detail view, ↑↓ scrolls the session list; the indicator updates; switching project resets to top.

- [ ] **Step 6: Commit** `feat(detail): scrollable Recent sessions list`.

---

## Phase 4 acceptance
- [ ] Cards are height 8 with each metric group on its own line; no line-1 overcrowding; grid still scrolls.
- [ ] Detail view shows Cost/day and Tokens/day stacked full-width.
- [ ] Detail "Recent sessions" scrolls with ↑↓/j/k, shows `range/total`, resets on project switch.
- [ ] `build --bin ccmeter` clean; `test` only the 2 known failures; clippy not above 11.

(i18n / Chinese localization is Phase 5 — separate plan.)
