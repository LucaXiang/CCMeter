# Unified project cards — Phase 2 (card-face provider split) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** On a project card that has both Claude and Codex usage, show a compact per-provider cost split (`ᴄᴄ $30 · ᴄx $12`) so the user sees, at a glance, how much of a repo's spend came from each provider.

**Architecture:** The cache entry's root (`CODEX_ROOT` vs a Claude install root) is the provider tag. `build_cards` already iterates `cache.iter_filtered(roots, cwds)` yielding `(root, cwd, date, entry)`. Split the per-card cost by `root == CODEX_ROOT` into two new `ProjectCard` fields, then render them on the card face when both are non-zero. No schema/grouping change.

**Tech Stack:** Rust 2021, ratatui. Build/test pinned: `cargo +1.95.0` (binary crate, no `--lib`).

**Spec:** `docs/superpowers/specs/2026-06-02-unified-project-cards-design.md` (decision A, "card-face provider split").

---

## File Structure
- `src/ui/cards/data.rs` — add `cost_claude`/`cost_codex` to `ProjectCard`; populate in `build_cards` by cache root.
- `src/ui/cards/render.rs` — render the split on the card face (`render_card`, line-1 area) when both > 0, with a width guard.

---

## Task 1: Split per-card cost by provider (data)

**Files:**
- Modify: `src/ui/cards/data.rs` (`ProjectCard` struct ~line 13; `build_cards` loop ~line 91; `ProjectCard { … }` literal ~line 178)
- Test: inline `#[cfg(test)] mod tests` in `src/ui/cards/data.rs` (add the module if none exists)

- [ ] **Step 1: Write the failing test**

`build_cards` needs a cache with one group whose cwd has BOTH a Claude-root entry and a CODEX_ROOT entry on the same cwd, and assert the card splits cost by provider. Add to `src/ui/cards/data.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::discovery::{ProjectGroup, ProjectSource, Provider};
    use crate::data::cache::{Cache, DayEntry};
    use crate::data::codex::CODEX_ROOT;
    use std::path::PathBuf;

    fn group(name: &str, cwd: &str) -> ProjectGroup {
        ProjectGroup {
            name: name.into(),
            root_path: PathBuf::from(format!("/repo/{name}")),
            remote_url: None,
            sources: vec![ProjectSource {
                dir_name: name.into(),
                path: PathBuf::from(cwd),
                session_files: vec![],
                cwd: Some(cwd.into()),
                source_root: PathBuf::from("/r"),
                provider: Provider::Claude,
            }],
            total_sessions: 0,
            override_info: None,
        }
    }

    fn entry(cost: f64) -> DayEntry {
        DayEntry { cost, input: 10, output: 5, ..Default::default() }
    }

    #[test]
    fn card_cost_splits_by_provider() {
        // Same cwd "/p/crab" has a Claude-root entry and a CODEX_ROOT entry.
        let mut cache = Cache::new();
        cache.entry_root("/Users/x/.claude/projects".into())
            .entry("/p/crab".into()).or_default()
            .insert("2026-05-04".into(), entry(30.0));
        cache.entry_root(CODEX_ROOT.into())
            .entry("/p/crab".into()).or_default()
            .insert("2026-05-04".into(), entry(12.0));

        let groups = vec![group("crab", "/p/crab")];
        let overrides = Overrides::default();
        let cards = build_cards(
            &groups, &cache, &overrides, &RootFilter::All,
            |_| true, &HashMap::new(), None, &HashMap::new(),
        );
        let crab = cards.iter().find(|c| c.name == "crab").expect("crab card");
        assert!((crab.total_cost - 42.0).abs() < 1e-9, "total = both providers");
        assert!((crab.cost_claude - 30.0).abs() < 1e-9, "claude split");
        assert!((crab.cost_codex - 12.0).abs() < 1e-9, "codex split");
    }
}
```

(If `Overrides::default()` isn't available, construct via `Overrides::load()` won't be hermetic — check `src/config/overrides.rs` for a test constructor; if none, derive `Default` is likely present. Adjust the test to however other tests in the crate build an empty `Overrides`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo +1.95.0 test --bin ccmeter cards::data::tests::card_cost_splits_by_provider`
Expected: compile error — `cost_claude`/`cost_codex` fields don't exist.

- [ ] **Step 3: Add fields + populate**

In `ProjectCard` (after `total_cost`, ~line 19):
```rust
    pub total_cost: f64,
    /// Per-provider cost split (Claude install roots vs CODEX_ROOT). Sums to total_cost.
    pub cost_claude: f64,
    pub cost_codex: f64,
```

In `build_cards`, add accumulators near `let mut total_cost = 0.0f64;` (~line 76):
```rust
        let mut cost_claude = 0.0f64;
        let mut cost_codex = 0.0f64;
```

Rename the loop binding `_root` → `root` (~line 91) and, inside the loop after `accumulate_entry(...)`, add:
```rust
            if root == crate::data::codex::CODEX_ROOT {
                cost_codex += entry.cost;
            } else {
                cost_claude += entry.cost;
            }
```
(`entry.cost` is the same value `accumulate_entry` adds to `total_cost`, so the two sub-totals sum to `total_cost`.)

Add to the `ProjectCard { … }` literal (~line 178, after `total_cost,`):
```rust
            total_cost,
            cost_claude,
            cost_codex,
```

- [ ] **Step 4: Run test to verify it passes + build**

Run: `cargo +1.95.0 test --bin ccmeter cards::data::tests::card_cost_splits_by_provider`
then `cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning:'` (no errors; `cost_claude`/`cost_codex` may warn "never read" until Task 2 renders them — that's expected mid-task).
Expected: test PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/cards/data.rs
git commit -m "feat(cards): split per-card cost by provider (claude vs codex)"
```

---

## Task 2: Render the provider split on the card face

**Files:**
- Modify: `src/ui/cards/render.rs` (`render_card`, line-1 block ~lines 385-428)

The split renders right after the cost on line 1, ONLY when both providers contributed (`cost_codex > 0.0 && cost_claude > 0.0`) — pure-Claude and pure-Codex cards stay clean (the total already tells the story). A width guard omits it on narrow cards so line 1 never overflows.

- [ ] **Step 1: Read the current line-1 layout**

Read `render_card` lines ~383-428. Note: `cost_str` is the left span; `padding` right-aligns the efficiency gauge by subtracting the left content width from `content_width`. Any span added to the left MUST have its display width added to the `padding` subtraction (both the `eff_spans` branch and the else branch) so the gauge stays aligned and line 1 doesn't wrap.

- [ ] **Step 2: Add the split segment (manual/visual feature — no unit test for rendered glyphs)**

After `let cost_str = format_cost(card.total_cost);` (~line 386), build the split text + width:
```rust
    // Provider split shown only on mixed (Claude+Codex) cards; keep single-
    // provider cards clean. Plain ASCII labels avoid unicode-width surprises.
    let split_str = if card.cost_codex > 0.0 && card.cost_claude > 0.0 {
        format!("  cc {} · cx {}", format_cost(card.cost_claude), format_cost(card.cost_codex))
    } else {
        String::new()
    };
    // Only render the split if line 1 has room for it (cost + split + a little slack).
    let split_str = if !split_str.is_empty()
        && content_width > cost_str.len() + split_str.len() + 8
    {
        split_str
    } else {
        String::new()
    };
```

Add `split_str.len()` to BOTH `padding` subtractions (the `eff_spans` branch ~line 402 and the else branch ~line 416): change `content_width.saturating_sub(cost_str.len() + time_extra + total_len)` → `...saturating_sub(cost_str.len() + split_str.len() + time_extra + total_len)`, and likewise the else branch.

Insert the split span into `line1_spans` immediately after the cost span (after the `vec![Span::styled(&cost_str, …)]` init, ~line 422), before the `if !time_str.is_empty()` block:
```rust
    if !split_str.is_empty() {
        line1_spans.push(Span::styled(&split_str, Style::default().fg(t.text_dim)));
    }
```
(Use `t.text_dim` so it reads as a secondary annotation; if a theme color for codex exists via `t.model_color("codex")` and you prefer color-coding, that's acceptable — but a single dim segment is the low-risk default.)

- [ ] **Step 3: Build + suite**

Run: `cargo +1.95.0 build --bin ccmeter 2>&1 | rg -i 'error|warning:'` (empty — the Task 1 "never read" warnings are now resolved).
Run: `cargo +1.95.0 test 2>&1 | rg 'test result'` (only the 2 known date-relative `rate_limits` failures).
Run: `cargo +1.95.0 clippy --bin ccmeter 2>&1 | rg 'generated'` (not above the 11 baseline).

- [ ] **Step 4: Real-data probe (orchestrator runs; verify the split is populated)**

The orchestrator adds a temporary `#[ignore]` probe test that calls `build_cards` via the real pipeline OR simply asserts on the crab card's `cost_claude`/`cost_codex` from a real run, prints them, and reverts. (Rendered glyphs are visually confirmed by the user.)

- [ ] **Step 5: Commit**

```bash
git add src/ui/cards/render.rs
git commit -m "feat(cards): show provider cost split on mixed card faces"
```

---

## Phase 2 acceptance
- [ ] A mixed card (e.g. `crab`) shows `cc $X · cx $Y` after its total cost; the two sum to the total.
- [ ] Pure-Claude and pure-Codex cards show no split (stay clean).
- [ ] Narrow cards never overflow line 1 (width guard).
- [ ] `build --bin ccmeter` clean; `test` only the 2 known failures; clippy not above baseline 11.
